# W7 検証記録(v1.0 ハーネス)

> 実装ウェーブ W0–W6 の上で、[done.md](done.md) の総括条件と
> [process-model.md §6](../platform/process-model.md#6-性能予算) の
> 性能基準線を観測した記録。後継設計(resume / 3ストア / marketplace /
> MCP 導線 UI / 署名)は対象外。

測定日: 2026-08-17。対象は現行アプリと並べて追加した新クレート
(`ene-session` / `ene-kernel` / `ene-daemon` / `ene-api` / `ene-ctl` /
`ene-stage` ほか)。旧 `ene-desktop` の置き換えはしていない。

## 総括条件

| # | 条件 | 新ハーネスでの観測 |
|---|---|---|
| 1 | stage + CLI + Web が同一コアに接続 | 成立。`ene-daemon::http_tests::three_clients_share_one_core` |
| 2 | 1 体が done.md の全 P-xxx を満たす | 成立(自動テスト観測)。下表の P-id をクレート単体/HTTP で固定。stage の VRM は minimal fixture + wgpu load。実機 GPU 同室 E2E は lavapipe 依存 |
| 3 | ネットワークなしで会話 | 未達。`spawned_core_offline_conversation_and_rss` は EchoModel 往復のみ。provider 配線なし |
| 4 | ビルドと性能(D-29) | 成立。`minimal_http_baselines_are_measurable` / `kernel::echo_turn_to_first_chunk_is_measurable` / `w7_acceptance` RSS。Cloud VM は `cargo` 直接(AGENTS.md) |
| 5 | 監査・バックアップ・エクスポート | 成立。`ene-plane::audit_hash_chain_verifies` / `http::backup::backup_and_restore_roundtrip` / `export_default_omits_inner` |

## v1.0 P-id → 自動テスト対応表

後継・stretch は掲載しない(P-111, P-113–P-115, P-213–P-214, P-308, P-408–P-409,
P-513–P-514, P-525, P-616, P-710–P-711, P-807, marketplace, 署名)。

| P-id | テスト (クレート::関数) |
|---|---|
| P-101 | `ene-kernel::text_turn_is_logged_and_projected`; `ene-daemon::http_tests::three_clients_share_one_core`; `ene-daemon::http_tests::concurrent_prompt_returns_lane_busy` |
| P-102 | `ene-body::barge_in_stops_playback_after_min_speech`; `ene-body::self_voice_during_playback_is_ignored`; `ene-body::idle_speech_becomes_transcript` |
| P-103 | `ene-kernel::abort_does_not_write_assistant_closure`; `ene-daemon::http_tests::boot_seeds_two_souls_and_session_ops` (barge-in API) |
| P-104 | `ene-session::surface_projection_hides_inner_and_thinking`; `ene-kernel::surface_live_subscription_does_not_receive_inner`; `ene-daemon::surface_ws_never_sees_inner` |
| P-105 | `ene-companion::proactive_gate_fail_closed_without_llm`; `ene-companion::proactive_speaks_when_gates_pass`; `ene-companion::proactive_disabled_never_invokes_llm`; `ene-companion::tendency_does_not_pierce_gates` |
| P-106 | `ene-body::autonomy_tick_does_not_require_a_turn` |
| P-107 | `ene-daemon::http_tests::boot_seeds_two_souls_and_session_ops` |
| P-108 | `ene-session::session_end_and_surface_search`; `ene-daemon::http_tests::boot_seeds_two_souls_and_session_ops` (split / end / search) |
| P-109 | `ene-session::fork_copies_prefix_and_leaves_source_intact`; `ene-daemon::http_tests::fork_leaves_original_session_intact` |
| P-110 | `ene-daemon::http_tests::export_default_omits_inner` |
| P-112 | `ene-work::observe_screen_does_not_enter_session_history`; `ene-work::screenshot_is_surface_and_high_sensitivity`; `ene-companion::world_state_does_not_store_screen_summary` |
| P-201 | `ene-companion::memory_survives_reopen` |
| P-202 | `ene-companion::extract_names_as_shared_and_arbitrates` |
| P-203 | `ene-companion::memory_survives_reopen` (recall); `ene-companion::shared_pool_is_usable_by_another_soul_as_own_knowledge` |
| P-204 | `ene-companion::extract_names_as_shared_and_arbitrates`; `ene-companion::classifier_scope_defaults_private_when_missing` |
| P-205 | `ene-companion::extract_names_as_shared_and_arbitrates` (arbitrate) |
| P-206 | `ene-companion::decay_surfaces_low_salience_forgetting_candidates` |
| P-207 | `ene-companion::shared_pool_is_usable_by_another_soul_as_own_knowledge` |
| P-208 | `ene-companion::extract_names_as_shared_and_arbitrates` |
| P-209 | `ene-companion::runtime_persists_affect_across_turns`; `ene-companion::affect_decays_toward_baseline_but_trust_accumulates` |
| P-210 | `ene-companion::forget_request_removes_matching_memory_and_records_journal` |
| P-211 | `ene-companion::sensitive_candidate_queues_for_approval` |
| P-212 | `ene-companion::memory_tools_surface_omits_write_shared` |
| P-301 | `ene-companion::self_report_updates_mood_label` |
| P-302 | `ene-companion::affect_decays_toward_baseline_but_trust_accumulates` |
| P-303 | `ene-body::emotion_always_emits_expression_even_without_body`; `ene-body::lipsync_from_tone_has_amplitude` |
| P-304 | `ene-companion::expression_arbiter_suppresses_rapid_label_changes` |
| P-305 | `ene-companion::runtime_persists_affect_across_turns` |
| P-306 | `ene-work::schedule_remind_fires_and_quiet_hours_defer`; `ene-companion::proactive_gate_fail_closed_without_llm` (quiet-hours gate path) |
| P-307 | `ene-body::commands_never_include_pad_numbers`; `ene-daemon::http_tests::boot_seeds_two_souls_and_session_ops` (`/affect`) |
| P-401 | `ene-companion::package_install_and_soul_creation`; `ene-companion::soul_and_body_packages_compose` |
| P-402 | `ene-body::hot_swap_drops_pending_cues`; `ene-body::unknown_emotion_falls_back_with_warning` |
| P-403 | `ene-vrm::minimal_glb_parses_as_vrm`; `ene-vrm::minimal_glb_loads_with_wgpu`; `ene-stage::default_minimal_vrm_writes_parseable_glb` |
| P-404 | `ene-body::lipsync_from_tone_has_amplitude`; `ene-body::emotion_always_emits_expression_even_without_body`; `ene-body::autonomy_tick_does_not_require_a_turn` (look-at) |
| P-405 | `ene-body::stage_caps_concurrent_rendered_bodies`; `ene-daemon::http_tests::boot_seeds_two_souls_and_session_ops` |
| P-406 | `ene-body::hot_swap_drops_pending_cues` |
| P-407 | `ene-daemon::boot_stage_maps_emotion_without_a_rendered_body`; `ene-body::emotion_always_emits_expression_even_without_body` |
| P-501 | `ene-session::turn_roundtrip_projects_history`; `ene-session::seq_is_monotonic_without_gaps` |
| P-502 | `ene-session::model_visible_hash_matches_projection`; `ene-kernel::model_visible_hash_matches_logged_projection` |
| P-503 | `ene-kernel::text_turn_is_logged_and_projected` |
| P-504 | `ene-work::lane_prompt_still_works_while_job_running` |
| P-505 | *(コンテキスト組立は Echo 経路のターン記録で間接固定)* `ene-kernel::text_turn_is_logged_and_projected` |
| P-506 | `ene-session::compaction_replaces_range_in_projection`; `ene-kernel::compact_keeps_original_rows` |
| P-507 | `ene-work::spill_huge_tool_output_keeps_brief_bounded` |
| P-508 | `ene-work::internal_delegation_has_no_job_row` |
| P-509 | `ene-work::grandchild_delegation_respects_depth_guard` |
| P-510 | `ene-work::interrupt_recovery_and_tool_failure_use_different_wording` |
| P-511 | `ene-work::mutating_work_waits_for_plan_approval` |
| P-512 | `ene-work::combined_child_questions_merge_and_route_answers` |
| P-515 | `ene-kernel::crash_recovery_is_reported_and_not_resumed`; `ene-daemon::boot_recovers_interrupted_turn_without_resume`; `ene-work::job_persists_and_recover_does_not_resume`; `ene-daemon::boot_reports_interrupted_job_without_resume` |
| P-516 | `ene-session::usage_ledger_rows_are_append_only`; `ene-daemon::http_tests::usage_ledger_records_completed_turn` |
| P-517 | `ene-kernel::observe_spans_do_not_leak_content`; `ene-daemon::http_tests::http_spans_and_schema_and_anon_health`; `ene-ctl::core_smoke::ctl_client_lists_tools_and_debug_spans` |
| P-518 | `ene-session::storage_too_new_is_rejected`; `ene-session::older_storage_migrates_and_interrupts_open_work`; `ene-session::recover_closes_open_turn_and_abandons_inbox` |
| P-519 | `ene-work::progress_and_complete_are_companion_speech`; `ene-work::surface_message_and_cancel_while_running` |
| P-520 | `ene-session::surface_projection_hides_inner_and_thinking`; `ene-daemon::surface_ws_never_sees_inner`; `ene-stage::surface_blocks_inner_and_thinking` |
| P-521 | `ene-work::progress_and_complete_are_companion_speech`; `ene-work::completion_waits_for_user_speech_gap`; barge-in / mic release / prompt drain in `ene-daemon` |
| P-522 | `ene-work::surface_fs_write_upgrades_without_invoking`; `ene-work::lane_auto_upgrade_does_not_execute_fs_write` |
| P-523 | `ene-work::step_budget_upgrades_even_for_empty_side_effects`; `ene-work::surface_router_upgrades_fs_write_without_spy` |
| P-524 | `ene-companion::proactive_speaks_when_gates_pass` (補助 LLM 分類); `ene-companion::classifier_scope_defaults_private_when_missing` |
| P-601 | `ene-registry::surface_schemas_omit_fs_write`; `ene-registry::harness_tool_uses_the_same_pipeline` |
| P-602 | `ene-registry::builtin_specs_cover_four_plugins`; `plugins/harness/utility::out_of_process_utility_registers_and_runs` |
| P-603 | `ene-work::mcp_handwritten_tools_execute_through_registry` |
| P-604 | `ene-work::mcp_handwritten_tools_execute_through_registry` (git スタブ) |
| P-605 | `ene-work::lane_prompt_still_works_while_job_running`; `ene-work::progress_and_complete_are_companion_speech` |
| P-606 | `ene-work::schedule_remind_fires_and_quiet_hours_defer`; `ene-work::missed_remind_fires_once` |
| P-607 | `ene-work::schedule_remind_fires_and_quiet_hours_defer` |
| P-608 | `ene-work::bookmark_workflow_delivers_markdown_artifact` |
| P-609 | `ene-work::bookmark_workflow_delivers_markdown_artifact`; `ene-work::artifact_register_and_deliver` |
| P-610 | `ene-work::skill_install_and_load` |
| P-611 | `ene-registry::exec_tools_stay_off_surface_schema`; `ene-plane::exec_is_higher_risk_than_workspace_fs_write` |
| P-612 | `ene-work::mcp_handwritten_tools_execute_through_registry`; `ene-registry::exec_tools_stay_off_surface_schema` |
| P-613 | `ene-work::work_tools_cover_delegate_surface`; `ene-registry::plane_denies_side_effects_and_sensitive_reads` |
| P-614 | `ene-work::spill_huge_tool_output_keeps_brief_bounded` |
| P-615 | `ene-work::work_tools_cover_delegate_surface` |
| P-701 | `ene-stage::spawn_core_writes_api_json_and_health_succeeds` |
| P-702 | `ene-daemon::http_tests::three_clients_share_one_core` |
| P-703 | `ene-daemon::http_tests::three_clients_share_one_core` (OpenAPI) |
| P-704 | `ene-daemon::http_tests::web_ui_cannot_mutate_memory_or_settings` (eframe/wgpu, WebView なし) |
| P-705 | `ene-ctl::core_smoke::ctl_client_lists_tools_and_debug_spans`; `ene-ctl::core_smoke::cli_binary_starts_core_and_runs_session_ops` |
| P-706 | `ene-daemon::http_tests::web_ui_cannot_mutate_memory_or_settings`; `three_clients_share_one_core` |
| P-707 | `ene-daemon::http_tests::boot_loads_settings_json_token_file`; `exclusive_mic_is_first_writer`; `approval_first_writer_wins` |
| P-708 | `ene-daemon::http_tests::http_spans_and_schema_and_anon_health` |
| P-709 | `ene-daemon::http::backup::backup_and_restore_roundtrip`; `http_tests::backup_copies_stores`; `http_tests::backup_restore_roundtrip_and_unknown_id` |
| P-712 | `ene-daemon::surface_ws_never_sees_inner`; `http_tests::export_default_omits_inner` (detail 履歴) |
| P-801 | `ene-companion::package_install_and_soul_creation` |
| P-802 | `ene-companion::soul_and_body_packages_compose` |
| P-803 | `ene-companion::v3_json_imports_as_enechar` |
| P-804 | `ene-companion::package_install_and_soul_creation` (export roundtrip) |
| P-805 | `ene-companion::package_localizes_display_name_en_us_and_ja` |
| P-806 | `ene-companion::package_rejects_unknown_format_and_bad_digest`; `ene-fiber::manifest_digest_matches_python_plugin_contract` |
| P-808 | `ene-companion::package_rejects_unknown_format_and_bad_digest` |
| P-901 | `plugins/harness/utility::process_survives_os_sandbox_when_supported` (Landlock 対応時) |
| P-902 | `ene-fiber::undeclared_broker_op_is_denied` |
| P-903 | `ene-plane::policy_allows_workspace_write`; `ene-plane::implicit_side_effect_without_popup_is_denied` |
| P-904 | `ene-plane::ai_judgement_reason_is_audited` |
| P-905 | `ene-daemon::approval_first_writer_wins` |
| P-906 | `ene-plane::policy_add_requires_confirmation` |
| P-907 | `ene-plane::vault_inject_ref_does_not_embed_plaintext`; `ene-daemon::boot_installs_approval_plane_and_vault` |
| P-908 | `ene-plane::audit_hash_chain_verifies` |
| P-909 | Echo 経路のみ（総括 3 未達）。`ene-daemon::w7_acceptance::spawned_core_offline_conversation_and_rss` |
| P-910 | `ene-kernel::observe_spans_do_not_leak_content` |
| P-1001 | `plugins/harness/utility::out_of_process_utility_registers_and_runs` |
| P-1002 | `ene-fiber::apply_profile_unloads_removed_rows_and_keeps_uid` |
| P-1003 | `ene-kernel::EchoModel` 経路 (`text_turn_is_logged_and_projected`) |
| P-1004 | `ene-fiber::python_dummy_registers_in_registry_and_executes` |
| P-1005 | `ene-fiber::circuit_breaker_opens_after_spawn_failures`; `ene-fiber::unload_removes_tools_and_grants` |
| P-1006 | `ene-fiber::sidecar_binary_resolves_config_then_cas_then_bundled_and_rejects_urls`; `ene-fiber::sidecar_spawn_health_and_kill_on_loopback` |
| P-1007 | `ene-kernel::waterfall_rewrites_by_calling_next_and_emit_cannot`; `ene-kernel::waterfall_pre_step_can_stop_the_model` |
| P-1008 | `ene-fiber::manifest_digest_matches_python_plugin_contract` |
| P-1009 | `ene-fiber::dummy_plugin_handshake_without_provider_subprotocol` |
| P-1010 | `ene-fiber::requires_unsatisfied_row_waits_without_error`; `ene-fiber::circular_requires_are_reported_and_rows_stay_inactive`; `ene-daemon::disabling_one_fiber_does_not_restart_the_core` |

## 固定した不変条件(テスト)

- 表層 WS / 履歴 / default export に inner が乗らない。詳細では読める
- 中断は検出・片付け・報告。自動 resume しない
- 同時 prompt は `lane_busy`。承認と mic は first-writer
- Web UI は memory/settings の PATCH/DELETE を持たない。stage は eframe+wgpu で WebView なし
- スパン属性にプロンプト内容が乗らない
- 空トークンは `/api/v1/health` 以外 `unauthorized`
- 会話 LLM は EchoModel。P-xxx の自動テストは機構検出。実モデルの分類・応答品質は総括 3 完了まで対象外

## 性能基準線

正は `minimal`(描画なし / EchoModel / debug)。数値と CI 上限は
[process-model.md §6.1](../platform/process-model.md#61-v10-基準線d-29)。

## 意図的にやらないこと(W7)

- `resume` / `lane.last_result` / OperationState / effect sandwich
- キャラ署名・marketplace・MCP 接続導線 UI
- `docs/` と `AGENTS.md` の新レイアウトへの書き換え
- 旧 `ene-desktop` / `ene-cli` の削除
