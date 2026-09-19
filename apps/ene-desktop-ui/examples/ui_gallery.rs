//! Test-only visual fixture. No Host, credentials, provider, or production success.
use ene_desktop_ui::{ChatWindow, Item, ManagementWindow, Message};
use slint::{ComponentHandle, ModelRc, VecModel};
fn rows(items: Vec<Item>) -> ModelRc<Item> {
    ModelRc::new(VecModel::from(items))
}
fn row(key: &str, title: &str, body: &str, meta: &str, state: &str) -> Item {
    Item {
        key: key.into(),
        title: title.into(),
        body: body.into(),
        meta: meta.into(),
        state: state.into(),
    }
}
fn main() -> Result<(), slint::PlatformError> {
    let chat = ChatWindow::new()?;
    let management = ManagementWindow::new()?;
    let en = std::env::args().any(|a| a == "--en");
    let small = std::env::args().any(|a| a == "--small");
    chat.set_japanese(!en);
    management.set_japanese(!en);
    chat.set_connected(true);
    management.set_connected(true);
    chat.set_status("UI fixture · connected".into());
    management.set_status("UI fixture · no Host".into());
    chat.set_messages(ModelRc::new(VecModel::from(vec![
        Message { owner:true,text:"週末のプロジェクトについて、一緒に整理してもらえますか？".into(),caption:"10:24".into() },
        Message { owner:false,text:"もちろん。まず、いちばん大切にしたいことから考えましょう。

今週進めた内容を整理し、次に取り組むことを一緒に決められます。気になっている点があれば教えてください。".into(),caption:"10:24".into() },
        Message { owner:true,text:"画面を開いたまま、資料の整理も続けたいです。".into(),caption:"10:25".into() },
        Message { owner:false,text:"作業の詳細は右側から確認できます。ここでは、そのまま会話を続けられます。".into(),caption:"10:25".into() },
    ])));
    chat.set_tasks(rows(vec![
        row(
            "task-a",
            "プロジェクト資料の整理",
            "必要な情報を集め、進め方をまとめる",
            "",
            "進行中",
        ),
        row(
            "task-b",
            "週末の計画",
            "無理のないスケジュールを考える",
            "",
            "保留中",
        ),
    ]));
    chat.on_select_task({
        let weak = chat.as_weak();
        move |key| {
            if let Some(c) = weak.upgrade() {
                c.set_selected_task(key.clone());
                c.set_has_task(true);
                c.set_details(rows(vec![
                    row(
                        "",
                        "目的",
                        if key == "task-b" {
                            "週末の時間を確保する"
                        } else {
                            "資料を読みやすく整理する"
                        },
                        "",
                        "",
                    ),
                    row("", "結果", "まだ結果はありません", "", ""),
                ]));
            }
        }
    });
    chat.set_progress_summary(
        if en {
            "2 tasks · Open details"
        } else {
            "2 件の作業 · 詳細を開く"
        }
        .into(),
    );
    management.set_memories(rows(vec![
        row("memory-a","作業の進め方","一度に多くを進めるより、ひとつずつ確実に。","2026-09-19",""),
        row("memory-b","Preferred working style","A deliberately long English sentence to verify wrapping without clipping at the smallest supported window size.","2026-09-18",""),
    ]));
    management.on_select_memory({
        let weak = management.as_weak();
        move |key| {
            if let Some(m) = weak.upgrade() {
                m.set_selected_memory(key);
                m.set_revisions(rows(vec![row(
                    "",
                    "変更履歴",
                    "会話で確認した作業の進め方。",
                    "2026-09-19",
                    "",
                )]));
            }
        }
    });
    management.set_usage(rows(vec![
        row(
            "",
            "OpenAI / conversation-model",
            "USD 0.012400",
            "240 / 120",
            "確定 / Reported",
        ),
        row(
            "",
            "OpenAI / long-model-name-for-wrapping",
            "不明 / Unknown",
            "不明 / Unknown",
            "結果不明 / Unknown",
        ),
        row(
            "",
            "OpenAI / conversation-model",
            "未確定 / Unsettled",
            "—",
            "予約中 / Reserved",
        ),
    ]));
    management.set_caps(rows(vec![row(
        "",
        "全体 · 日次 (UTC)",
        "上限 USD 1.00 · 残り USD 0.98",
        "",
        "",
    )]));
    management.set_deletions(rows(vec![row(
        "operation-a",
        "データ削除",
        "処理先からの報告を待っています。",
        "2026-09-19",
        "保留中 / Held",
    )]));
    management.on_select_deletion({
        let weak = management.as_weak();
        move |key| {
            if let Some(m) = weak.upgrade() {
                m.set_selected_deletion(key);
            }
        }
    });
    management.on_navigate({
        let weak = management.as_weak();
        move |page| {
            if let Some(m) = weak.upgrade() {
                m.invoke_clear_secret();
                m.set_page(page);
            }
        }
    });
    management.on_wizard_next({
        let weak = management.as_weak();
        move || {
            if let Some(m) = weak.upgrade() {
                m.set_step((m.get_step() + 1).min(4));
            }
        }
    });
    management.on_wizard_back({
        let weak = management.as_weak();
        move || {
            if let Some(m) = weak.upgrade() {
                m.set_step((m.get_step() - 1).max(0));
            }
        }
    });
    management.on_language({
        let cw = chat.as_weak();
        let mw = management.as_weak();
        move |ja| {
            if let Some(c) = cw.upgrade() {
                c.set_japanese(ja);
            }
            if let Some(m) = mw.upgrade() {
                m.set_japanese(ja);
            }
        }
    });
    chat.on_open_management({
        let weak = management.as_weak();
        move || {
            if let Some(m) = weak.upgrade() {
                match m.show() {
                    Ok(_) | Err(_) => {}
                }
            }
        }
    });
    management.on_open_chat({
        let weak = chat.as_weak();
        move || {
            if let Some(c) = weak.upgrade() {
                match c.show() {
                    Ok(_) | Err(_) => {}
                }
            }
        }
    });
    if small {
        chat.window().set_size(slint::LogicalSize::new(720., 540.));
        management
            .window()
            .set_size(slint::LogicalSize::new(720., 540.));
    }
    chat.show()?;
    management.show()?;
    slint::run_event_loop()
}
