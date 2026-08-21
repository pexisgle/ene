# W7 検証記録(v1.0 ハーネス)

> 実装ウェーブ W0–W6 の上で、[done.md](done.md) の総括条件と
> [process-model.md §6](../platform/process-model.md#6-性能予算) の
> 性能基準線を観測した記録。後継設計(resume / 3ストア / marketplace /
> MCP 導線 UI / 署名)は対象外。
>
> **機構検出と完成宣言は別列。** テスト関数が存在するだけでは
> [done.md](done.md) の `[x]` にしない。完成は Echo / Scripted /
> Placeholder 以外、または実プロセスで観察できるものに限る。

測定日: 2026-08-17(機構検出)。完成列の見直しは 2026-08-21。
対象は現行アプリと並べて追加した新クレート
(`ene-session` / `ene-kernel` / `ene-daemon` / `ene-api` / `ene-ctl` /
`ene-stage` ほか)。W7 時点では旧 `ene-desktop` も残していた。

## 経路の記号

| 記号 | 意味 |
|---|---|
| Echo | `EchoModel`(最終ユーザー文を `ack:` 。`tool_calls` は空) |
| Scripted | `ScriptedClassify` / `ScriptedTts` / `ScriptedAsr` / `ScriptedAi` / `ScriptedMcp` |
| Placeholder | `{ "available": false }` 等のプレースホルダ |
| host API | テストがホスト API(`host.start` / `progress` / `complete` 等)を直接呼ぶ |
| launcher | `BuiltinKind` ランチャー起動。本体はホスト lib（現行バンドルは `run_tool_plugin`） |
| store | 実ストア / スキーマ / 監査 / マイグレーション |
| HTTP | 実 HTTP/WS プロセス |
| process | アウトプロセスプラグインまたはサンドボックス |

## 総括条件

| # | 条件 | 機構 | 完成 |
|---|---|---|---|
| 1 | stage + CLI + Web が同一コアに接続 | `ene-daemon::http_tests::three_clients_share_one_core` | はい(HTTP) |
| 2 | 1 体が done.md の全 P-xxx を満たす | 下表の機構検出。VRM は minimal fixture | いいえ。総括 3・ジョブループ・本番音声が未達 |
| 3 | ネットワークなしで会話 | `spawned_core_offline_conversation_and_rss` は Echo 往復（機構）。`seamed_model_rejects_unconfigured_chat`; `tool_calling_model_runs_calc_through_http` | いいえ（実 GGUF 未観測） |
| 4 | ビルドと性能(D-29) | `minimal_http_baselines_are_measurable` / `kernel::echo_turn_to_first_chunk_is_measurable` / `w7_acceptance` RSS | はい(測定。会話は Echo) |
| 5 | 監査・バックアップ・エクスポート | `ene-plane::audit_hash_chain_verifies` / `http::backup::backup_and_restore_roundtrip` / `export_default_omits_inner` | はい(store / HTTP) |

## v1.0 P-id → 機構 / 完成

後継・stretch は掲載しない(P-111, P-113–P-115, P-213–P-214, P-308, P-408–P-409,
P-513–P-514, P-525, P-616, P-710–P-711, P-807, marketplace, 署名)。

| P-id | 機構テスト | 経路 | 完成 |
|---|---|---|---|
| P-101 | `ene-kernel::text_turn_is_logged_and_projected`; `tool_calling_model_runs_surface_tool_then_speaks`; `ene-daemon::http_tests::three_clients_share_one_core`; `tool_calling_model_runs_calc_through_http`; `seamed_model_rejects_unconfigured_chat` | Echo（機構） / ToolCalling / Seamed fail-closed | いいえ(実モデル応答は未観測) |
| P-102 | `ene-body::barge_in_stops_playback_after_min_speech`; `self_voice_during_playback_is_ignored`; `idle_speech_becomes_transcript` | Scripted | いいえ |
| P-103 | `ene-kernel::abort_does_not_write_assistant_closure`; `boot_seeds_two_souls_and_session_ops` (barge-in API); `ene-stage::parses_audio_chunk_abort`; `ene-stage::stop_clears_recent_playback_pcm` | Echo / HTTP / stage | 部分(stage が `audio.chunk` abort で sink 停止と viseme reset。0.5s 手動は本番 TTS 未配線) |
| P-104 | `ene-session::surface_projection_hides_inner_and_thinking`; `ene-kernel::surface_live_subscription_does_not_receive_inner`; `ene-daemon::surface_ws_never_sees_inner` | store / HTTP | はい(表層非露出) |
| P-105 | `ene-companion::proactive_gate_fail_closed_without_llm`; `proactive_speaks_when_gates_pass`; `proactive_disabled_never_invokes_llm`; `tendency_does_not_pierce_gates` | Scripted | いいえ(ゲート機構は実コード) |
| P-106 | `ene-body::autonomy_tick_does_not_require_a_turn` | store | はい(描画なし tick) |
| P-107 | `boot_seeds_two_souls_and_session_ops`; `two_souls_keep_isolated_sessions_and_stage_occupants`; overlay 2-slot layout | HTTP / stage | 部分(セッション隔離と2スロット。GUI E2E は手動) |

| P-108 | `ene-session::session_end_and_surface_search`; `boot_seeds_two_souls_and_session_ops` (split / end / search) | store / HTTP | はい |
| P-109 | `ene-session::fork_copies_prefix_and_leaves_source_intact`; `fork_leaves_original_session_intact` | store / HTTP | はい |
| P-110 | `ene-daemon::http_tests::export_default_omits_inner` | HTTP | はい |
| P-112 | `ene-work::observe_screen_from_png_does_not_enter_session_history`; `tool_path_returns_png_and_placeholder_is_unavailable`; `world_state_does_not_store_screen_summary` | process / host API | 部分(PNG観測。ツール結果は画像ブロック未配線) |
| P-201 | `ene-companion::memory_survives_reopen` | store | はい(永続化。抽出は正規表現) |
| P-202 | `ene-companion::extract_names_as_shared_and_arbitrates` | Scripted / 正規表現 | いいえ |
| P-203 | `recall_without_query_vector_stays_lexical`; `recall_with_query_vector_ranks_embedded_neighbor`; `runtime_hybrid_recall_matches_store_when_vector_present`; `memory_recall_tool_is_hybrid_when_embedder_bound`; `hybrid_recall_falls_back_without_embedding_and_ranks_with_vector` | store / tool / HTTP | 部分(スタブベクトル。本番 embed provider は未観測) |
| P-204 | `extract_names_as_shared_and_arbitrates`; `classifier_scope_defaults_private_when_missing` | Scripted | いいえ |
| P-205 | `extract_names_as_shared_and_arbitrates` (arbitrate) | Scripted | いいえ |
| P-206 | `decay_surfaces_low_salience_forgetting_candidates` | store | はい(減衰候補) |
| P-207 | `shared_pool_is_usable_by_another_soul_as_own_knowledge` | store | はい(プール契約) |
| P-208 | `extract_names_as_shared_and_arbitrates` | Scripted / 正規表現 | いいえ |
| P-209 | `runtime_persists_affect_across_turns`; `affect_decays_toward_baseline_but_trust_accumulates` | store | いいえ(トーン反映はキーワード) |
| P-210 | `forget_request_removes_matching_memory_and_records_journal` | store | はい |
| P-211 | `sensitive_candidate_queues_for_approval` | store | はい(承認待ちキュー) |
| P-212 | `memory_tools_surface_omits_write_shared` | store | はい(スキーマ) |
| P-301 | `self_report_updates_mood_label` | store | いいえ(本番分類未配線) |
| P-302 | `affect_decays_toward_baseline_but_trust_accumulates` | store | いいえ(一貫したトーンは未観測) |
| P-303 | `ene-body::emotion_always_emits_expression_even_without_body`; `lipsync_from_tone_has_amplitude` | Scripted | いいえ |
| P-304 | `expression_arbiter_suppresses_rapid_label_changes` | store | はい(ちらつき抑制) |
| P-305 | `runtime_persists_affect_across_turns` | store | はい(永続化) |
| P-306 | `schedule_remind_fires_and_quiet_hours_defer`; `proactive_gate_fail_closed_without_llm` | store / Scripted | 部分(スケジュールははい) |
| P-307 | `commands_never_include_pad_numbers`; `boot_seeds_two_souls_and_session_ops` (`/affect`) | store / HTTP | はい(PAD 非露出) |
| P-401 | `package_install_and_soul_creation`; `soul_and_body_packages_compose`; `import_shipped_alicia_vrm_exposes_parseable_avatar` | store / HTTP | 部分(I/O とはい。stage GUI 表示は手動) |
| P-402 | `hot_swap_drops_pending_cues`; `unknown_emotion_falls_back_with_warning` | store | はい |
| P-403 | `ene-vrm::minimal_glb_parses_as_vrm`; `minimal_glb_loads_with_wgpu`; `shipped_alicia_vrm_parses_and_loads`; `ene-stage::default_minimal_vrm_writes_parseable_glb` | Alicia / fixture | 部分(同梱 Alicia のパース/wgpu。GUI は手動) |
| P-404 | `lipsync_from_tone_has_amplitude`; `emotion_always_emits_expression_even_without_body`; `autonomy_tick_does_not_require_a_turn` | Scripted | いいえ |
| P-405 | `stage_caps_concurrent_rendered_bodies`; `boot_seeds_two_souls_and_session_ops`; `two_souls_keep_isolated_sessions_and_stage_occupants`; `overlay_slot_offsets_place_two_bodies_apart` | store / HTTP / overlay | 部分(2スロット配置。GUI E2E は手動) |
| P-406 | `hot_swap_drops_pending_cues` | store | はい |
| P-407 | `boot_stage_maps_emotion_without_a_rendered_body`; `emotion_always_emits_expression_even_without_body` | HTTP / store | はい(描画なしレーン) |
| P-501 | `turn_roundtrip_projects_history`; `seq_is_monotonic_without_gaps` | store | はい |
| P-502 | `model_visible_hash_matches_projection`; `model_visible_hash_matches_logged_projection` | store | はい |
| P-503 | `text_turn_is_logged_and_projected`; `tool_calling_model_runs_surface_tool_then_speaks`; `tool_calling_model_runs_calc_through_http` | Echo（機構） / ToolCalling | いいえ（実モデル簡易応答は未観測） |
| P-504 | `lane_prompt_still_works_while_job_running` | host API | いいえ(ランナー無し) |
| P-505 | `text_turn_is_logged_and_projected`(コンテキスト組立は固定文2本); `tool_calling_model_runs_calc_through_http` | Echo（機構） / ToolCalling | いいえ |
| P-506 | `compaction_replaces_range_in_projection`; `compact_keeps_original_rows` | store(切り詰め) | いいえ(要約 LLM 無し) |
| P-507 | `spill_huge_tool_output_keeps_brief_bounded` | store | はい |
| P-508 | `internal_delegation_has_no_job_row` | host API | はい(行契約) |
| P-509 | `grandchild_delegation_respects_depth_guard` | host API | はい(深さガード) |
| P-510 | `interrupt_recovery_and_tool_failure_use_different_wording` | host API | はい(文言) |
| P-511 | `mutating_work_waits_for_plan_approval` | store | はい |
| P-512 | `combined_child_questions_merge_and_route_answers` | host API | はい(結合契約) |
| P-515 | `crash_recovery_is_reported_and_not_resumed`; `boot_recovers_interrupted_turn_without_resume`; `job_persists_and_recover_does_not_resume`; `boot_reports_interrupted_job_without_resume` | store / HTTP | はい(非 resume) |
| P-516 | `usage_ledger_rows_are_append_only`; `usage_ledger_records_completed_turn` | store / HTTP | はい |
| P-517 | `observe_spans_do_not_leak_content`; `http_spans_and_schema_and_anon_health`; `ctl_client_lists_tools_and_debug_spans` | HTTP / CLI | はい |
| P-518 | `storage_too_new_is_rejected`; `older_storage_migrates_and_interrupts_open_work`; `recover_closes_open_turn_and_abandons_inbox` | store | はい |
| P-519 | `progress_and_complete_are_companion_speech`; `surface_message_and_cancel_while_running` | host API | いいえ(ランナー無し) |
| P-520 | `surface_projection_hides_inner_and_thinking`; `surface_ws_never_sees_inner`; `ene-stage::surface_blocks_inner_and_thinking` | store / HTTP | はい(表層) |
| P-521 | `progress_and_complete_are_companion_speech`; `completion_waits_for_user_speech_gap` | host API / Scripted | いいえ |
| P-522 | `surface_fs_write_upgrades_without_invoking`; `lane_auto_upgrade_does_not_execute_fs_write` | store | はい(機構。モデルは注入) |
| P-523 | `step_budget_upgrades_even_for_empty_side_effects`; `surface_router_upgrades_fs_write_without_spy` | store | はい(機構。モデルは注入) |
| P-524 | `proactive_speaks_when_gates_pass`; `classifier_scope_defaults_private_when_missing` | Scripted | いいえ |
| P-601 | `surface_schemas_omit_fs_write`; `harness_tool_uses_the_same_pipeline` | store | はい |
| P-602 | `builtin_specs_cover_five_plugins`; `out_of_process_utility_registers_and_runs` | launcher | いいえ(薄いスケッチ。`system_info` / 為替欠落) |
| P-603 | `handwritten_stdio_mcp_registers_and_runs` | process | はい |
| P-604 | `handwritten_stdio_mcp_git_status_runs_real_git` | process | はい |
| P-605 | `lane_prompt_still_works_while_job_running`; `progress_and_complete_are_companion_speech` | host API | いいえ |
| P-606 | `schedule_driver_delivers_remind_through_http`; `schedule_driver_defers_quiet_hours_and_fires_important`; `schedule_catch_up_does_not_start_missed_jobs`; `schedule_remind_fires_and_quiet_hours_defer`; `missed_remind_fires_once` | HTTP / store | はい |
| P-607 | `schedule_driver_delivers_remind_through_http` | HTTP | はい |
| P-608 | `bookmark_workflow_delivers_markdown_artifact` | host API | いいえ |
| P-609 | `bookmark_workflow_delivers_markdown_artifact`; `artifact_register_and_deliver` | host API | 部分(I/O ははい) |
| P-610 | `skill_install_and_load` | store | いいえ(コンテキスト未接続) |
| P-611 | `exec_tools_stay_off_surface_schema`; `exec_is_higher_risk_than_workspace_fs_write` | store | はい |
| P-612 | `handwritten_stdio_mcp_git_status_runs_real_git`; `exec_tools_stay_off_surface_schema` | process / store | いいえ(`status`/`log` のみ。fs/exec コーディング未達) |
| P-613 | `work_tools_cover_delegate_surface`; `plane_denies_side_effects_and_sensitive_reads` | store | はい(スキーマ) |
| P-614 | `spill_huge_tool_output_keeps_brief_bounded` | store | はい |
| P-615 | `work_tools_cover_delegate_surface` | store | 部分(呼称契約。発話は host API) |
| P-701 | `spawn_core_writes_api_json_and_health_succeeds` | process | はい |
| P-702 | `three_clients_share_one_core` | HTTP | はい |
| P-703 | `three_clients_share_one_core` (OpenAPI) | HTTP | はい |
| P-704 | `web_ui_cannot_mutate_memory_or_settings` (eframe/wgpu, WebView なし) | HTTP | はい |
| P-705 | `ctl_client_lists_tools_and_debug_spans`; `cli_binary_starts_core_and_runs_session_ops` | CLI | はい |
| P-706 | `web_ui_cannot_mutate_memory_or_settings`; `three_clients_share_one_core` | HTTP | 部分(閲覧 UX は JSON ダンプ) |
| P-707 | `boot_loads_settings_json_token_file`; `exclusive_mic_is_first_writer`; `approval_first_writer_wins` | HTTP | はい |
| P-708 | `http_spans_and_schema_and_anon_health` | HTTP | はい |
| P-709 | `backup_and_restore_roundtrip`; `backup_copies_stores`; `backup_restore_roundtrip_and_unknown_id` | HTTP / store | はい |
| P-712 | `surface_ws_never_sees_inner`; `export_default_omits_inner` | HTTP | いいえ(詳細画面 UX 未達。表層非露出ははい) |
| P-801 | `package_install_and_soul_creation` | store | 部分(表示・会話はいいえ) |
| P-802 | `soul_and_body_packages_compose` | store | はい |
| P-803 | `v3_json_imports_as_enechar` | store | はい |
| P-804 | `package_install_and_soul_creation` (export roundtrip) | store | はい |
| P-805 | `package_localizes_display_name_en_us_and_ja` | store | はい |
| P-806 | `package_rejects_unknown_format_and_bad_digest`; `manifest_digest_matches_python_plugin_contract` | store / process | はい |
| P-808 | `package_rejects_unknown_format_and_bad_digest` | store | はい |
| P-901 | `process_survives_os_sandbox_when_supported` | process | はい(Landlock 対応時) |
| P-902 | `undeclared_broker_op_is_denied`; `net_fetch_runs_after_grant` | store | 部分(未宣言は Denied。grant 後の `net.fetch` は SSRF 付きでホスト代行) |
| P-903 | `policy_allows_workspace_write`; `implicit_side_effect_without_popup_is_denied` | store | はい |
| P-904 | `ai_judgement_reason_is_audited` | Scripted | いいえ(本番 `ApproveModel = None`) |
| P-905 | `approval_first_writer_wins` | HTTP | はい |
| P-906 | `policy_add_requires_confirmation` | store | はい |
| P-907 | `vault_inject_ref_does_not_embed_plaintext`; `boot_installs_approval_plane_and_vault` | store / HTTP | はい |
| P-908 | `audit_hash_chain_verifies` | store | はい |
| P-909 | `spawned_core_offline_conversation_and_rss` | Echo | いいえ |
| P-910 | `observe_spans_do_not_leak_content` | store | はい |
| P-1001 | `out_of_process_utility_registers_and_runs` | process | はい |
| P-1002 | `apply_profile_unloads_removed_rows_and_keeps_uid` | process | はい |
| P-1003 | `tool_calling_model_runs_calc_through_http`; `seamed_model_rejects_unconfigured_chat`; Echo は `text_turn_is_logged_and_projected` の機構検出 | ToolCalling / Seamed fail-closed | いいえ（実 provider 会話は未観測） |
| P-1004 | `python_dummy_registers_in_registry_and_executes` | process | はい |
| P-1005 | `circuit_breaker_opens_after_spawn_failures`; `unload_removes_tools_and_grants` | process | はい |
| P-1006 | `sidecar_binary_resolves_config_then_cas_then_bundled_and_rejects_urls`; `sidecar_spawn_health_and_kill_on_loopback` | process | はい |
| P-1007 | `waterfall_rewrites_by_calling_next_and_emit_cannot`; `waterfall_pre_step_can_stop_the_model` | 整数単体 / host API | いいえ(プラグイン登録面無し) |
| P-1008 | `manifest_digest_matches_python_plugin_contract` | process | はい |
| P-1009 | `dummy_plugin_handshake_without_provider_subprotocol` | process(handshake) | いいえ(capability / バルク FD 未実装) |
| P-1010 | `requires_unsatisfied_row_waits_without_error`; `circular_requires_are_reported_and_rows_stay_inactive`; `disabling_one_fiber_does_not_restart_the_core` | process / HTTP | はい |

## 固定した不変条件(テスト)

- 表層 WS / 履歴 / default export に inner が乗らない。詳細の閲覧 UX は未達
- 中断は検出・片付け。自動 resume しない。作業継続ランナーは無い
- 同時 prompt は `lane_busy`。承認と mic は first-writer
- Web UI は memory/settings の PATCH/DELETE を持たない。stage は eframe+wgpu で WebView なし
- スパン属性にプロンプト内容が乗らない
- 空トークンは `/api/v1/health` 以外 `unauthorized`
- 会話 LLM の機構検出は EchoModel。ツール呼び出しの受入は
  `ToolCallingModel`（`utility.calc`）。`SeamedModel` は未設定で fail-closed。
  実モデルの分類・応答品質は総括 3 完了まで対象外

## 性能基準線

正は `minimal`(描画なし / EchoModel / debug)。数値と CI 上限は
[process-model.md §6.1](../platform/process-model.md#61-v10-基準線d-29)。

## 意図的にやらないこと(W7)

- `resume` / `lane.last_result` / OperationState / effect sandwich
- キャラ署名・marketplace・MCP 接続導線 UI

後継マイルストーン(M1–M9)と未達 v1.0 は混ぜない。未達 v1.0 は
[done.md](done.md) の `[ ]` と上表の完成=いいえ。後継は done.md の
「後継マイルストーン」表のみ。
