//! ブラウザ操作ツール (browser_tools)
//! Phase 3: Chromium と Chrome DevTools Protocol (CDP) を用いたブラウザ自動化
//! セッション単位でブラウザ状態を保持し、連続した操作に対応

use super::definition::ToolDefinition;
use crate::error::AiCoreError;
use dashmap::DashMap;
use scraper::{ElementRef, Html, Node};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

/// 生HTMLを抽出（extract・trimに対応）
fn extract_html(html: &str, extract: &str, trim: bool) -> String {
    let document = Html::parse_document(html);

    let target = select_target_element(&document, extract);

    if trim {
        serialize_element_clean(target)
    } else {
        target.html()
    }
}

/// Markdownを抽出（extract・trimに対応）
fn extract_markdown(html: &str, extract: &str, trim: bool) -> String {
    let html_input = if trim || extract != "full" {
        extract_html(html, extract, trim)
    } else {
        html.to_string()
    };

    let md = html2md::parse_html(&html_input);
    normalize_text(&md)
}

/// 抽出対象の要素を選択
fn select_target_element<'a>(document: &'a Html, extract: &str) -> ElementRef<'a> {
    match extract {
        "main" => {
            let sel = scraper::Selector::parse("main").unwrap();
            document
                .select(&sel)
                .next()
                .or_else(|| {
                    let sel = scraper::Selector::parse("body").unwrap();
                    document.select(&sel).next()
                })
                .unwrap_or(document.root_element())
        }
        "body" => {
            let sel = scraper::Selector::parse("body").unwrap();
            document
                .select(&sel)
                .next()
                .unwrap_or(document.root_element())
        }
        _ => document.root_element(),
    }
}

/// 不要要素を除外してHTMLを再構築
fn serialize_element_clean(element: ElementRef) -> String {
    const SKIP_TAGS: &[&str] = &[
        "script", "style", "noscript", "iframe", "svg", "nav", "header", "footer", "aside",
        "template", "code", "canvas", "audio", "video", "map", "object", "embed",
    ];

    let tag = element.value().name();
    if SKIP_TAGS.contains(&tag) {
        return String::new();
    }

    let mut result = String::new();
    result.push('<');
    result.push_str(tag);

    for (name, value) in element.value().attrs.iter() {
        result.push(' ');
        result.push_str(&name.local);
        result.push_str("=\"");
        result.push_str(value);
        result.push('"');
    }
    result.push('>');

    for child in element.children() {
        match child.value() {
            Node::Text(text) => result.push_str(text),
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    result.push_str(&serialize_element_clean(child_el));
                }
            }
            _ => {}
        }
    }

    result.push_str("</");
    result.push_str(tag);
    result.push('>');

    result
}

/// テキストを正規化（連続空白・改行を整理）
fn normalize_text(text: &str) -> String {
    let re_multispace = regex::Regex::new(r"[ \t]+").unwrap();
    let re_multiline = regex::Regex::new(r"\n[ \t]*\n[ \t\n]*").unwrap();
    let re_leading_space = regex::Regex::new(r"[ \t]*\n[ \t]*").unwrap();

    let step1 = re_multispace.replace_all(text, " ");
    let step2 = re_multiline.replace_all(&step1, "\n\n");
    let step3 = re_leading_space.replace_all(&step2, "\n");

    step3.trim().to_string()
}

