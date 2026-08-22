use crate::affect::{
    AffectBaseline, AffectPresentation, AffectProposal, AffectState, ExpressionArbiter,
    apply_self_report, apply_turn_signals, parse_affect_json, project_decay,
};
use crate::classify::ScriptedClassify;
use crate::config::{AffectSettings, MindSettings, ProactiveSettings};
use crate::ids::CandidateId;
use crate::inner::{model_visible_for, split_surface_and_inner};
use crate::memory::{
    ArbitrateOutcome, MemoryCandidate, MemoryKind, MemoryScope, MemorySource, NewMemory, arbitrate,
    extract_turn,
};
use crate::package::{
    avatar_path_for_install, compose_soul_and_body, content_digest, export_dir, import_v3,
    install_archive, localized_display_name, looks_like_package_zip, looks_like_zip, pack_archive,
    soul_from_install,
};
use crate::proactive::{
    ActivitySnapshot, GateRejectReason, ProactiveConfirmation, ProactiveObservation,
    ProactiveSkipReason, ProactiveSuppressionState, SILENT_TOKEN, ScreenSummaryStatus,
    WorldStateMemory, WorldStateSnapshot, build_proactive_context, classify_confirmation_prefix,
    decide_proactive_speech, evaluate_deterministic_gates,
};
use crate::runtime::CompanionRuntime;
use crate::soul::NewSoul;
use crate::store::CompanionStore;
use crate::tools::{QueryEmbed, register_memory_tools, surface_hides_write_shared};
use crate::{CompanionError, EmotionReport, VOCABULARY};
use chrono::{Duration, Utc};
use ene_registry::{Layer, ToolRegistry};
use ene_session::{
    DisplayDepth, InnerAspect, ProjectOptions, derive_messages, surface_leaks_inner,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;

fn open_store() -> (TempDir, CompanionStore) {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    (dir, store)
}

#[test]
fn memory_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("companions.db");
    let soul;
    {
        let store = CompanionStore::open(&path).unwrap();
        soul = store
            .create_soul(&NewSoul::text_only("char.ene@1"))
            .unwrap();
        store
            .insert_memory(NewMemory {
                soul_id: soul.id,
                scope: MemoryScope::Private,
                kind: MemoryKind::Episodic,
                title: "trip".into(),
                content: "planned a trip together".into(),
                confidence: 0.9,
                salience: 0.8,
                source: MemorySource::Extraction,
                source_seq: None,
                expires_at: None,
            })
            .unwrap();
    }
    let store = CompanionStore::open(&path).unwrap();
    let hits = store
        .recall(
            soul.id,
            "trip",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "trip");
    assert!(hits[0].as_own_knowledge().contains("planned a trip"));
}

fn insert_episodic(
    store: &CompanionStore,
    soul_id: ene_session::SoulId,
    title: &str,
    content: &str,
) -> crate::MemoryRecord {
    store
        .insert_memory(NewMemory {
            soul_id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: title.into(),
            content: content.into(),
            confidence: 0.9,
            salience: 0.5,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap()
}

#[test]
fn recall_without_query_vector_stays_lexical() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let near = insert_episodic(&store, soul.id, "apple pie", "baked dessert");
    let far = insert_episodic(&store, soul.id, "zebra stripes", "black and white");
    store.set_embedding(near.id, &[1.0, 0.0]).unwrap();
    store.set_embedding(far.id, &[0.0, 1.0]).unwrap();
    let now = Utc::now().to_rfc3339();
    let hits = store
        .recall_ranked(
            soul.id,
            "xyzzy",
            8,
            &now,
            crate::store::RecallWeights::default(),
            None,
            false,
        )
        .unwrap();
    assert!(
        hits.is_empty(),
        "unconfigured embedding must not surface unrelated rows: {hits:?}"
    );
    let lexical = store
        .recall(
            soul.id,
            "zebra",
            8,
            &now,
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert_eq!(lexical.len(), 1);
    assert_eq!(lexical[0].title, "zebra stripes");
}

#[test]
fn recall_with_query_vector_ranks_embedded_neighbor() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let near = insert_episodic(&store, soul.id, "apple pie", "baked dessert");
    let far = insert_episodic(&store, soul.id, "zebra stripes", "black and white");
    store.set_embedding(near.id, &[1.0, 0.0]).unwrap();
    store.set_embedding(far.id, &[0.0, 1.0]).unwrap();
    let hits = store
        .recall_ranked(
            soul.id,
            "xyzzy",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
            Some(&[1.0, 0.0]),
            false,
        )
        .unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].title, "apple pie");
    assert!(hits[0].score > 0.3);
}

