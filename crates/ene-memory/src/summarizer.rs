//! LLM を使った会話要約生成モジュール
//!
//! セッション分割時に呼び出され、会話履歴を構造化された要約に変換する。

use async_openai::types::chat::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs, ResponseFormat, ResponseFormatJsonSchema, Role,
};
use serde::{Deserialize, Serialize};

use ene_embedding::client::build_openai_client;
use ene_config::AiSettings;
use crate::error::MemoryError;
use crate::store::KeyFact;

/// LLM が返す構造化された会話要約
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummaryResult {
    /// 自然言語の会話要約
    pub summary: String,
    /// 抽出されたトピックキーワード
    pub topics: Vec<String>,
    /// ユーザーに関する重要な事実（キーバリュー型）
    pub key_facts: Vec<KeyFact>,
}

/// 会話履歴から要約を生成する
pub async fn summarize_conversation(
    settings: &AiSettings,
    history: &[(Role, String)],
    character_name: &str,
    user_name: &str,
    existing_facts: &[KeyFact],
) -> Result<ConversationSummaryResult, MemoryError> {
    if history.is_empty() {
        return Ok(ConversationSummaryResult {
            summary: String::new(),
            topics: vec![],
            key_facts: vec![],
        });
    }

    let base_url = settings.resolve_summarization_base_url()?;
    let api_key = settings.resolve_api_key();
    let model = settings.resolve_summarization_model();

    let client = build_openai_client(&base_url, &api_key);

    let mut conversation_text = String::new();
    for (role, content) in history {
        let label = match role {
            Role::User => user_name,
            Role::Assistant => character_name,
            _ => "System",
        };
        conversation_text.push_str(&format!("{}: {}\n", label, content));
    }

    let existing_facts_str = if existing_facts.is_empty() {
        "なし".to_string()
    } else {
        existing_facts
            .iter()
            .map(|f| format!("- {}: {}", f.key, f.value))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system_prompt = format!(
        r#"あなたは会話分析の専門家です。与えられた会話履歴を分析し、重要な情報のみを抽出して JSON 形式で出力してください。

## 出力形式（純粋な JSON のみを返してください）
{{
  "summary": "会話の重要な出来事・結論・決定事項のみを簡潔に要約",
  "topics": ["トピック1", "トピック2"],
  "key_facts": [
    {{ "key": "job", "value": "エンジニア" }}
  ]
}}

## summary の書き方
- 出来事を時系列に羅列しないでください
- 会話から「重要な結論」「決定事項」「新しい発見」「感情 of 重要な変化」のみを抽出してください
- 具体的な固有名詞・日付・数値は必ず含めてください
- 雑談や挨拶、繰り返し確認などの定型文は省略してください
- 2〜4文で、第三者が読んでも何が重要な会話だったか分かるように書いてください
- 例（悪い）: 「{user_name}は挨拶をした。そして天気の話になった。{user_name}は明日雨が降ると言った。」
- 例（良い）: 「{user_name}は明日の外出予定を共有した。午後2時に渋谷で友人と待ち合わせ、映画『オッペンハイマー』を観る計画。」

## topics の書き方
- 会話で扱われた主なトピックを1〜5個の具体的なキーワードで列挙
- 例: ×「雑談」→ ○「週末の予定」「映画『オッペンハイマー』」

## key_facts の書き方（重要）
ユーザー（{user_name}）について判明した・更新された情報をキーバリュー形式で列挙します。

### 既存の事実リストとの比較ルール
以下の「既存の事実リスト」と比較して、以下のいずれかの操作を行ってください:

1. **更新**: 既存の key と同じ key で新しい値がある場合 → 新しい値で上書き
   - 例: 既存 `job: "エンジニア"` → 会話で「転職してデザイナーになった」→ `job: "デザイナー"`

2. **削除**: 既存の key の情報が会話で「もう該当しない」「辞めた」「終わった」と言及された場合 → value に空文字 `""` を設定
   - 例: 既存 `job: "エンジニア"` → 会話で「来月で退職する」→ `job: ""`
   - 空文字 `""` が設定された key_facts エントリは、保存時に削除されます

3. **過去情報として保持**: 削除するが、過去の文脈として残しておいた方が良い重要な情報の場合 → key に `previous_` プレフィックスを付けて保持
   - 例: 既存 `job: "エンジニア"` → 会話で「来月デザイナーに転職する」→ `job: "デザイナー"`, `previous_job: "エンジニア"`
   - 例: 既存 `hobby: "ギター"` → 会話で「ギターはもう辞めてピアノに集中している」→ `hobby: "ピアノ"`, `previous_hobby: "ギター"`

4. **新規追加**: 既存のリストにない新しい事実があれば追加

### 注意事項
- value は短く端的に（例: ×「Userはエンジニアとして働いている」→ ○「エンジニア」）
- 会話で言及されなかった既存の事実は、そのままの値で含めてください（維持）
- キャラクター（{character_name}）についての情報は含めない
- 該当する事実がなければ空配列 `[]` を返す

## 注意
- JSON 以外のテキストは絶対に出力しないでください"#
    );

    let user_prompt = format!(
        "【既存の事実リスト】\n{}\n\n以下の会話履歴を分析して、指定された JSON 形式で要約と最新の事実リストを作成してください。\n\n---\n{}\n---",
        existing_facts_str, conversation_text
    );

    let sys_msg = ChatCompletionRequestSystemMessageArgs::default()
        .content(system_prompt)
        .build()
        .map_err(|e| MemoryError::PromptBuildError(format!("system message: {e}")))?;

    let user_msg = ChatCompletionRequestUserMessageArgs::default()
        .content(user_prompt)
        .build()
        .map_err(|e| MemoryError::PromptBuildError(format!("user message: {e}")))?;

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string", "description": "会話の重要な出来事・結論・決定事項のみの要約" },
            "topics": { "type": "array", "items": { "type": "string" }, "description": "トピックのリスト" },
            "key_facts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "事実のキー。既存のkeyと同じなら更新、削除する場合は空文字\"\"をvalueに設定、過去情報として残す場合はprevious_プレフィックスを付けた新しいkeyを使用" },
                        "value": { "type": "string", "description": "事実の値。空文字\"\"の場合はその事実の削除を意味する" }
                    },
                    "required": ["key", "value"],
                    "additionalProperties": false
                },
                "description": "ユーザーに関する重要な事実のリスト（既存事実との比較結果）"
            }
        },
        "required": ["summary", "topics", "key_facts"],
        "additionalProperties": false
    });

    let request = CreateChatCompletionRequestArgs::default()
        .model(model)
        .messages(vec![sys_msg.into(), user_msg.into()])
        .response_format(ResponseFormat::JsonSchema {
            json_schema: ResponseFormatJsonSchema {
                description: Some("Conversation summary result".to_string()),
                name: "ConversationSummaryResult".to_string(),
                schema: Some(schema),
                strict: Some(true),
            },
        })
        .build()
        .map_err(|e| MemoryError::PromptBuildError(format!("request: {e}")))?;

    let response = client
        .chat()
        .create(request)
        .await
        .map_err(|e| MemoryError::ApiRequestError(format!("summarization: {e}")))?;

    let content = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();

    parse_summary_json(&content)
}