/// 最大文字数で切り詰め
fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let truncated = &text[..max_chars];
        let end = truncated
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(max_chars);
        format!(
            "{}\n\n[... truncated, total {} chars ...]",
            &text[..end],
            text.len()
        )
    }
}

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "browser".to_string(),
        description: concat!(
            "Automates a Chromium browser via Chrome DevTools Protocol (CDP). ",
            "Supports navigation, clicking, text input, waiting for elements, taking screenshots, and extracting DOM content. ",
            "Browser state is persisted per session, so multiple actions (e.g., navigate -> wait -> get_content) can be performed sequentially. ",
            "Use the 'close' action to explicitly close the browser session.\n\n",
            "IMPORTANT: Always prefer 'navigate' over 'click' when accessing a page by URL. ",
            "Use 'click' ONLY when you must interact with page elements (buttons, links without visible URLs, form submissions) that cannot be reached via direct navigation. ",
            "Navigation is faster and more reliable than clicking.\n\n",
            "get_content output formats and data removal:\n",
            "- format='markdown' (default): Converts visible content to Markdown using html2md. ",
            "  REMOVED: All <script>, <style>, <noscript>, <iframe>, <svg>, <nav>, <header>, <footer>, <aside>, <template>, <code>, <canvas>, <audio>, <video>, <map>, <object>, <embed> elements and their contents. ",
            "  PRESERVED: Headings (h1-h6), paragraphs, lists, tables, links [text](url), images, blockquotes, and other structural content. ",
            "  Typically reduces token count to 5-15% of raw HTML.\n",
            "- format='html': Returns raw HTML. ",
            "  With trim=true (default): Same elements removed as above, returns cleaned HTML. ",
            "  With trim=false: Returns complete HTML including all scripts, styles, and hidden content. ",
            "  extract='full' with trim=false returns the entire document including <head>.\n\n",
            "extract scopes:\n",
            "- 'body' (default): Content within <body> tag only\n",
            "- 'main': Content within <main> tag (falls back to <body> if not found)\n",
            "- 'full': Entire document including <head> and <html> tags"
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "wait", "screenshot", "get_content", "scroll", "close"],
                    "description": "The browser action to perform"
                },
                "url": { "type": "string", "description": "URL for navigate action. Prefer navigate+URL over clicking links whenever possible." },
                "selector": { "type": "string", "description": "CSS selector for click, type, wait actions. Use only when navigate cannot reach the target." },
                "text": { "type": "string", "description": "Text to type into an element" },
                "wait_ms": { "type": "integer", "description": "Milliseconds to wait" },
                "scroll_x": { "type": "integer", "description": "Horizontal scroll amount" },
                "scroll_y": { "type": "integer", "description": "Vertical scroll amount" },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "html"],
                    "description": "Output format for get_content (default: 'markdown'). 'markdown' preserves headings/links/lists as Markdown, 'html' returns raw HTML. See full tool description for details on what is removed."
                },
                "extract": {
                    "type": "string",
                    "enum": ["body", "main", "full"],
                    "description": "Extraction scope for get_content (default: 'body'). 'body' = <body> content, 'main' = <main> content (falls back to <body>), 'full' = entire document including <head>."
                },
                "trim": {
                    "type": "boolean",
                    "description": "Remove non-content elements for get_content (default: true). When true, removes: script, style, noscript, iframe, svg, nav, header, footer, aside, template, code, canvas, audio, video, map, object, embed. Set false to keep all elements."
                }
            },
            "required": ["action"]
        }),
    }
}

/// セッション単位のブラウザ状態
pub struct BrowserSession {
    #[allow(dead_code)]
    pub browser: chromiumoxide::browser::Browser,
    pub page: chromiumoxide::page::Page,
    pub handler_task: tokio::task::JoinHandle<()>,
    pub user_data_dir: std::path::PathBuf,
}

/// セッション単位のブラウザセッション管理
#[derive(Default)]
pub struct BrowserSessionStore {
    sessions: DashMap<String, Arc<Mutex<BrowserSession>>>,
}

impl BrowserSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// セッションを取得または新規作成
    pub async fn get_or_create(
        &self,
        session_id: &str,
    ) -> Result<Arc<Mutex<BrowserSession>>, AiCoreError> {
        // 既存セッションがあれば返す
        if let Some(entry) = self.sessions.get(session_id) {
            return Ok(entry.clone());
        }

        // 新規ブラウザセッションを作成
        let chrome_path = find_chrome_executable().ok_or_else(|| {
            AiCoreError::BrowserError(
                "No Chrome/Chromium browser found. Please install Google Chrome or Chromium, "
                    .to_string()
                    + "or set PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH environment variable.",
            )
        })?;

        let user_data_dir =
            std::env::temp_dir().join(format!("ene-browser-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&user_data_dir).map_err(|e| {
            AiCoreError::BrowserError(format!("Failed to create user data dir: {e}"))
        })?;

        let config = chromiumoxide::browser::BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir)
            .no_sandbox()
            .build()
            .map_err(|e| {
                AiCoreError::BrowserError(format!("Failed to build browser config: {e}"))
            })?;

        let (browser, mut handler) = chromiumoxide::browser::Browser::launch(config)
            .await
            .map_err(|e| AiCoreError::BrowserError(format!("Failed to launch browser: {e}")))?;

        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| AiCoreError::BrowserError(format!("Failed to create page: {e}")))?;

        let session = BrowserSession {
            browser,
            page,
            handler_task,
            user_data_dir: user_data_dir.clone(),
        };

        let session_arc = Arc::new(Mutex::new(session));
        self.sessions
            .insert(session_id.to_string(), session_arc.clone());

        Ok(session_arc)
    }

    /// セッションをクローズ
    pub fn close(&self, session_id: &str) {
        if let Some((_, session_arc)) = self.sessions.remove(session_id) {
            tokio::spawn(async move {
                let session = session_arc.lock().await;
                session.handler_task.abort();
                let dir = session.user_data_dir.clone();
                drop(session);
                let _ = tokio::fs::remove_dir_all(&dir).await;
            });
        }
    }
}