#[test]
fn runtime_hybrid_recall_matches_store_when_vector_present() {
    let (_dir, store) = open_store();
    let store = Arc::new(store);
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let near = insert_episodic(store.as_ref(), soul.id, "apple pie", "baked dessert");
    store.set_embedding(near.id, &[1.0, 0.0]).unwrap();
    let runtime = CompanionRuntime::new(Arc::clone(&store), MindSettings::default());
    let empty = runtime.recall(soul.id, "xyzzy").unwrap();
    assert!(empty.is_empty());
    let hits = runtime
        .recall_ranked(soul.id, "xyzzy", Some(&[1.0, 0.0]))
        .unwrap();
    assert_eq!(hits[0].title, "apple pie");
}

#[test]
fn shared_pool_is_usable_by_another_soul_as_own_knowledge() {
    let (_dir, store) = open_store();
    let a = store.create_soul(&NewSoul::text_only("char.a@1")).unwrap();
    let b = store.create_soul(&NewSoul::text_only("char.b@1")).unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: a.id,
            scope: MemoryScope::Shared,
            kind: MemoryKind::UserProfile,
            title: "user name".into(),
            content: "the user's name is Tama's human".into(),
            confidence: 0.95,
            salience: 0.9,
            source: MemorySource::Shared,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: a.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "movie night".into(),
            content: "we cried at the movie".into(),
            confidence: 0.9,
            salience: 0.7,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    let hits = store
        .recall(
            b.id,
            "name",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].as_own_knowledge().contains(&a.id.to_string()));
    let private = store
        .recall(
            b.id,
            "movie",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert!(private.is_empty());
}

#[test]
fn open_commitments_omit_expired_rows() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: soul.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Commitment,
            title: "open".into(),
            content: "still due".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::UserStated,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: soul.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Commitment,
            title: "stale".into(),
            content: "already passed".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::UserStated,
            source_seq: None,
            expires_at: Some("2000-01-01T00:00:00Z".into()),
        })
        .unwrap();
    let notes = store.open_commitments(soul.id, 8).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("still due"));
    assert!(!notes.iter().any(|note| note.contains("already passed")));
}

#[test]
fn affect_decays_toward_baseline_but_trust_accumulates() {
    let mut state = AffectState::baseline(&AffectBaseline::default());
    state.valence = 0.9;
    state.trust = 0.5;
    let t0 = Utc::now();
    state.last_report_ts = t0.to_rfc3339();
    project_decay(
        &mut state,
        &AffectBaseline::default(),
        &AffectSettings::default(),
        t0 + Duration::hours(6),
    );
    assert!(state.valence < 0.9);
    assert!(state.valence > AffectBaseline::default().valence);
    assert!((state.trust - 0.5).abs() < f32::EPSILON);
}

#[test]
fn conversation_turns_raise_fatigue() {
    let mut state = AffectState::default();
    assert!(state.fatigue.abs() < f32::EPSILON);
    apply_turn_signals(&mut state, "hello", None, &AffectSettings::default());
    let after_short = state.fatigue;
    assert!(after_short > 0.0);
    apply_turn_signals(
        &mut state,
        &"word ".repeat(80),
        None,
        &AffectSettings::default(),
    );
    assert!(state.fatigue > after_short);
    let after_long = state.fatigue;
    apply_turn_signals(&mut state, "   ", None, &AffectSettings::default());
    assert!((state.fatigue - after_long).abs() < f32::EPSILON);
}

#[test]
fn self_report_updates_mood_label() {
    let mut state = AffectState::default();
    let out = apply_self_report(
        &mut state,
        &EmotionReport {
            label: "overjoyed".into(),
            intensity: 0.9,
        },
        Utc::now(),
    );
    assert!(
        VOCABULARY.contains(&out.label.as_str()) || out.label == "happy" || out.label == "joyful"
    );
    assert_eq!(state.mood_label, out.label);
}