/// LLM のレスポンスから JSON を抽出してパースする
fn parse_summary_json(raw: &str) -> Result<ConversationSummaryResult, MemoryError> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    if let Ok(result) = serde_json::from_str::<ConversationSummaryResult>(cleaned) {
        return Ok(result);
    }

    if let Some(start) = cleaned.find('{') {
        if let Some(end) = cleaned.rfind('}') {
            let json_str = &cleaned[start..=end];
            if let Ok(result) = serde_json::from_str::<ConversationSummaryResult>(json_str) {
                return Ok(result);
            }
        }
    }

    tracing::warn!("[Summarizer] Failed to parse JSON response, using raw text as summary");
    Ok(ConversationSummaryResult {
        summary: raw.to_string(),
        topics: vec![],
        key_facts: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_summary_json() {
        let json = r#"{"summary": "テスト要約", "topics": ["topic1"], "key_facts": [{"key": "job", "value": "fact1"}]}"#;
        let result = parse_summary_json(json).unwrap();
        assert_eq!(result.summary, "テスト要約");
        assert_eq!(result.topics, vec!["topic1"]);
        assert_eq!(result.key_facts.len(), 1);
        assert_eq!(result.key_facts[0].key, "job");
        assert_eq!(result.key_facts[0].value, "fact1");
    }

    #[test]
    fn test_parse_summary_json_with_code_block() {
        let json = "```json\n{\"summary\": \"test\", \"topics\": [], \"key_facts\": []}\n```";
        let result = parse_summary_json(json).unwrap();
        assert_eq!(result.summary, "test");
    }

    #[test]
    fn test_parse_summary_json_fallback() {
        let raw = "This is not valid JSON at all";
        let result = parse_summary_json(raw).unwrap();
        assert_eq!(result.summary, raw);
        assert!(result.topics.is_empty());
    }
}
