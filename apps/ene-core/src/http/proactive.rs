use std::time::{Duration, Instant};

use chrono::DateTime;
use ene_companion::{
    ActivitySnapshot, ProactiveObservation, ProactiveSuppressionState, ScreenSummaryStatus,
    build_proactive_context, decide_proactive_speech, evaluate_quiet_hours,
};
use ene_session::{EventKind, Role};

use serde_json::json;

use super::AppState;
use super::classify::SeamedClassify;

/// Event-driven companion speech. Fail-closed when the classifier is Echo.
pub async fn run_loop(state: AppState, classify: std::sync::Arc<SeamedClassify>) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tick(&state, classify.as_ref()).await;
    }
}

async fn tick(state: &AppState, classify: &SeamedClassify) {
    let mind = state.core.mind();
    if !mind.proactive.enabled || mind.proactive.paused {
        return;
    }
    if state.lanes.any_busy(&state.core) {
        return;
    }
    let Ok(sessions) = state.core.store().list_sessions(None) else {
        return;
    };
    let Some(meta) = sessions.iter().find(|row| row.ended_at.is_none()) else {
        return;
    };
    let Ok(events) = state.core.store().load_events(meta.id, 0) else {
        return;
    };
    let now = chrono::Utc::now();
    let last_user = events
        .iter()
        .rev()
        .find(|event| matches!(event.kind, EventKind::UserMessage));
    let seconds_since_user = last_user
        .and_then(|event| DateTime::parse_from_rfc3339(&event.ts).ok())
        .map_or(u64::MAX, |ts| {
            u64::try_from(
                now.signed_duration_since(ts.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(0),
            )
            .unwrap_or(0)
        });
    let last_proactive = state.core.last_proactive(meta.id);
    let seconds_since_proactive = last_proactive.map_or(u64::MAX, |then| {
        Instant::now().saturating_duration_since(then).as_secs()
    });
    let proactive_turns = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TurnStart))
        .filter(|event| match &event.payload {
            ene_session::EventPayload::TurnStart { origin, .. } => {
                *origin == ene_session::TurnOrigin::Proactive
            }
            _ => false,
        })
        .count();
    let history = ene_session::derive_messages(
        &events,
        ene_session::ProjectOptions::for_depth(ene_kernel::DisplayDepth::Surface, 8),
    )
    .messages
    .into_iter()
    .filter(|message| message.role == Role::User || message.role == Role::Assistant)
    .map(|message| message.text())
    .collect::<Vec<_>>();
    let suppression = ProactiveSuppressionState {
        seconds_since_user_input: seconds_since_user,
        seconds_since_proactive,
        proactive_turns_this_session: proactive_turns,
        user_turn_busy: false,
    };
    let registry = state.core.supervisor().registry();
    let mut window_label = String::new();
    let activity = if mind.proactive.sources.activity {
        if let Ok(value) = registry.execute_host("app.active_window", json!({})).await {
            value
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .clone_into(&mut window_label);
        }
        Some(ActivitySnapshot {
            idle_seconds: Some(seconds_since_user),
            active_window_label: window_label.clone(),
            recent_change: String::new(),
        })
    } else {
        None
    };
    let (screen_summary, screen_summary_status) = if mind.proactive.sources.screen_summary {
        match capture_screen_summary(registry.as_ref(), classify, &window_label).await {
            Some(summary) => (Some(summary), ScreenSummaryStatus::Available),
            None => (None, ScreenSummaryStatus::Unavailable),
        }
    } else {
        (None, ScreenSummaryStatus::Disabled)
    };
    let observation = ProactiveObservation {
        captured_at_unix_ms: u64::try_from(now.timestamp_millis()).unwrap_or(0),
        activity,
        screen_summary,
        screen_summary_status,
    };
    let ctx = build_proactive_context(
        &mind.proactive,
        &history,
        &observation,
        None,
        None,
        &[],
        &[],
        suppression,
        evaluate_quiet_hours(&mind.proactive.quiet_hours, now),
        None,
        None,
    );
    let outcome = decide_proactive_speech(&mind.proactive, &ctx, Some(classify)).await;
    if outcome.skip.is_some() || !outcome.decision.should_speak {
        return;
    }
    let hint = if outcome.decision.topic_hint.is_empty() {
        "Speak if you have something worth saying. Stay brief.".to_owned()
    } else {
        format!(
            "Speak proactively about: {}. Stay brief.",
            outcome.decision.topic_hint
        )
    };
    let Ok(lane) = state.lanes.get_or_open(&state.core, meta.id) else {
        return;
    };
    match lane.proactive(hint).await {
        Ok(_) => state.core.mark_proactive(meta.id),
        Err(err) => tracing::debug!(error = %err, "proactive turn skipped"),
    }
}

async fn capture_screen_summary(
    registry: &ene_registry::ToolRegistry,
    classify: &SeamedClassify,
    window_label: &str,
) -> Option<String> {
    let value = registry
        .execute_host("app.screenshot", json!({}))
        .await
        .ok()?;
    let encoded = value
        .get("png_base64")
        .and_then(serde_json::Value::as_str)?;
    let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).ok()?;
    tracing::debug!(bytes = png.len(), "proactive screenshot captured");
    classify.summarize_screen(&png, window_label).await.ok()
}