#[tokio::test]
async fn extract_names_as_shared_and_arbitrates() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let cands = extract_turn(soul.id, "My name is Alice", "nice to meet you", None).await;
    assert!(
        cands
            .iter()
            .any(|c| c.kind == MemoryKind::UserProfile && c.scope == MemoryScope::Shared)
    );
    let approval = MindSettings::default().memory_approval;
    let mut saw_insert = false;
    for cand in cands {
        match arbitrate(&store, &cand, &approval).unwrap() {
            ArbitrateOutcome::Inserted(row) | ArbitrateOutcome::Updated(row)
                if row.scope == MemoryScope::Shared =>
            {
                saw_insert = true;
            }
            _ => {}
        }
    }
    assert!(saw_insert);
}

#[test]
fn forget_request_removes_matching_memory_and_records_journal() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: soul.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "trip".into(),
            content: "planned a trip together".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    let journal_before = store.journal_len().unwrap();
    let n = crate::memory::apply_forget_request(
        &store,
        soul.id,
        "forget trip",
        crate::config::ForgettingMode::Immediate,
    )
    .unwrap();
    assert_eq!(n, 1);
    assert!(store.journal_len().unwrap() > journal_before);
    let hits = store
        .recall(
            soul.id,
            "trip",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert!(hits.is_empty());
}

#[test]
fn forget_confirm_queues_candidate_without_deleting() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: soul.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "trip".into(),
            content: "planned a trip together".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    let n = crate::memory::apply_forget_request(
        &store,
        soul.id,
        "forget trip",
        crate::config::ForgettingMode::Confirm,
    )
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(store.list_pending_candidates(soul.id).unwrap().len(), 1);
    let hits = store
        .recall(
            soul.id,
            "trip",
            8,
            &Utc::now().to_rfc3339(),
            crate::store::RecallWeights::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn sensitive_candidate_queues_for_approval() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let cand = MemoryCandidate {
        id: CandidateId::new(),
        soul_id: soul.id,
        kind: MemoryKind::Episodic,
        title: "credentials".into(),
        content: "password is hunter2".into(),
        scope: MemoryScope::Private,
        confidence: 0.95,
        salience: 0.9,
        sensitive: true,
    };
    let approval = MindSettings::default().memory_approval;
    let outcome = arbitrate(&store, &cand, &approval).unwrap();
    assert!(matches!(outcome, ArbitrateOutcome::Queued(_)));
    let pending = store.list_pending_candidates(soul.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].sensitive);
}

#[test]
fn decay_surfaces_low_salience_forgetting_candidates() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    store
        .insert_memory(NewMemory {
            soul_id: soul.id,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "old note".into(),
            content: "fading".into(),
            confidence: 0.8,
            salience: 0.2,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    assert!(store.forgetting_candidates(0.15).unwrap().is_empty());
    store.decay_salience(MemoryKind::Episodic, 0.4).unwrap();
    let candidates = store.forgetting_candidates(0.15).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].title, "old note");
}

#[test]
fn expression_arbiter_suppresses_rapid_label_changes() {
    let mut arbiter = ExpressionArbiter::default();
    let settings = AffectSettings::default();
    let now = Utc::now();
    let first = arbiter.decide(
        AffectPresentation {
            label: "happy".into(),
            intensity: 0.8,
        },
        &settings,
        now,
        true,
    );
    assert!(first.is_some());
    let second = arbiter.decide(
        AffectPresentation {
            label: "sad".into(),
            intensity: 0.7,
        },
        &settings,
        now + Duration::milliseconds(200),
        true,
    );
    assert!(second.is_none());
}

#[tokio::test]
async fn classifier_scope_defaults_private_when_missing() {
    let soul = ene_session::SoulId::new();
    let classify = ScriptedClassify::new([
        r#"{"candidates":[{"kind":"episodic","title":"walk","content":"we walked in the rain","confidence":0.8}]}"#,
    ]);
    let cands = extract_turn(soul, "hi", "hello", Some(&classify)).await;
    assert_eq!(cands[0].scope, MemoryScope::Private);
}

#[tokio::test]
async fn classifier_json_can_mark_shared_scope() {
    let soul = ene_session::SoulId::new();
    let classify = ScriptedClassify::new([
        r#"{"candidates":[{"kind":"semantic","title":"i work nights","content":"i work nights","scope":"shared","confidence":0.9}]}"#,
    ]);
    let cands = extract_turn(soul, "remember that I work nights", "ok", Some(&classify)).await;
    assert!(
        cands
            .iter()
            .any(|cand| cand.kind == MemoryKind::Semantic && cand.scope == MemoryScope::Shared)
    );
}