/// インストールされている Chrome/Chromium 実行ファイルを自動検出する
fn find_chrome_executable() -> Option<std::path::PathBuf> {
    // 1. 環境変数を優先
    for env_var in &[
        "PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH",
        "CHROME_EXECUTABLE",
        "CHROMIUM_EXECUTABLE",
    ] {
        if let Ok(path) = std::env::var(env_var) {
            let p = std::path::PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // 2. PATH から検索
    let candidates = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave",
    ];

    if let Ok(path_env) = std::env::var("PATH") {
        for dir in path_env.split(if cfg!(windows) { ';' } else { ':' }) {
            for candidate in candidates {
                let path = std::path::Path::new(dir).join(candidate);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    // 3. よくある絶対パスを確認
    let common_paths = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/local/bin/google-chrome",
        "/usr/local/bin/chromium",
        "/home/pexisgle/.nix-profile/bin/google-chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    for path in common_paths {
        let p = std::path::PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

/// ブラウザ操作を実行（セッション状態を保持）
pub async fn browser_exec(
    store: &BrowserSessionStore,
    session_id: &str,
    action: &str,
    url: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    wait_ms: Option<u64>,
    scroll_x: Option<i32>,
    scroll_y: Option<i32>,
    format: Option<&str>,
    extract: Option<&str>,
    trim: Option<bool>,
) -> Result<String, AiCoreError> {
    let session = store.get_or_create(session_id).await?;
    let session_guard = session.lock().await;
    let page = &session_guard.page;

    let result = match action {
        "navigate" => {
            let url = url.ok_or_else(|| {
                AiCoreError::BrowserError("URL required for navigate".to_string())
            })?;
            page.goto(url)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Navigation failed: {e}")))?;

            let current_url = page
                .url()
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Failed to get URL: {e}")))?
                .unwrap_or_else(|| url.to_string());

            let title = page
                .evaluate("document.title")
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Failed to get title: {e}")))?
                .into_value::<String>()
                .unwrap_or_default();

            let ready_state = page
                .evaluate("document.readyState")
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Failed to get readyState: {e}")))?
                .into_value::<String>()
                .unwrap_or_default();

            format!(
                "Navigation successful\nURL: {}\nTitle: {}\nReady State: {}",
                current_url, title, ready_state
            )
        }
        "click" => {
            let selector = selector.ok_or_else(|| {
                AiCoreError::BrowserError("Selector required for click".to_string())
            })?;
            page.find_element(selector)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Element not found: {e}")))?
                .click()
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Click failed: {e}")))?;

            match tokio::time::timeout(
                tokio::time::Duration::from_secs(5),
                page.wait_for_navigation(),
            )
            .await
            {
                Ok(Ok(_)) => format!("Clicked element: {} (page loaded)", selector),
                Ok(Err(_)) => format!(
                    "Clicked element: {} (navigation error, page may still be loading)",
                    selector
                ),
                Err(_) => format!(
                    "Clicked element: {} (navigation timeout, page may still be loading)",
                    selector
                ),
            }
        }
        "type" => {
            let selector = selector.ok_or_else(|| {
                AiCoreError::BrowserError("Selector required for type".to_string())
            })?;
            let text = text
                .ok_or_else(|| AiCoreError::BrowserError("Text required for type".to_string()))?;
            page.find_element(selector)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Element not found: {e}")))?
                .type_str(text)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Type failed: {e}")))?;

            match tokio::time::timeout(
                tokio::time::Duration::from_secs(5),
                page.wait_for_navigation(),
            )
            .await
            {
                Ok(Ok(_)) => format!("Typed into element: {} (page loaded)", selector),
                Ok(Err(_)) => format!(
                    "Typed into element: {} (navigation error, page may still be loading)",
                    selector
                ),
                Err(_) => format!(
                    "Typed into element: {} (navigation timeout, page may still be loading)",
                    selector
                ),
            }
        }
        "wait" => {
            let ms = wait_ms.unwrap_or(1000);
            tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
            format!("Waited {} ms", ms)
        }
        "screenshot" => {
            let params = chromiumoxide::page::ScreenshotParams::default();
            let data = page
                .screenshot(params)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Screenshot failed: {e}")))?;
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
            let data_uri = format!("data:image/png;base64,{}", b64);
            return Ok(serde_json::json!({
                "type": "screenshot",
                "data": data_uri
            })
            .to_string());
        }
        "get_content" => {
            let format = format.unwrap_or("markdown");
            let extract = extract.unwrap_or("body");
            let trim = trim.unwrap_or(true);

            let html = page
                .content()
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Failed to get content: {e}")))?;

            let extracted = match format {
                "html" => extract_html(&html, extract, trim),
                _ => extract_markdown(&html, extract, trim),
            };
            truncate_text(&extracted, 15000)
        }
        "scroll" => {
            let x = scroll_x.unwrap_or(0);
            let y = scroll_y.unwrap_or(0);
            let js = format!("window.scrollBy({}, {});", x, y);
            page.evaluate(js)
                .await
                .map_err(|e| AiCoreError::BrowserError(format!("Scroll failed: {e}")))?;
            format!("Scrolled by ({}, {})", x, y)
        }
        "close" => {
            drop(session_guard);
            store.close(session_id);
            "Browser session closed.".to_string()
        }
        _ => {
            return Err(AiCoreError::BrowserError(format!(
                "Unknown browser action: {}",
                action
            )));
        }
    };

    Ok(result)
}
