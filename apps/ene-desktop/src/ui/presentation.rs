//! Typed, body-bearing projections. Opaque keys are action premises, never labels.
use crate::i18n::Locale;
use std::{future::Future, pin::Pin, sync::Arc};

pub type SurfaceErasure = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

#[derive(Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub key: String,
    pub title: String,
    pub body: String,
    pub meta: String,
    pub state: String,
}
impl core::fmt::Debug for Row {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Row")
            .field("body", &"[redacted]")
            .finish_non_exhaustive()
    }
}
impl Row {
    pub(crate) fn text(title: &str, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            ..Self::default()
        }
    }
}
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Message {
    pub round: String,
    pub owner: bool,
    pub text: String,
    pub caption: String,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub japanese: bool,
    pub connected: bool,
    pub ready: bool,
    pub credential: bool,
    pub consent: bool,
    pub step: i32,
    pub model: String,
    pub status: String,
    pub body_available: bool,
    pub body_visible: bool,
    pub messages: Vec<Message>,
    pub tasks: Vec<Row>,
    pub details: Vec<Row>,
    pub selected_task: String,
    pub memories: Vec<Row>,
    pub revisions: Vec<Row>,
    pub selected_memory: String,
    pub memory_more: bool,
    pub revisions_more: bool,
    pub usage: Vec<Row>,
    pub caps: Vec<Row>,
    pub usage_more: bool,
    pub deletions: Vec<Row>,
    pub selected_deletion: String,
    pub can_resume_deletion: bool,
    pub confirmation: Option<Confirmation>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub key: String,
    pub title: String,
    pub description: String,
    pub target: String,
}
pub(crate) fn tr(locale: Locale, ja: &str, en: &str) -> String {
    if locale == Locale::Ja { ja } else { en }.into()
}
pub(crate) fn state(locale: Locale, value: &str) -> String {
    let (ja, en) = match value {
        "unknown" => ("不明・結果を確認できません", "Unknown · result unavailable"),
        "running" | "active" | "in_progress" => ("進行中", "In progress"),
        "completed" | "done" => ("完了", "Completed"),
        "held" => ("保留中・確認が必要", "On hold · needs attention"),
        "finalizing" => ("最終処理中", "Finalizing"),
        "reported" => ("確定", "Reported"),
        "reserved" => ("予約中・未確定", "Reserved · unsettled"),
        "interrupted" => ("中断", "Interrupted"),
        "cancelled" | "canceled" => ("中止", "Cancelled"),
        "failed" => ("失敗", "Failed"),
        "pending" => ("待機中", "Pending"),
        _ => ("状態を確認してください", "Review status"),
    };
    tr(locale, ja, en)
}
pub(crate) fn task_key(task: &str, revision: u64, purpose: &str) -> String {
    serde_json::to_string(&(task, revision, purpose)).unwrap_or_default()
}
pub(crate) fn memory_key(id: &str, revision: &str) -> String {
    serde_json::to_string(&(id, revision)).unwrap_or_default()
}
pub fn parse_cap(value: &str) -> Option<u64> {
    let value = value.trim();
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let amount = whole.parse::<u64>().ok()?.checked_mul(1_000_000)?;
    let part = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().ok()?
    };
    amount.checked_add(part.checked_mul(10_u64.pow(6 - u32::try_from(fraction.len()).ok()?))?)
}