#[tokio::test]
async fn classifier_failure_keeps_deterministic_extract() {
    let soul = ene_session::SoulId::new();
    let cands = extract_turn(
        soul,
        "My name is Bob",
        "hi",
        Some(&ScriptedClassify::silent()),
    )
    .await;
    assert!(
        cands
            .iter()
            .any(|cand| cand.kind == MemoryKind::UserProfile && cand.content.contains("bob"))
    );
}

#[test]
fn classifier_affect_proposal_blends_when_confident() {
    let mut state = AffectState::default();
    let before = state.valence;
    let proposal = parse_affect_json(
        r#"```json
{"valence":0.9,"arousal":0.4,"irritation":0.0,"affinity":0.5,"confidence":0.95}
```"#,
    )
    .expect("proposal");
    apply_turn_signals(
        &mut state,
        "the weather is fine",
        Some(&proposal),
        &AffectSettings::default(),
    );
    assert!(state.valence > before);
}

#[test]
fn classifier_affect_proposal_ignored_when_unconfident() {
    let mut state = AffectState::default();
    let before = state.valence;
    apply_turn_signals(
        &mut state,
        "the weather is fine",
        Some(&AffectProposal {
            valence: 0.9,
            arousal: 0.4,
            irritation: 0.0,
            affinity: 0.5,
            confidence: 0.1,
        }),
        &AffectSettings::default(),
    );
    assert!((state.valence - before).abs() < f32::EPSILON);
}

#[tokio::test]
async fn runtime_blends_classifier_affect_and_fail_closes() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let runtime = CompanionRuntime::new(Arc::new(store), MindSettings::default());
    let classify = ScriptedClassify::new([
        r#"{"valence":0.85,"arousal":0.2,"irritation":0.0,"affinity":0.4,"confidence":0.9}"#,
    ]);
    runtime
        .on_user_turn(soul.id, "the weather is fine", &[], &[], Some(&classify))
        .await
        .unwrap();
    let after = runtime.soul(soul.id).unwrap();
    assert!(after.affect.valence > soul.affect.valence);

    runtime
        .on_user_turn(
            soul.id,
            "still talking",
            &[],
            &[],
            Some(&ScriptedClassify::silent()),
        )
        .await
        .unwrap();
    let again = runtime.soul(soul.id).unwrap();
    assert!((again.affect.valence - after.affect.valence).abs() < 0.0001);
}

#[test]
fn inner_tags_do_not_leak_on_surface_projection() {
    let (speech, inner) =
        split_surface_and_inner(r#"hi <inner aspect="thought">secret plan</inner>"#);
    assert_eq!(speech, "hi");
    assert_eq!(inner[0].1, "secret plan");
    assert!(model_visible_for(InnerAspect::Thought));
}

#[tokio::test]
async fn proactive_gate_fail_closed_without_llm() {
    let config = ProactiveSettings {
        enabled: true,
        min_idle_seconds: 30,
        ..ProactiveSettings::default()
    };
    let ctx = build_proactive_context(
        &config,
        &["hello".into()],
        &ProactiveObservation::default(),
        None,
        None,
        &[],
        &[],
        ProactiveSuppressionState {
            seconds_since_user_input: 5,
            seconds_since_proactive: 1000,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        },
        crate::proactive::QuietHoursEval::inactive(),
        None,
        None,
    );
    let classify = ScriptedClassify::new([
        r#"{"should_speak":true,"confidence":1.0,"reason":"x","topic_hint":"hi","urgency":"low","screen_digest":""}"#,
    ]);
    let outcome = decide_proactive_speech(&config, &ctx, Some(&classify)).await;
    assert!(!outcome.llm_invoked);
    assert!(matches!(
        outcome.skip,
        Some(ProactiveSkipReason::Gate(GateRejectReason::MinIdle))
    ));
}

#[tokio::test]
async fn proactive_disabled_never_invokes_llm() {
    let config = ProactiveSettings {
        enabled: false,
        min_idle_seconds: 0,
        ..ProactiveSettings::default()
    };
    let ctx = speak_context(&config);
    let classify = ScriptedClassify::new([
        r#"{"should_speak":true,"confidence":1.0,"reason":"x","topic_hint":"hi","urgency":"low","screen_digest":""}"#,
    ]);
    let outcome = decide_proactive_speech(&config, &ctx, Some(&classify)).await;
    assert!(!outcome.llm_invoked);
    assert!(matches!(outcome.skip, Some(ProactiveSkipReason::Disabled)));
}

#[tokio::test]
async fn proactive_speaks_when_gates_pass() {
    let config = ProactiveSettings {
        enabled: true,
        min_idle_seconds: 0,
        cooldown_seconds: 0,
        ..ProactiveSettings::default()
    };
    let ctx = speak_context(&config);
    let classify = ScriptedClassify::new([
        r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"check in","urgency":"low","screen_digest":""}"#,
    ]);
    let outcome = decide_proactive_speech(&config, &ctx, Some(&classify)).await;
    assert!(outcome.skip.is_none());
    assert_eq!(outcome.decision.topic_hint, "check in");
}

#[tokio::test]
async fn proactive_confirmation_silent_token() {
    assert_eq!(
        classify_confirmation_prefix(SILENT_TOKEN),
        ProactiveConfirmation::Declined
    );
    assert_eq!(
        classify_confirmation_prefix("hello there"),
        ProactiveConfirmation::Accepted
    );
    assert_eq!(
        classify_confirmation_prefix("   "),
        ProactiveConfirmation::Empty
    );
}

fn speak_context(config: &ProactiveSettings) -> crate::proactive::ProactiveContext {
    build_proactive_context(
        config,
        &["hey".into()],
        &ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            screen_summary_status: ScreenSummaryStatus::Disabled,
        },
        None,
        None,
        &[],
        &[],
        ProactiveSuppressionState {
            seconds_since_user_input: 200,
            seconds_since_proactive: 1000,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        },
        crate::proactive::QuietHoursEval::inactive(),
        None,
        None,
    )
}

#[test]
fn world_state_does_not_store_screen_summary() {
    let mut ring = WorldStateMemory::default();
    let cfg = crate::config::WorldStateSettings {
        enabled: true,
        ..crate::config::WorldStateSettings::default()
    };
    let obs = ProactiveObservation {
        captured_at_unix_ms: 1,
        activity: Some(ActivitySnapshot {
            idle_seconds: Some(10),
            active_window_label: "Code".into(),
            recent_change: String::new(),
        }),
        screen_summary: Some("secret pixels".into()),
        screen_summary_status: ScreenSummaryStatus::Available,
    };
    ring.push(WorldStateSnapshot::from_observation(&obs, 60), &cfg);
    ring.push(
        WorldStateSnapshot::from_observation(
            &ProactiveObservation {
                captured_at_unix_ms: 2,
                ..obs.clone()
            },
            60,
        ),
        &cfg,
    );
    ring.push(
        WorldStateSnapshot::from_observation(
            &ProactiveObservation {
                captured_at_unix_ms: 3,
                activity: Some(ActivitySnapshot {
                    idle_seconds: Some(30),
                    active_window_label: "Code".into(),
                    recent_change: String::new(),
                }),
                ..obs
            },
            60,
        ),
        &cfg,
    );
    let summary = ring.summary(&cfg).unwrap();
    assert_eq!(summary.snapshot_count, 3);
    assert_eq!(summary.idle_trend, crate::proactive::IdleTrend::Rising);
}

#[test]
fn package_install_and_soul_creation() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let files = stamp_digest(sample_char_files());
    let zip = pack_archive(&files).unwrap();
    let installed =
        install_archive(&store, &dir.path().join("characters"), &zip, 10_000_000).unwrap();
    assert_eq!(installed.kind, crate::package::PackageKind::Character);
    assert!(!installed.origin_unverified);
    let soul = soul_from_install(&store, &installed).unwrap();
    assert!(soul.character_ref.contains("char.mychar"));
    let exported = export_dir(&installed.path).unwrap();
    let roundtrip = install_archive(
        &store,
        &dir.path().join("characters2"),
        &exported,
        10_000_000,
    )
    .unwrap();
    assert_eq!(roundtrip.id, installed.id);
}

#[test]
fn package_rejects_unknown_format_and_bad_digest() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let mut files = sample_char_files();
    let manifest = String::from_utf8(files["manifest.toml"].clone())
        .unwrap()
        .replace("format_version = 1", "format_version = 99");
    files.insert("manifest.toml".into(), manifest.into_bytes());
    let zip = pack_archive(&files).unwrap();
    let err = install_archive(&store, &dir.path().join("c"), &zip, 10_000_000).unwrap_err();
    assert!(matches!(
        err,
        CompanionError::UnknownFormat { found: 99, .. }
    ));

    let mut files = sample_char_files();
    let manifest = String::from_utf8(files["manifest.toml"].clone())
        .unwrap()
        .replace("digest = \"\"", "digest = \"sha256:deadbeef\"");
    files.insert("manifest.toml".into(), manifest.into_bytes());
    let zip = pack_archive(&files).unwrap();
    let err = install_archive(&store, &dir.path().join("c2"), &zip, 10_000_000).unwrap_err();
    assert!(matches!(err, CompanionError::DigestMismatch));
}

#[test]
fn v3_json_imports_as_enechar() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let card = serde_json::json!({
        "spec": "chara_card_v3",
        "spec_version": "3.0",
        "data": {
            "name": "Imported",
            "description": "A visitor.",
            "personality": "kind"
        }
    });
    let bytes = serde_json::to_vec(&card).unwrap();
    let installed = import_v3(&store, &dir.path().join("characters"), &bytes, 10_000_000).unwrap();
    assert!(installed.path.join("soul/persona.md").exists());
    assert!(
        std::fs::read_to_string(installed.path.join("soul/persona.md"))
            .unwrap()
            .contains("A visitor")
    );
}

#[tokio::test]
async fn memory_tools_surface_omits_write_shared() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let soul = store.create_soul(&NewSoul::text_only("char.a@1")).unwrap();
    let registry = ToolRegistry::new();
    let audit = ene_plane::AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = Arc::new(ene_plane::ApprovalPlane::new(
        ene_plane::ApprovalSettings::default(),
        audit,
        ene_plane::ScriptedPopup::deny_all(),
        None,
    ));
    plane.set_policy(ene_plane::PolicyFile {
        rules: vec![ene_plane::PolicyRule {
            tool: "memory.write_shared".to_owned(),
            scope: None,
            decision: ene_plane::PolicyDecision::Allow,
        }],
    });
    registry.set_plane(Arc::clone(&plane));
    register_memory_tools(&registry, Arc::new(store), None);
    assert!(surface_hides_write_shared(&registry));
    registry
        .execute(
            "memory.write_shared",
            serde_json::json!({
                "title": "cat",
                "content": "the user's cat is Tama",
                "kind": "user_profile",
                "soul_id": soul.id.to_string()
            }),
            Layer::Job,
        )
        .await
        .unwrap();
    let recalled = registry
        .execute(
            "memory.recall",
            serde_json::json!({ "query": "cat", "soul_id": soul.id.to_string() }),
            Layer::Surface,
        )
        .await
        .unwrap();
    let blob = recalled.to_string();
    assert!(blob.contains("Tama"));
    assert!(!blob.contains(&format!("soul {}", soul.id)));
    let records = plane.audit().records().unwrap();
    assert!(records.iter().any(
        |row| row.kind == "approval" && row.payload.to_string().contains("memory.write_shared")
    ));
}

struct FixedEmbed(Vec<f32>);

#[async_trait::async_trait]
impl QueryEmbed for FixedEmbed {
    async fn embed_query(&self, _text: &str) -> Option<Vec<f32>> {
        Some(self.0.clone())
    }
}

#[tokio::test]
async fn memory_recall_tool_is_hybrid_when_embedder_bound() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let near = insert_episodic(&store, soul.id, "apple pie", "baked dessert");
    let far = insert_episodic(&store, soul.id, "zebra stripes", "black and white");
    store.set_embedding(near.id, &[1.0, 0.0]).unwrap();
    store.set_embedding(far.id, &[0.0, 1.0]).unwrap();
    let store = Arc::new(store);

    let lexical = ToolRegistry::new();
    register_memory_tools(&lexical, Arc::clone(&store), None);
    let empty = lexical
        .execute(
            "memory.recall",
            serde_json::json!({ "query": "xyzzy", "soul_id": soul.id.to_string() }),
            Layer::Surface,
        )
        .await
        .unwrap();
    assert_eq!(empty["memories"].as_array().map(Vec::len), Some(0));

    let hybrid = ToolRegistry::new();
    register_memory_tools(&hybrid, store, Some(Arc::new(FixedEmbed(vec![1.0, 0.0]))));
    let hits = hybrid
        .execute(
            "memory.recall",
            serde_json::json!({ "query": "xyzzy", "soul_id": soul.id.to_string() }),
            Layer::Surface,
        )
        .await
        .unwrap();
    let blob = hits["memories"].to_string();
    assert!(blob.contains("apple pie"), "{blob}");
    assert!(!blob.contains("zebra"), "{blob}");
}

#[tokio::test]
async fn runtime_persists_affect_across_turns() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    let runtime = CompanionRuntime::new(Arc::new(store), MindSettings::default());
    runtime
        .on_user_turn(
            soul.id,
            "thank you so much",
            &[],
            &[(InnerAspect::Emotion, "emotion: happy(0.8)".into())],
            None,
        )
        .await
        .unwrap();
    let after = runtime.soul(soul.id).unwrap();
    assert!(after.affect.valence > soul.affect.valence);
    assert_eq!(after.affect.mood_label, "happy");
}

#[tokio::test]
async fn runtime_forwards_tone_notes_to_affect_classifier() {
    let (_dir, store) = open_store();
    let soul = store
        .create_soul(&NewSoul::text_only("char.ene@1"))
        .unwrap();
    store.set_skill_refs(soul.id, &["travel".into()]).unwrap();
    assert_eq!(
        store.get_soul(soul.id).unwrap().unwrap().skill_refs,
        vec!["travel"]
    );
    let runtime = CompanionRuntime::new(Arc::new(store), MindSettings::default());
    let classify = ScriptedClassify::new([
        r#"{"valence":0.2,"arousal":0.1,"irritation":0.0,"affinity":0.2,"confidence":0.8}"#,
    ]);
    runtime
        .on_user_turn(
            soul.id,
            "plan a trip",
            &["keep it light".into()],
            &[],
            Some(&classify),
        )
        .await
        .unwrap();
    let input = classify.last_input().expect("classifier ran");
    assert!(input.contains("keep it light"), "{input}");
    assert!(input.contains("plan a trip"), "{input}");
}

fn sample_char_files() -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.toml".into(),
        b"[package]\nkind = \"character\"\nid = \"char.mychar\"\nversion = \"1.0.0\"\nformat_version = 1\ndisplay_name = \"My Character\"\n\n[contents]\nsoul = \"embedded\"\nbody = \"embedded\"\n\n[integrity]\ndigest = \"\"\n".to_vec(),
    );
    files.insert(
        "soul/soul.toml".into(),
        b"[identity]\nname = \"Ene\"\n\n[affect]\nbaseline = { valence = 0.2, arousal = 0.1, dominance = 0.0, trust = 0.3, affinity = 0.3, irritation = 0.0, curiosity = 0.4, fatigue = 0.0 }\n".to_vec(),
    );
    files.insert("soul/persona.md".into(), b"You are Ene.".to_vec());
    files.insert(
        "body/body.toml".into(),
        b"[body]\nkind = \"text\"\n\n[expressions]\navailable = [\"happy\", \"calm\"]\n".to_vec(),
    );
    files
}

fn stamp_digest(mut files: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let digest = content_digest(&files);
    let manifest = String::from_utf8(files.get("manifest.toml").unwrap().clone()).unwrap();
    files.insert(
        "manifest.toml".into(),
        manifest
            .replace("digest = \"\"", &format!("digest = \"{digest}\""))
            .into_bytes(),
    );
    files
}

fn sample_soul_files() -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.toml".into(),
        b"[package]\nkind = \"soul\"\nid = \"soul.ene\"\nversion = \"1.0.0\"\nformat_version = 1\ndisplay_name = \"Ene Soul\"\n\n[integrity]\ndigest = \"\"\n".to_vec(),
    );
    files.insert(
        "soul/soul.toml".into(),
        b"[identity]\nname = \"Ene\"\n\n[affect]\nbaseline = { valence = 0.1, arousal = 0.0, dominance = 0.0, trust = 0.2, affinity = 0.2, irritation = 0.0, curiosity = 0.3, fatigue = 0.0 }\n".to_vec(),
    );
    files.insert("soul/persona.md".into(), b"You are Ene.".to_vec());
    files
}

fn sample_body_files() -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.toml".into(),
        b"[package]\nkind = \"body\"\nid = \"body.text\"\nversion = \"1.0.0\"\nformat_version = 1\ndisplay_name = \"Text Body\"\n\n[integrity]\ndigest = \"\"\n".to_vec(),
    );
    files.insert(
        "body/body.toml".into(),
        b"[body]\nkind = \"text\"\n\n[expressions]\navailable = [\"happy\", \"calm\"]\n".to_vec(),
    );
    files
}

#[test]
fn soul_and_body_packages_compose() {
    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let soul_zip = pack_archive(&stamp_digest(sample_soul_files())).unwrap();
    let body_zip = pack_archive(&stamp_digest(sample_body_files())).unwrap();
    let soul_pkg = install_archive(
        &store,
        &dir.path().join("characters"),
        &soul_zip,
        10_000_000,
    )
    .unwrap();
    let body_pkg = install_archive(
        &store,
        &dir.path().join("characters"),
        &body_zip,
        10_000_000,
    )
    .unwrap();
    let composed = compose_soul_and_body(&store, &soul_pkg, &body_pkg).unwrap();
    assert!(composed.character_ref.contains("soul.ene"));
    assert!(composed.body_ref.is_some());
}

#[test]
fn package_localizes_display_name_en_us_and_ja() {
    let mut files = sample_char_files();
    files.insert(
        "i18n/en-US.toml".into(),
        b"display_name = \"My Character\"\n".to_vec(),
    );
    files.insert(
        "i18n/ja.toml".into(),
        "display_name = \"マイキャラ\"\n".as_bytes().to_vec(),
    );
    assert_eq!(localized_display_name(&files, "en-US"), "My Character");
    assert_eq!(localized_display_name(&files, "ja"), "マイキャラ");
    assert_eq!(localized_display_name(&files, "fr"), "My Character");

    let dir = TempDir::new().unwrap();
    let store = CompanionStore::open(dir.path().join("companions.db")).unwrap();
    let zip = pack_archive(&stamp_digest(files)).unwrap();
    let installed =
        install_archive(&store, &dir.path().join("characters"), &zip, 10_000_000).unwrap();
    assert!(installed.path.join("i18n/ja.toml").is_file());
    assert!(installed.path.join("i18n/en-US.toml").is_file());
}

#[test]
fn tendency_does_not_pierce_gates() {
    let config = ProactiveSettings {
        enabled: true,
        min_idle_seconds: 100,
        ..ProactiveSettings::default()
    };
    let chatty = config.with_tendency("chatty");
    assert!(chatty.min_confidence < config.min_confidence);
    let ctx = build_proactive_context(
        &chatty,
        &["x".into()],
        &ProactiveObservation::default(),
        None,
        None,
        &[],
        &[],
        ProactiveSuppressionState {
            seconds_since_user_input: 1,
            seconds_since_proactive: 10_000,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        },
        crate::proactive::QuietHoursEval::inactive(),
        None,
        None,
    );
    assert_eq!(
        evaluate_deterministic_gates(&chatty, &ctx),
        Err(GateRejectReason::MinIdle)
    );
}

#[test]
fn projection_helper_flags_inner_roles() {
    let history = derive_messages(&[], ProjectOptions::for_depth(DisplayDepth::Surface, 8));
    assert!(!surface_leaks_inner(&history));
}

#[test]
fn zip_magic_and_package_manifest_detection() {
    assert!(!looks_like_zip(b"not a zip"));
    assert!(!looks_like_package_zip(b"not a zip"));
    let zip = pack_archive(&stamp_digest(sample_char_files())).unwrap();
    assert!(looks_like_zip(&zip));
    assert!(looks_like_package_zip(&zip));
}

#[test]
fn avatar_path_reads_body_toml_and_vrm_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let body = dir.path().join("body");
    std::fs::create_dir_all(body.join("avatar")).unwrap();
    std::fs::write(
        body.join("body.toml"),
        "[body]\nkind = \"vrm\"\navatar = \"avatar/model.vrm\"\n",
    )
    .unwrap();
    let vrm = body.join("avatar/model.vrm");
    std::fs::write(&vrm, b"vrm-bytes").unwrap();
    assert_eq!(
        avatar_path_for_install(dir.path()).as_deref(),
        Some(vrm.as_path())
    );
}
