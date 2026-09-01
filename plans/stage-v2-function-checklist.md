# Stage v2 function checklist

Scope: `ene-stage`, `ene-stage-ui` (hand-written only), `ene-vrm`, `ene-api`, `ene-config`, `ene-tray-linux`, `ene-ctl::core`.
Work branch: `cursor/stage-exhaustive-tests-d099` from PR #1314 (`b214d83a`).

Walk rule: open the next unchecked item, test from multiple perspectives, then mark `[x]` with a result. Do not batch-check a file.
進捗: 1308 / 1308 checked (0 waiting GUI)


## `ene-stage` — `apps/ene-stage/src/app.rs`
- [x] `OverlayFocus::transition` (private, L80)
  - 役割: Set the chrome focus target (no change flag)
  - 視点: 正常系
  - 既存テスト: overlay_focus_tracks_chat_and_detail_transitions, focus_event_returns_changed_only_on_actual_transition
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::on_focus_event` (private, L84)
  - 役割: Dispatch Focused(true/false) per window owner; overlay gain clears, chrome gain sets, chrome loss starts grace
  - 視点: 正常系 / true/false
  - 既存テスト: window_focus_state_ignores_non_focus_events
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::set` (private, L124)
  - 役割: Claim a chrome target, cancel pending loss, return whether the target changed
  - 視点: 正常系 / true/false
  - 既存テスト: chat_waits_for_settings_before_checking_setup, chat_checks_setup_after_settings_loads, provider_asset_load_status_reports_success_and_empty_results, closing_chat_resets_chat_state_for_reopen, closing_detail_resets_visibility_and_focus, reopen_after_close_sets_open_intent, new_session_resets_stale_completion_terminal, stale_settings_save_cannot_overwrite_newer_status
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::mark_pending_loss` (private, L131)
  - 役割: Start focus-loss grace if a target exists; returns false so overlay sync does not run yet
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::expire_pending_loss` (private, L143)
  - 役割: Drop protection once a pending focus loss outlives its grace period
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::clear` (private, L154)
  - 役割: Drop target and pending loss; return whether a target was present
  - 視点: 正常系 / true/false
  - 既存テスト: retarget_clears_session_scoped_questions_and_approvals
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::clear_target` (private, L161)
  - 役割: Clear only when the current target matches; otherwise cancel pending loss
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::cancel_pending_loss` (private, L169)
  - 役割: Drop grace without dropping the current target
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayFocus::protects` (private, L174)
  - 役割: True while a chrome window currently owns overlay protection
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `run` (pub, L203)
  - 役割: Build StageApp and run the winit event loop
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: event loop boot in GUI smoke; cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::chrome_window_exists` (private, L458)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::global_cursor_to_physical` (private, L468)
  - 役割: Converts a screen-space pointer position to overlay-local physical coordinates using the overlay window's outer position. Returns `None` when the overlay window is gone.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_overlay_interaction` (private, L477)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::open_chat` (private, L562)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: Companion opens at boot in GUI smoke; cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::spawn` (private, L593)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_active_soul` (private, L607)
  - 役割: Load the active soul once so the Home readiness cards and the companion list reflect the live companion without first opening the Companion tab (#1177). The Companion tab re-issues
  - 視点: 正常系
  - 既存テスト: request_active_soul_enqueues_a_load_soul_outcome
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::claim_speaker_notify` (private, L618)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drain_async_results` (private, L645)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_async_outcome` (private, L656)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_listed_approvals` (private, L1461)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::set_pending_approval` (private, L1484)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::poll_pending_approvals` (private, L1513)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_memories` (private, L1533)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_characters` (private, L1572)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_jobs` (private, L1585)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_history_refresh` (private, L1600)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::begin_completion_reconciliation` (private, L1621)
  - 役割: Re-fetch history after a turn completes. The first refresh can still be stale because the projection lags the end event, so the fetch retries inside one task until authoritative hi
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::schedule_completion_refresh` (private, L1638)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::start_new_session` (private, L1657)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::toggle_mic` (private, L1675)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_stt_cta_after_settings_parse` (private, L1707)
  - 役割: A successful mic claim proves STT readiness on its own, but parked Voice-setup CTAs must also be disarmed as soon as effective settings show a non-placeholder provider, independent
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::send_chat` (private, L1713)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: send_chat_paints_the_optimistic_user_row_immediately
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::push_optimistic_user_row` (private, L1761)
  - 役割: Paints the user row before the HTTP send resolves; the composer keeps its editable draft (failure restores it), and a real assistant-era row from the next refresh supersedes this o
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::complete_optimistic_user_row` (private, L1767)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::discard_optimistic_user_row` (private, L1785)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::append_optimistic_user_row` (private, L1795)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::handle_avatar_reaction` (private, L1813)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::send_direct_reaction` (private, L1841)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_direct_reaction_retarget` (private, L1854)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::select_greeting` (private, L1866)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::answer_question` (private, L1885)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::barge_in` (private, L1910)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::cancel_turn` (private, L1929)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::respond_approval` (private, L1948)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::save_local_settings` (private, L1976)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::save_local_settings_with_status` (private, L1980)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::overlay_monitor_target` (private, L2012)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::record_overlay_monitor` (private, L2051)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::clamp_overlay_positions` (private, L2072)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_overlay_monitor` (private, L2107)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::refresh_monitor_inventory` (private, L2163)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::process_overlay_monitor_action` (private, L2177)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::sync_chrome_titles` (private, L2186)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::toggle_overlay_chrome` (private, L2201)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::raise_chrome` (private, L2216)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::dispatch_shell_command` (private, L2230)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::poll_shell` (private, L2245)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::process_surface_actions` (private, L2278)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::drain_surface_events` (private, L2307)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::seed_history_from_session` (private, L2317)
  - 役割: Seed an empty surface from the boot-time session snapshot. A turn that completed before the first paint leaves that snapshot empty until its refresh lands; seeding stays one-shot s
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::persist_body_position` (private, L2327)
  - 役割: Stores one body overlay position in the settings map. The active soul coordinates are also mirrored into the legacy scalar keys so hand-edited config files stay readable; reads sti
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::has_avatar_occupant` (private, L2341)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::visible_display_count` (private, L2347)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::include_soul_in_display` (private, L2358)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_display_action` (private, L2381)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::request_detail_and_overlay_redraw` (private, L2468)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_live_event` (private, L2480)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drain_detail_events` (private, L2597)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::spawn_listen` (private, L2641)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::poll_audio` (private, L2655)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::reload_avatar` (private, L2676)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::cycle_occupant` (private, L2780)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::active_companion_label` (private, L2794)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::chat_targets` (private, L2798)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::companion_label_for_soul` (private, L2810)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::overlay_motion_controls` (private, L2818)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_motion_command` (private, L2842)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::cycle_active_motion` (private, L2880)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::select_occupant` (private, L2902)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::commit_session_target` (private, L2941)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::open_detail` (private, L2963)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drop_focus_if_no_chrome` (private, L2994)
  - 役割: Clear focus protection only when no other chrome window can hold it.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_caption_window` (private, L3000)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::ensure_caption` (private, L3006)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::ensure_spotlight` (private, L3028)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::handle_overlay_key` (private, L3047)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::handle_overlay_shortcut` (private, L3063)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::abort_audio_playback` (private, L3081)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_expression_cue` (private, L3092)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::camera_basis` (private, L3106)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::overlay_hit_candidates` (private, L3119)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_cursor_moved` (private, L3140)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_pointer_moved` (private, L3144)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_press` (private, L3178)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_pointer_press_with_protection` (private, L3182)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_pointer_press` (private, L3226)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_release` (private, L3269)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::on_overlay_pointer_release` (private, L3273)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::cancel_overlay_pointer` (private, L3295)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::tick_overlay` (private, L3302)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_overlay_slint` (private, L3420)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::overlay_ui_request` (private, L3488)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageApp::rebuild_scene` (private, L3499)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::paint_chrome` (private, L3602)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_chat_slint` (private, L3686)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drain_chat_actions` (private, L3746)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_detail_slint` (private, L3788)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drain_detail_actions` (private, L3845)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::read_detail_drafts` (private, L3902)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::dispatch_detail_primary` (private, L3942)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::submit_new_job` (private, L3992)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::cancel_job` (private, L4026)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: cancel_job_other_errors_stay_raw
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::reload_mcp_tools` (private, L4041)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::apply_body_ref` (private, L4054)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::import_character_dialog` (private, L4073)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::start_character_import` (private, L4083)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::sync_spotlight_slint` (private, L4106)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::drain_spotlight_actions` (private, L4128)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::close_chat_window` (private, L4156)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::close_detail_window` (private, L4162)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::close_caption_window` (private, L4168)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageApp::close_spotlight_window` (private, L4173)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_send_block_reason` (private, L4180)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_window_action` (private, L4299)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for StageApp::resumed` (private, L4308)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageApp::window_event` (private, L4393)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: pointer/hover/drag in GUI smoke; cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageApp::about_to_wait` (private, L4590)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `auth_failure` (private, L4624)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `provider_asset_load_status` (private, L4631)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: provider_asset_load_status_reports_success_and_empty_results
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `window_level` (private, L4636)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: save_applies_window_level_for_transparent_and_opaque_overlays
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `overlay_window_level` (private, L4645)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `window_focus_state` (private, L4654)
  - 役割: Cyan AABB is drag-only
  - 視点: 正常系 / None / true/false
  - 既存テスト: window_focus_state_ignores_non_focus_events
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `should_repaint_after_event` (private, L4664)
  - 役割: Repaint when Slint marked the window dirty, or when a text field is focused so IME/paste is not deferred until the next pointer event.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_theme_overlay` (private, L4668)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_theme_chat` (private, L4672)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_theme_detail` (private, L4676)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_theme_caption` (private, L4680)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_theme_spotlight` (private, L4684)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_ene_theme` (private, L4688)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `user_theme_file` (private, L4700)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spotlight_entry_id` (private, L4704)
  - 役割: Repaint when Slint marked the window dirty, or when a text field is focused so IME/paste is not deferred until the next pointer event
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format_log_text` (private, L4719)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `disambiguated_occupant_label` (private, L4728)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `map_turn_err` (private, L4752)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `project_world_to_px` (private, L4762)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `world_aabb_to_px` (private, L4786)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `position_changed` (private, L4828)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `direct_reaction_expression` (private, L4835)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `direct_reaction_message` (private, L4843)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `create_denied_by_approval` (private, L4858)
  - 役割: Recognize the daemon's approval-pending rejection of job creation; only this error may stash a request for replay after its approval resolves.
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `friendly_create_job_error` (private, L4865)
  - 役割: Map a raw job-creation error to a user-facing reason, translating the approval-pending rejection instead of surfacing the raw `http 403` body.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `cancel_already_terminal` (private, L4876)
  - 役割: Core maps both `AlreadyCompleted` and `Cancelled` to this error class, so a Cancel click that races a finished job arrives as `http 409: already_completed: ...`.
  - 視点: 正常系 / true/false / 空
  - 既存テスト: cancel_already_terminal_matches_core_conflict_class
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `friendly_cancel_job_error` (private, L4882)
  - 役割: Replace the raw cancel 409 with a short status, keeping unrelated failures intact.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `request_active_soul_enqueues_a_load_soul_outcome` (private, L6279)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: request_active_soul_enqueues_a_load_soul_outcome
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mic_toggle_is_blocked_until_stt_is_configured` (private, L6309)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: mic_toggle_is_blocked_until_stt_is_configured
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/capture.rs`
- [x] `MicCapture::new` (pub, L26)
  - 役割: Open the default input device. `energy_threshold` overrides the barge-in RMS gate.
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::new_with_device` (pub, L31)
  - 役割: Open the configured input device, falling back to the system default when it is absent.
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::try_recv` (pub, L64)
  - 役割: Non-blocking receive of the next 16 kHz PCM chunk.
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::barge_in_active` (pub, L69)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `open_stream` (private, L74)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `select_device_by_name` (private, L133)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `list_input_device_names` (pub, L142)
  - 役割: Snapshot the names of currently available input devices for the settings UI.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_stream` (private, L158)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/dsp.rs`
- [x] `Resampler::new` (pub, L26)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Resampler::process` (pub, L34)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `rms` (pub, L62)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: rms_energy_computes_correctly
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `should_forward_mic` (pub, L73)
  - 役割: While TTS plays, drop frames quieter than speaker bleed. Idle frames (including silence) still go to core VAD so utterances can close.
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `push_coalesced` (pub, L80)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/listen.rs`
- [x] `MicListen::new` (pub, L38)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::generation` (pub, L43)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: stale_generation_does_not_drop_current_stream
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::start` (pub, L48)
  - 役割: Open a stream now (mic just claimed). Ignores retry backoff.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::release` (pub, L54)
  - 役割: Drop the sender and invalidate in-flight outcomes (mic released).
  - 視点: 正常系
  - 既存テスト: release_does_not_reconnect
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::on_done` (pub, L61)
  - 役割: After the listen task returns: clear if this generation is still current.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::poll` (pub, L75)
  - 役割: Reconnect when mic is claimed, the sender is gone, and backoff has elapsed.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::try_send` (pub, L92)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicListen::open` (private, L106)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/mod.rs`
- [x] `AudioHub::new` (pub, L39)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::new_with_mic_device` (pub, L44)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::set_mic_device` (pub, L63)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::list_input_device_names` (pub, L78)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::poll_mic_batches` (pub, L90)
  - 役割: Coalesced 16 kHz frames for the listen stream (~100 ms each).
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::play_pcm` (pub, L118)
  - 役割: Play mono/stereo-interleaved PCM at `sample_rate`.
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::stop` (pub, L131)
  - 役割: Stop playback and clear the viseme PCM buffer (no-op without `voice`).
  - 視点: 正常系
  - 既存テスト: stop_clears_recent_playback_pcm
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::playback_pcm` (pub, L140)
  - 役割: Recent playback PCM for lip-sync analysis.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: stop_clears_recent_playback_pcm
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::sample_rate` (pub, L152)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::is_tts_playing` (pub, L165)
  - 役割: Whether local TTS playback is still queued (echo-aware barge-in).
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::mic_barge_in` (pub, L178)
  - 役割: Whether mic energy exceeded the barge-in threshold.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioHub::analyze_visemes` (pub, L190)
  - 役割: Analyze playback audio into viseme weights (no-op without `voice`).
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for AudioHub::default` (private, L209)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `MicCapture::new` (pub, L237)
  - 役割: コンストラクタ
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::new_with_device` (pub, L245)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::try_recv` (pub, L253)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MicCapture::barge_in_active` (pub, L258)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::new` (pub, L271)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::play_pcm` (pub, L275)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::stop` (pub, L279)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: stop_clears_recent_playback_pcm
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::recent_pcm` (pub, L282)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::sample_rate` (pub, L287)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/playback.rs`
- [x] `Trait for AudioPlayback::default` (private, L27)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `AudioPlayback::new` (pub, L33)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::tts_playing_flag` (pub, L63)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::play_pcm` (pub, L67)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::stop` (pub, L96)
  - 役割: Drop queued PCM and stop the sink immediately (barge-in abort).
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::tick_playback` (pub, L105)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::is_tts_playing` (pub, L113)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::note_playback` (private, L118)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::recent_pcm` (pub, L135)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AudioPlayback::sample_rate` (pub, L140)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/audio/stream.rs`
- [x] `run_listen_stream` (pub, L9)
  - 役割: Forward coalesced 16 kHz frames until `rx` is closed (mic released).
  - 視点: 正常系 / 0 / 端 / HiDPI / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/avatar/collider_debug.rs`
- [x] `overlay_collider` (pub(crate), L29)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `overlay_sphere` (private, L69)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `overlay_capsule` (private, L95)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `node_world` (private, L141)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `push_overlay_collider` (private, L150)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `collider_debug_lines` (pub(crate), L174)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/avatar/look_at.rs`
- [x] `neutral_target` (pub, L15)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `compute_world_target` (pub, L19)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/avatar/mod.rs`
- [x] `CompanionAvatar::load` (pub, L57)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: companion_avatar_loads_the_minimal_fixture passed; minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed

- [x] `CompanionAvatar::format_version_label` (pub, L95)
  - 役割: Human-readable VRM dialect label ("VRM 0.x" / "VRM 1.0") for status surfaces that show which format the loaded avatar uses.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::load_motions` (pub, L99)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::motion_names` (pub, L119)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::current_motion` (pub, L124)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::select_motion_manually` (pub, L130)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_body_motion` (pub, L135)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::motion_is_manually_overridden` (pub, L143)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::select_motion_named` (private, L147)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::reset_motion` (pub, L155)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::stop_motion` (pub, L176)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::cycle_motion` (pub, L183)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::reset_pose_and_springs` (private, L194)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::load_motion_at` (private, L211)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_expression` (pub, L233)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_expression_cue` (pub, L241)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_expression_cue_weighted` (pub, L245)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::trigger_interaction_feedback` (pub, L258)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_expression_weighted` (private, L278)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::tick_expression_cue` (pub, L288)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::clear_expression_cue` (pub, L311)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_viseme` (pub, L320)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: silent_viseme_does_not_keep_the_overlay_dirty passed; cargo test -p ene-stage --lib (354 passed)

- [x] `CompanionAvatar::set_look_at_target` (pub, L324)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::apply_body_event` (pub, L328)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::camera` (pub, L382)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::head_world` (pub, L387)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::tick` (pub, L395)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::tick_idle` (private, L432)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::sample_motion` (private, L457)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::eval_look_at` (private, L486)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::step_springs` (private, L506)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::overlay_model_transform` (private, L523)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::world_aabb` (pub, L543)
  - 役割: World-space AABB of the rendered body for hit-testing. The render matrix subtracts the model center before scaling; applying it directly would double-subtract the center. Scaling t
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::overlay_bone_world` (pub, L556)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::part_world_aabb` (pub, L565)
  - 役割: Coarse CPU collider AABB for one body part. `None` when the bone is missing.
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::sphere_aabb` (private, L581)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::needs_redraw` (pub, L589)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::push_part_collider_wires` (pub(crate), L598)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `CompanionAvatar::fit_world_offset` (pub, L643)
  - 役割: Clamps a requested translation so the rendered AABB stays inside the camera viewport, accounting for the current model scale and aspect.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `push_interaction_outline` (pub(crate), L695)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `push_spring_collider_wires` (pub(crate), L722)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `debug_camera_uniform` (pub(crate), L735)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `render_to_texture` (pub, L739)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `write_default_minimal_vrm` (pub, L773)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `aabb_corners` (private, L779)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `project_ndc` (private, L792)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clamp_ndc_translation` (private, L801)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `empty_frame` (private, L809)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `node_maps` (private, L825)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `discover_motions` (private, L845)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/bundle.rs`
- [x] `pack_bundled_alicia` (pub, L27)
  - 役割: Build an Ene character archive from the repo-shipped Alicia body assets.
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pack_bundled_named` (pub, L32)
  - 役割: Same Alicia mesh under a second package id so two occupants can render.
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pack_alicia_from` (pub, L37)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: pack_alicia_from_missing_dir_errors, pack_alicia_from_embeds_vrm_and_motions
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pack_named_from` (pub, L41)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: pack_named_from_uses_requested_id
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pack_zip` (private, L132)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: pack_zip_has_local_file_magic
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `motions_dir_for_package` (pub, L154)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/chrome.rs`
- [x] `ChromeKind::title` (pub, L24)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::restore` (pub, L46)
  - 役割: Restore visibility even when the WM rejects programmatic focus.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::restore_or_create` (pub, L56)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: Companion restore at boot; lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::raise` (pub, L73)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::create` (pub, L79)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: Companion create at boot; lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::id` (pub, L141)
  - 役割: trivial getter
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::request_redraw` (pub, L145)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::sync_title` (pub, L149)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::owns_input` (pub, L154)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::composer_owns_keyboard` (pub, L159)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::place_caption` (pub, L163)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::on_window_event` (pub, L170)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::resize` (pub, L177)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: stale_surface_frame_is_skipped_during_resize
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::layer` (pub, L186)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::take_actions` (pub, L190)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeWindow::paint` (pub, L197)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: chrome_frame_needed_skips_unchanged_idle_windows passed; chrome paint in GUI smoke; cargo test -p ene-stage --lib (354 passed)

- [x] `minimum_inner_size` (pub, L246)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `surface_target_matches_window` (private, L257)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clear_color` (pub, L265)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `place_caption_window` (private, L277)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clamp_to_monitor` (private, L295)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clamp_window_axis` (private, L331)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: clamp_window_axis_keeps_the_full_window_on_screen
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/core/events.rs`
- [x] `EventFeeds::new_for_test` (pub(crate), L90)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_event_feeds` (pub, L102)
  - 役割: Spawn one socket per depth. Overlay/chat must only read `surface`.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_depth` (private, L125)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `event_socket_loop` (private, L137)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_surface_event` (private, L179)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_detail_event` (private, L192)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_live_event` (private, L196)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `string_field` (private, L348)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `f32_field` (private, L356)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `bool_field` (private, L363)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/core/session.rs`
- [x] `PreparedSessionTarget::session_id` (pub(crate), L67)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::client` (pub, L88)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::adopt_new_session` (pub, L92)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: adopt_new_session_switches_before_history_refresh
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::soul_id` (pub, L133)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::session_id` (pub, L138)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::turn_id` (pub, L143)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::clear_turn` (pub, L152)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::history` (pub, L157)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: history_is_normalized_oldest_to_newest, adopt_new_session_switches_before_history_refresh
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::greetings` (pub, L162)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::replace_history` (pub, L166)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::occupants` (pub, L171)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::avatar_path` (pub, L176)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: pick_avatar_occupant_prefers_avatar_path
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::motions_dir` (pub, L181)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::clone_handle` (pub, L186)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::bootstrap` (pub, L197)
  - 役割: Resolve a soul (preferring an occupant with an avatar), open a session, load history.
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::retarget_soul` (pub, L215)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::commit_retarget` (pub(crate), L221)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::avatar_loads` (pub, L233)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::send_prompt` (pub, L250)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::send_steer` (pub, L254)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::send_follow_up` (pub, L258)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::send` (private, L262)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::barge_in` (pub, L282)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::cancel_turn` (pub, L286)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::claim_mic` (pub, L295)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::release_mic` (pub, L299)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::claim_speaker` (pub, L303)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::claim_notify` (pub, L307)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::refresh_history` (pub, L311)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::select_greeting` (pub, L317)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::respond_approval` (pub, L325)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::get_soul` (pub, L333)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::patch_soul_body` (pub, L337)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::claim` (private, L348)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageSession::release` (private, L359)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::refresh_history` (pub, L365)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::select_greeting` (pub, L371)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::send` (pub, L376)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::answer_job` (pub, L400)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::barge_in` (pub, L413)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::cancel_turn` (pub, L417)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::respond_approval` (pub, L426)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::claim_mic` (pub, L434)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::release_mic` (pub, L438)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::claim_speaker` (pub, L442)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::claim_notify` (pub, L446)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SessionHandle::claim` (private, L450)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ensure_alicia` (pub, L463)
  - 役割: Import or activate Alicia so a stage occupant exposes `avatar_path`.
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ensure_avatar_occupants` (pub, L476)
  - 役割: Ensure up to `want` occupants expose a VRM `avatar_path`.
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_stage_with` (private, L485)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `import_named_companion` (private, L517)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `find_alicia_package` (private, L550)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `avatar_slots` (pub, L598)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: avatar_slots_caps_at_two_and_skips_text
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `motions_dir_for_occupant` (pub, L608)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `occupant_with_avatar` (pub, L621)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `occupant_has_avatar` (pub, L629)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `occupant_label` (pub, L637)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `next_avatar_occupant` (pub, L648)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pick_avatar_occupant` (pub, L671)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: pick_avatar_occupant_prefers_avatar_path
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_stage` (private, L675)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: resolve_stage_surfaces_missing_bundle
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_session_id` (private, L682)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `send_direct_interaction` (pub(crate), L700)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `prepare_soul_target` (pub(crate), L721)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_history` (pub, L745)
  - 役割: Keep the chat transcript in chronological order regardless of API ordering.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/core/spawn.rs`
- [x] `StageCore::detached` (pub, L23)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageCore::child` (pub, L31)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for StageCore::drop` (private, L37)
  - 役割: Drop
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `attach_or_spawn_core` (pub, L69)
  - 役割: Attach to an existing core or spawn `ene-core --data-dir`. Returns the API client and an optional child handle. When `desktop.core_lifetime` is `app`, the returned [`StageCore`] ki
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `stage_data_dir` (pub, L91)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: stage_data_dir_honors_ene_data_dir, stage_data_dir_default_is_not_repo_assets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `try_env_attach` (private, L98)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `try_data_dir_attach` (private, L115)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_core` (private, L140)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/cursor_poll.rs`
- [x] `spawn` (pub, L22)
  - 役割: Spawns a background thread that polls the X11 root-window pointer at a fixed interval. Returns `None` when the display is unavailable (non-X11, headless CI) so callers only need to
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_x11` (private, L35)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/detail/mod.rs`
- [x] `caption_position_label` (pub(crate), L84)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `theme_label` (pub(crate), L95)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `language_value_label` (pub(crate), L105)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `core_lifetime_label` (pub(crate), L115)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_profile_label` (pub(crate), L124)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `log_kind_label` (pub(crate), L147)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailTab::label` (pub, L172)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: detail_value_labels_do_not_expose_storage_ids
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailTab::keywords` (pub, L187)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailTab::matches_search` (pub, L234)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailTab::search_rank` (pub, L240)
  - 役割: Lower is a better match. `None` means the tab should be hidden.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `best_search_tab` (private, L270)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for MemoryCandidateDraft::from` (private, L311)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: shared_scope_is_detected_from_original_or_edit, search_vo_switches_from_home_to_voice
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for ScheduleBuilderState::default` (private, L493)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: default_provider_assets_skips_tool_plugins, local_timezone_name_never_panics_and_defaults_to_utc
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `ScheduleBuilderState::spec_for` (pub, L531)
  - 役割: Spec the core will receive in the current builder mode. Advanced mode returns an empty string; the raw text lives in the `new_schedule_spec` field of `DetailUiState` so power users
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format_interval_spec` (private, L579)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `cron_spec` (private, L587)
  - 役割: Render a 5-field cron spec. An empty weekday selection means no valid schedule exists yet, so the caller sees an empty spec instead of a silently always-fire pattern.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::push_log` (pub, L631)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::invalidate_settings` (pub, L642)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::settings_load_failed` (pub, L646)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::settings_loaded` (pub(crate), L650)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::begin_settings_load` (pub(crate), L654)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::finish_settings_load` (pub(crate), L662)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::set_session_id` (pub(crate), L666)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::refresh_settings_on_open` (pub, L672)
  - 役割: Reload core settings when Detail is reopened so external vault writes and restarts cannot leave a stale API-key banner behind.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::select_tab` (pub, L679)
  - 役割: Explicit navigation wins over the search box; otherwise search re-selects the tab every frame.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::next_activation_generation` (pub, L684)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::activation_is_current` (pub, L690)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::next_mcp_probe_generation` (pub, L694)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::mcp_probe_is_current` (pub, L700)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::invalidate_character` (pub, L704)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::invalidate_memory` (pub, L714)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DetailUiState::sync_candidate_drafts` (pub(crate), L723)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_core_fields` (pub, L750)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: parse_core_fields_reads_effective_tasks_and_profile
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `nested_string` (private, L818)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `nested_bool` (private, L829)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_title_mode` (pub, L841)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_approval_mode` (pub, L850)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mcp_args_text` (pub, L860)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `valid_mcp_id` (pub, L867)
  - 役割: Mirror of the daemon-side `valid_mcp_id` rule so the form can explain the constraint while typing instead of surfacing a raw HTTP 400 after Save.
  - 視点: 正常系 / true/false / 空
  - 既存テスト: valid_mcp_id_mirrors_daemon_rule
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mcp_id_suggestion` (pub, L879)
  - 役割: Sanitize a free-form display name into a valid MCP id token. Empty results mean the name had no usable characters and the user must type an id.
  - 視点: 正常系 / 空
  - 既存テスト: mcp_id_suggestion_sanitizes_display_names
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_mcp_args_text` (pub, L904)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_mcp_form` (pub, L912)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `validate_mcp_server` (pub, L920)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `validate_mcp_document` (pub, L943)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mcp_credential_row` (pub, L953)
  - 役割: Only persistent `mcp.<id>` rows keep credentials; probe rows are ephemeral and their ids are rejected by the daemon id rule anyway.
  - 視点: 正常系 / None / 空
  - 既存テスト: mcp_credential_rows_skip_probe_rows_and_extract_server_ids
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_mcp_credential_save` (pub, L963)
  - 役割: The credential is stored by probing the saved row with the new token; the daemon persists it before the connection attempt, so the response tells us whether the token was at least
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_provider_plugin_id` (pub, L1042)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `default_provider_assets_plugin` (pub, L1047)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_needs_key` (pub, L1059)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_is_local` (pub, L1064)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_has_fallback` (pub, L1071)
  - 役割: Plugins whose clients substitute a working default when a field is blank, so an empty form is functional rather than misconfigured.
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `provider_bool` (private, L1078)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `provider_display_name` (pub, L1089)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_provider_choices` (pub, L1109)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: chat_provider_choices_use_display_names_not_raw_ids
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `provider_choices_for_seam` (pub, L1144)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_setup_gap` (pub, L1177)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_setup_status` (pub, L1191)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_apply_block_reason` (pub, L1199)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `home_chat_next_step` (pub, L1216)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SetupCard::title` (private, L1238)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SetupCard::state_label` (private, L1242)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SetupCard::detail` (private, L1250)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: detail_value_labels_do_not_expose_storage_ids, reopening_detail_refreshes_stale_vault_state
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `home_status_cards` (pub(crate), L1272)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `setup_cards` (private, L1297)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `blocking_unconfigured` (pub, L1334)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `optional_unconfigured` (pub, L1343)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `list_models_status` (pub, L1352)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: list_models_status_prefers_error_then_empty_hint
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `filtered_provider_models` (pub, L1364)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: filtered_provider_models_keeps_apply_reachable_by_narrowing
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sync_search_tab` (pub, L1377)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `onboarding_visible` (pub(crate), L1387)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_character_active` (pub, L1397)
  - 役割: A package is the active companion when the soul it created or reused matches the currently active soul, so the UI can badge it instead of offering a redundant Activate action (#117
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `companion_display_rows` (pub(crate), L1415)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: companion_display_rows_keep_overlay_order_and_chat_badges_separate
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `companion_display_row` (private, L1455)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: companion_display_rows_keep_overlay_order_and_chat_badges_separate
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `title_mode_label` (pub(crate), L1537)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `observation_scope_text` (pub(crate), L1545)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `memory_kind_label` (pub(crate), L1556)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `memory_scope_label` (pub(crate), L1568)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `editable_schema_fields` (pub, L1637)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `begin_plugin_config_load` (pub, L1649)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_is_loading` (pub, L1665)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_values_empty` (pub, L1670)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_values_valid` (pub, L1678)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_load_is_current` (pub, L1683)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_plugin_config_view` (pub, L1692)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_request_is_current` (pub, L1703)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_config_status` (pub, L1707)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `request_overlay_monitor_action` (pub(crate), L1723)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `overlay_monitor_mode_label` (pub(crate), L1731)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `monitor_summary` (pub(crate), L1741)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `log_empty_copy` (pub(crate), L1759)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: log_empty_copy_is_resolved
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `approval_mode_label` (pub(crate), L1763)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ensure_settings` (pub(crate), L1855)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_ai_patch` (pub(crate), L1876)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `voice_settings_patch` (private, L1925)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_voice_patch` (pub(crate), L1971)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_observation_patch` (pub(crate), L1990)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `spawn_async` (private, L2019)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/detail/project.rs`
- [x] `project` (pub, L10)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `handle_select_tab` (pub, L35)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `handle_primary` (pub, L41)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `handle_row` (pub, L71)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/detail/tabs.rs`
- [x] `project_tab` (pub, L34)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `action` (private, L53)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `home` (private, L57)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `companion` (private, L88)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: companion_tab_applies_body_not_ai
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `conversation` (private, L122)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: conversation_tab_applies_ai
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `voice` (private, L149)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `memory` (private, L173)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: memory_and_connections_do_not_share_apply
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `work` (private, L197)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: work_tab_creates_a_job_instead_of_applying_ai
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `connections` (private, L222)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: memory_and_connections_do_not_share_apply
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `system` (private, L242)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `log` (private, L284)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/drag.rs`
- [x] `clamp_position` (pub, L17)
  - 役割: Clamps a normalized position into the valid overlay range.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: clamp_position_bounds_coordinates
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clamp_axis` (private, L24)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalized_to_world` (pub, L34)
  - 役割: Maps a normalized position to a world-space XY offset.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `world_to_normalized` (pub, L40)
  - 役割: Maps a world-space XY offset back to clamped normalized coordinates.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BodyDrag::soul_id` (pub, L63)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BodyDrag::grab_offset` (pub, L70)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: press_on_body_arms_with_grab_offset, dragging_keeps_grab_offset_between_moves
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `allows_input` (pub, L95)
  - 役割: Whether the overlay window should accept pointer input. An opaque overlay always accepts input. A transparent overlay with click-through preferred accepts input only when a chrome
  - 視点: 正常系 / true/false
  - 既存テスト: allows_input_combination_table
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `press_body` (pub, L108)
  - 役割: Arms a press on the hovered body, or clears any stale gesture when the background was pressed. Takes the body's saved normalized position; the grab offset keeps the cursor anchored
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `drag_body` (pub, L129)
  - 役割: Advances the gesture for a cursor-move event: the first move turns the armed press into a drag, and every move repositions only the dragged body while preserving its grab offset.
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `release_body` (pub, L158)
  - 役割: Ends the gesture, returning the soul whose position must persist.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `aabb_world_corners` (pub, L164)
  - 役割: Transforms the eight corners of a local AABB by `model_mat`.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `transformed_aabb_bounds` (pub, L182)
  - 役割: Computes the world-space bounds of a transformed local AABB.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ray_aabb_entry` (pub, L199)
  - 役割: Slab-method ray/AABB intersection returning the entry distance along the view ray. None when the ray misses or the box lies behind the origin.
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `cursor_logical_to_world_2d` (pub, L235)
  - 役割: Projects a window-logical cursor position to world XY on the camera's focal plane. Returns None for degenerate viewports.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `hit_test` (pub, L257)
  - 役割: Finds the frontmost body whose world AABB contains the logical cursor. Overlapping candidates resolve to the nearest along the view ray; equidistant hits prefer the later index, ma
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: hit_test_center_of_single_body, hit_test_misses_outside_corner, hit_test_respects_viewport_aspect, hit_test_prefers_nearest_body_on_overlap, hit_test_prefers_nearest_center_at_equal_depth
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/fonts.rs`
- [x] `cjk_font_candidates` (pub, L11)
  - 役割: Paths searched for a Japanese-capable UI font. Optional `assets/fonts/NotoSansJP-Regular.ttf` next to the binary is preferred when present; otherwise the OS CJK fonts (Yu Gothic /
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `os_cjk_font_paths` (private, L34)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `first_available_cjk_font` (pub, L80)
  - 役割: First existing CJK font path, if any. Slint/FemtoVG also resolve `Noto Sans CJK JP` via the platform fontconfig / DirectWrite stack.
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/gpu.rs`
- [x] `GpuContext::create` (pub, L31)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GpuContext::create_surface` (pub, L82)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GpuContext::surface_format` (pub, L89)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `configure_surface` (pub, L100)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `acquire_frame` (pub, L123)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `backend_options` (private, L136)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pick_alpha_mode` (pub, L154)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `alpha_mode_supports_transparency` (pub(crate), L178)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `create_depth` (pub, L186)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format_has_alpha` (private, L209)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/i18n.rs`
- [x] `loader_lock` (private, L19)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `should_report_missing` (private, L30)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `select_language` (pub, L38)
  - 役割: Apply a BCP-47 tag (`en-US`, `ja`). Empty string follows the OS default.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `fl` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: fl_resolves_app_title
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format` (pub, L72)
  - 役割: Resolve a message with named placeholders, e.g. `{ $tab }` in the FTL source.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for RestoreLanguage::drop` (private, L93)
  - 役割: Drop
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)







## `ene-stage` — `apps/ene-stage/src/interaction.rs`
- [x] `MoveResult::is_dragging` (pub, L32)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GestureTracker::press` (pub, L76)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: stationary_press_is_a_click, long_press_and_double_click_are_distinct, background_press_does_not_start_a_gesture
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GestureTracker::move_to` (pub, L103)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GestureTracker::release` (pub, L123)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GestureTracker::cancel` (pub, L164)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `GestureTracker::cancel_all` (pub, L177)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/interaction_controller.rs`
- [x] `Trait for StageInteractionController::default` (private, L65)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageInteractionController::mode` (pub, L77)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::ui_request` (pub, L82)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: clickable_ui_requests_interactive_entry
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::cursor_hittest_enabled` (pub, L88)
  - 役割: Whether the native window should receive pointer events.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::request_ui` (pub, L97)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::release_ui` (pub, L102)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::sync` (pub, L107)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::cancel` (pub, L112)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::apply_to_window` (pub, L137)
  - 役割: Apply the current mode to the window. Returns whether the OS call ran.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageInteractionController::recompute` (private, L171)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/main.rs`
- [x] `main` (private, L1)
  - 役割: binary / build entry
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/monitor.rs`
- [x] `OverlayMonitorMode::from_setting` (pub, L23)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayMonitorMode::setting` (pub, L33)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MonitorRect::right` (pub, L51)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MonitorRect::bottom` (pub, L56)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MonitorRect::contains` (pub, L61)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `union` (pub, L71)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: union_covers_negative_and_adjacent_monitor_coordinates
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MonitorInfo::rect` (pub, L96)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `stable_id` (pub, L112)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `stable_id_from_parts` (pub, L118)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `inventory` (pub, L126)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `union_rect` (pub, L183)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `find_saved_monitor` (pub, L191)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_target` (pub, L219)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `saturating_i32` (private, L271)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `saturating_u32` (private, L281)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/overlay.rs`
- [x] `OverlayWindow::create` (pub, L61)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::id` (pub, L111)
  - 役割: trivial getter
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::format` (pub, L116)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::set_slint_layer` (pub, L120)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::slint_layer` (pub, L124)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::slint_layer_mut` (pub, L128)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::has_avatars` (pub, L133)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::first_avatar` (pub, L138)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::first_avatar_mut` (pub, L142)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::avatar_mut` (pub, L146)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::avatar_or_first_mut` (pub, L153)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::reset_visemes` (pub, L161)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::resize` (pub, L168)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::set_click_through` (pub, L176)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::apply_click_through` (pub, L183)
  - 役割: Chrome on (decorations visible) always hit-tests so Allow/Detail work. Chrome off restores the saved click-through preference. Hit-test OS calls go through [`crate::interaction_con
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::toggle_chrome` (pub, L187)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::clear_avatars` (pub, L197)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::load_avatars` (pub, L201)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::set_interaction_targets` (pub, L251)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayWindow::tick_and_render` (pub, L262)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: silent viseme + overlay skip GPU when clean; minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_one` (private, L314)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/platform/mod.rs`
- [x] `apply_overlay_hints` (pub, L27)
  - 役割: Best-effort overlay window hints (Linux layer-shell / input region). winit 0.30 does not expose layer-shell; click-through uses [`winit::window::Window::set_cursor_hittest`] on Win
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_click_through` (pub, L35)
  - 役割: Apply click-through to the native window when supported. Production hit-test goes through [`OverlayPlatform`]. This helper is the leftover HWND EXSTYLE path and is not used by the
  - 視点: 正常系 / None / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `global_cursor_position` (pub, L51)
  - 役割: Returns the global pointer position when the platform exposes a native query. Linux uses the existing X11 polling channel instead.
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_click_through_windows` (private, L68)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `preferred_data_dir` (pub, L97)
  - 役割: Preferred persistent data directory for stage-local state.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayPlatform::attach` (pub, L125)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayPlatform::name` (pub, L137)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayPlatform::apply` (pub, L142)
  - 役割: Map controller mode + scene geometry onto the OS hit-test path.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayPlatform::commit_region_push` (private, L195)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OverlayPlatform::should_push_region` (private, L201)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PlatformKind::name` (private, L216)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `detect_backend` (private, L231)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `rects_i32` (pub(crate), L258)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `physical_to_surface_local` (pub(crate), L268)
  - 役割: Convert physical-pixel scene rects into Wayland surface-local coordinates. `wl_surface.set_input_region` is surface-local. `winit` `inner_size` and [`PxRect`] are physical pixels,
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/platform/wayland.rs`
- [x] `WaylandRegion::try_new` (pub, L37)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `WaylandRegion::apply` (pub, L90)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for AppData::event` (private, L128)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: Wayland region path; platform unit tests; cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for AppData::event` (private, L149)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: Wayland region path; platform unit tests; cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for AppData::event` (private, L161)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: Wayland region path; platform unit tests; cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for AppData::event` (private, L173)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: Wayland region path; platform unit tests; cargo test -p ene-stage --lib (354 passed)







## `ene-stage` — `apps/ene-stage/src/platform/windows.rs`
- [x] `apply_mode` (pub, L7)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/platform/x11.rs`
- [x] `X11Shape::try_new` (pub, L24)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `X11Shape::apply` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `X11Shape::set_kind` (private, L79)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/renderer/compositor.rs`
- [x] `vs_main` (private, L37)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PremulCompositor::new` (pub, L69)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PremulCompositor::target_format` (pub, L166)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PremulCompositor::bind_ui_texture` (pub, L170)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PremulCompositor::encode` (pub, L187)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PremulCompositor::draw` (pub, L198)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/renderer/mod.rs`
- [x] `StageRenderer::new` (pub, L32)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::format` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::config` (pub, L59)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::size` (pub, L64)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::resize` (pub, L68)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::reconfigure_surface` (pub, L82)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::ensure_ui_target` (private, L91)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageRenderer::render` (pub, L123)
  - 役割: Draw VRM (and optional Slint overlay) onto the swapchain.
  - 視点: 正常系 / None / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/renderer/slint_gpu.rs`
- [x] `Trait for StageSlintPlatform::create_window_adapter` (private, L41)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageSlintPlatform::duration_since_start` (private, L53)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageSlintPlatform::set_clipboard_text` (private, L57)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageSlintPlatform::clipboard_text` (private, L61)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `StageWindowAdapter::try_new` (private, L74)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageWindowAdapter::take_redraw` (private, L88)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for StageWindowAdapter::window` (private, L95)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageWindowAdapter::size` (private, L99)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageWindowAdapter::renderer` (private, L103)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageWindowAdapter::set_visible` (private, L107)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / エラー / true/false
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for StageWindowAdapter::request_redraw` (private, L111)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `install` (pub, L123)
  - 役割: Install the custom Slint platform that shares the Stage wgpu device.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `take_last_adapter` (private, L143)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::new` (pub, L167)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::size` (pub, L180)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::ensure_component` (pub, L184)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::component` (pub, L209)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::set_choices` (pub, L213)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::take_actions` (pub, L227)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::dispatch_winit` (pub, L231)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::needs_redraw` (pub, L245)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SlintOverlayLayer::render` (pub, L252)
  - 役割: Draw the current overlay UI into `target`.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::chat` (pub, L327)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::detail` (pub, L370)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::caption` (pub, L401)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::spotlight` (pub, L415)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::chat_ui` (pub, L440)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::detail_ui` (pub, L447)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::caption_ui` (pub, L454)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::spotlight_ui` (pub, L461)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::take_actions` (pub, L468)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::input_focused` (pub, L472)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::dispatch_winit` (pub, L480)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ChromeLayer::render` (pub, L504)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `dispatch_to_window` (private, L538)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `convert_window_event` (private, L561)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `classify_ime` (private, L641)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `dispatch_ime` (private, L668)
  - 役割: Public [`WindowEvent`] has no IME variants. Slint's own winit backend feeds preedit/commit through `WindowInner::process_key_input` as `UpdateComposition` / `CommitComposition`.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_modifier_key` (private, L690)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `forward_keyboard` (private, L704)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `modifier_key_events` (private, L708)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `key_text` (private, L732)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_owned_clipboard` (private, L773)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_owned_clipboard` (private, L791)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `with_owned_clipboard` (private, L802)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_selection_clipboard` (private, L814)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_selection_clipboard` (private, L825)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_selection_clipboard` (private, L828)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_selection_clipboard` (private, L838)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/scene.rs`
- [x] `PxRect::new` (pub, L20)
  - 役割: コンストラクタ
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PxRect::is_empty` (pub, L25)
  - 役割: trivial getter
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PxRect::contains` (pub, L30)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PxRect::padded` (pub, L38)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PxRect::max_extent_delta` (pub, L48)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisualPrimitive::id` (pub, L77)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: hidden_ui_drops_from_both_geometries
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisualPrimitive::visual_rect` (pub, L85)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `InteractionPrimitive::id` (pub, L128)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: hidden_ui_drops_from_both_geometries
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `InteractionPrimitive::os_input_rect` (pub, L136)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `InteractionPrimitive::hit_rect` (pub, L148)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::set_anchors` (pub, L194)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::anchors` (pub, L202)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::new` (pub, L209)
  - 役割: コンストラクタ
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::set_visuals` (pub, L213)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::set_interactions` (pub, L220)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::hide` (pub, L227)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::show` (pub, L235)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::overlay_ui_flags` (pub, L244)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::is_dirty` (pub, L263)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::take_dirty` (pub, L268)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::is_hidden` (pub, L275)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::visual_geometry` (pub, L281)
  - 役割: Drawn footprint, including effect padding. Hidden ids are omitted.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::interaction_geometry` (pub, L295)
  - 役割: Pointer footprint sent to OS backends. Display-only and hidden ids are omitted.
  - 視点: 正常系
  - 既存テスト: visual_only_effect_does_not_enter_interaction_geometry
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `StageScene::hit` (pub, L308)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: empty_geometry_hits_none
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `InteractionGeometry::is_empty` (pub, L345)
  - 役割: trivial getter
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `InteractionGeometry::within_threshold` (pub, L351)
  - 役割: True when every rect moved less than `threshold` px vs `previous`.
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/settings.rs`
- [x] `load_desktop_settings` (pub, L49)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `save_desktop_settings` (pub, L64)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `clamp_model_scale` (pub, L86)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `effective_model_scale` (pub, L95)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `default_position_for` (pub, L103)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `arranged_positions` (pub, L112)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: arranged_positions_keep_one_body_centered_and_two_separated
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `seed_character_positions` (pub, L134)
  - 役割: Ensures every loaded soul has an entry in `character_positions`. The active soul inherits the legacy scalar keys; other bodies start on the left side so two freshly-seeded bodies d
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mirror_active_position` (pub, L155)
  - 役割: Mirrors the active soul's per-body position into the legacy scalar fields so hand-edited config files stay readable. Reads still prefer the map.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_displayed_souls` (pub(crate), L162)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ordered_visible_souls` (pub(crate), L189)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/shell/hotkeys.rs`
- [x] `HotkeyRegistration::initial` (private, L19)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: failed_initial_stays_inactive_until_a_late_retry_succeeds
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HotkeyRegistration::retry` (private, L26)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / true/false
  - 既存テスト: failed_initial_stays_inactive_until_a_late_retry_succeeds
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HotkeyManager::new` (pub, L54)
  - 役割: Best-effort registration: skips hotkeys that are already taken.
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HotkeyManager::retry_spotlight` (private, L71)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HotkeyManager::poll` (pub, L81)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HotkeyManager::spotlight_active` (pub, L96)
  - 役割: Whether Alt+Space is currently registered, including successful retries.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/shell/mod.rs`
- [x] `init_tracing` (pub, L25)
  - 役割: Initialize tracing for the stage binary.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/shell/notify.rs`
- [x] `show_notification` (pub, L13)
  - 役割: Show a desktop notification with an optional hint category.
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_category_hint` (private, L24)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_category_hint` (private, L31)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/shell/tray.rs`
- [x] `TrayManager::new` (pub, L35)
  - 役割: Build the tray icon and wire menu events into an internal channel.
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `TrayManager::try_recv` (pub, L98)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `TrayManager::take_interactions` (pub, L114)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `TrayManager::set_mic_active` (pub, L125)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `map_linux_event` (private, L139)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `map_menu_id` (private, L149)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mic_menu_label` (private, L160)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: mic_menu_label_is_the_next_action
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_icon_rgba` (private, L165)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_icon_bytes` (private, L177)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `synthetic_icon_rgba` (private, L185)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `WindowsTrayBackend::new` (pub(super), L213)
  - 役割: コンストラクタ
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `WindowsTrayBackend::try_recv` (pub(super), L235)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `WindowsTrayBackend::take_interactions` (pub(super), L239)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `WindowsTrayBackend::set_mic_active` (pub(super), L247)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_menu` (private, L253)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_icon` (private, L297)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `poll_tray_events` (private, L302)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/surface/caption.rs`
- [x] `outer_offset` (pub, L10)
  - 役割: Native-window offset inside a monitor, in physical pixels from the top-left.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_speech_caption` (pub(crate), L29)
  - 役割: True when `text` is spoken reply content, not a kernel provider-failure marker.
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `speech_text` (pub, L38)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/surface/chat.rs`
- [x] `normalize_transcript` (pub(crate), L37)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_message` (private, L56)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `transcript_label` (pub(crate), L72)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `request_single_greeting_commit` (pub(crate), L84)
  - 役割: A lone canonical greeting commits as soon as the picker renders; guard against re-queueing while the selection is already pending or in flight.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `composer_send_allowed` (pub(crate), L144)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `chat_setup_cta_eligible` (pub(crate), L182)
  - 役割: The chat-setup CTA may not piggyback on generic status text nor crowd out live conversation rows or a visible greeting picker; only a dedicated setup gap over a quiet panel may sho
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/surface/mod.rs`
- [x] `Trait for SurfaceUiState::default` (private, L97)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `SurfaceUiState::push_action` (pub, L138)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::close_chat` (pub, L147)
  - 役割: Closing the Chat window removes the redraw that would normally clear input focus, so the flag must be reset alongside `chat_open`.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::chat_window_exists` (pub, L154)
  - 役割: Whether a chat window should currently be kept alive.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::begin_send` (pub(crate), L159)
  - 役割: A new turn supersedes the previous turn's composer feedback.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::apply_text_delta` (pub(crate), L164)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::on_turn_ended` (pub(crate), L179)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::dismiss_caption` (pub(crate), L186)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SurfaceUiState::caption_visible` (pub(crate), L192)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/surface/spotlight.rs`
- [x] `SpotlightAction::dismisses_palette` (pub, L17)
  - 役割: Every palette action leaves the destination in front; the palette must not stay open behind it.
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SpotlightEntry::command` (private, L40)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: every_quick_command_dismisses_the_palette
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SpotlightEntry::close` (private, L50)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SpotlightEntry::rank` (private, L61)
  - 役割: Lower is a better match. Mirrors `DetailTab::search_rank` so both search boxes behave identically.
  - 視点: 正常系 / None / 空
  - 既存テスト: rank_order_prefix_before_contains_before_keyword
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `palette_entries` (pub, L82)
  - 役割: All actions the palette can run, in display order.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `filter_entries` (pub, L114)
  - 役割: Same ordering rule as `DetailTab::search_rank`: label prefix beats label substring beats keyword hit.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage` — `apps/ene-stage/src/ui/theme.rs`
- [x] `load_user_theme` (pub, L24)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_to_global` (pub, L31)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_color` (private, L49)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage-ui` — `crates/ene-stage-ui/build.rs`
- [x] `main` (private, L1)
  - 役割: binary / build entry
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/animation.rs`
- [x] `Interpolation::from_gltf` (private, L75)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `T::duration` (pub, L101)
  - 役割: Duration of this sampler (last timestamp, or 0 if empty).
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: sampler_duration_from_last_timestamp, sampler_duration_empty_is_zero
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `T::keyframe_count` (pub, L105)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: sampler_keyframe_count
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for VrmaPlayer::default` (private, L236)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: vrma_player_default_state
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `VrmaPlayer::play` (pub, L248)
  - 役割: Start or resume playback.
  - 視点: 正常系
  - 既存テスト: vrma_player_default_state, player_advance_loop_wraps, player_advance_once_stops_at_end, player_stop_resets_time, player_seek_clamps_negative, player_speed_multiplier, player_paused_does_not_advance
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmaPlayer::pause` (pub, L253)
  - 役割: Pause playback (preserves current time).
  - 視点: 正常系
  - 既存テスト: player_paused_does_not_advance
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmaPlayer::stop` (pub, L258)
  - 役割: Stop playback and reset to the beginning.
  - 視点: 正常系
  - 既存テスト: player_advance_once_stops_at_end, player_stop_resets_time
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmaPlayer::seek` (pub, L264)
  - 役割: Seek to a specific time in seconds.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: player_seek_clamps_negative
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmaPlayer::advance` (pub, L272)
  - 役割: Advance the playback clock by `dt` seconds. `duration` is the clip's total duration. The method handles looping and one-shot termination.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: player_advance_loop_wraps, player_advance_once_stops_at_end, player_paused_does_not_advance
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_scalar` (private, L298)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: sample_scalar_step, sample_scalar_linear
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_vec3` (private, L302)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: sample_vec3_linear
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_quat` (private, L306)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: sample_quat_linear_slerp, sample_quat_step
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_step` (private, L327)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `find_keyframe_index` (private, L345)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_keyframes` (private, L360)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_cubic_spline` (private, L395)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sample_cubic_spline_quat` (private, L431)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `retarget_rotation` (pub, L474)
  - 役割: Retarget a rotation from source skeleton to destination skeleton using the VRM spec's NormalizedLocalRotation formula. ```text NLR = W_src * L_src^-1 * src_pose * W_src^-1 dst_pose
  - 視点: 正常系
  - 既存テスト: retarget_rotation_identity_is_identity
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `retarget_hips_translation` (pub, L496)
  - 役割: Retarget hips translation using height-based scaling. ```text delta = src_pose - src_rest_local scale = dst_hips_height / src_hips_height result = dst_rest_local + delta * scale ``
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: retarget_hips_translation_no_scale_when_equal_height, retarget_hips_translation_scales_by_height_ratio
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `quat_to_yaw_pitch` (pub, L521)
  - 役割: Decompose a quaternion into yaw/pitch using Extrinsic ZXY order. The VRMA spec says: Y rotation = yaw, X rotation = pitch. Yaw positive = model looks left, pitch positive = looks d
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: quat_to_yaw_pitch_identity
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `evaluate_clip` (pub, L532)
  - 役割: Evaluate a VRMA clip at time `t` and produce a [`VrmaFrame`]. The frame contains raw (un-retargeted) bone rotations and expression weights. The consumer is responsible for retarget
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: evaluate_clip_empty, evaluate_clip_samples_bone_rotation, evaluate_clip_clamps_expression_weight, evaluate_clip_look_at
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `evaluate_retargeted` (pub, L573)
  - 役割: Evaluate a VRMA clip and retarget onto a destination humanoid rest pose. Rotations use the VRM `NormalizedLocalRotation` formula. Hips translation uses `dst_rest_local + (src_pose
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: evaluate_retargeted_hips_are_dest_local_not_world_additive, evaluate_retargeted_y_only_does_not_shift_x
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `retarget_hips_to_dest` (private, L624)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_vrma` (pub, L653)
  - 役割: Load a `.vrma` file from disk and parse it into a [`VrmaAsset`]. A VRMA file is a standard glTF/GLB binary with the `VRMC_vrm_animation` extension. The function reads the semantic
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_vrma_properties` (private, L698)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `compute_node_rest_transforms` (private, L765)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_animation` (private, L824)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/beat_sync.rs`
- [x] `Trait for BeatSway::default` (private, L45)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: default_sway_is_inactive_and_leaves_frame_untouched
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `BeatSway::on_pulse` (pub, L63)
  - 役割: Register a detected beat. The phase snaps to the sine peak (π/2) so the sway's maximum rotation lands exactly on the beat; the snap coincides with the intensity attack, which masks
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BeatSway::update` (pub, L73)
  - 役割: The phase advances at the current BPM; the intensity decays toward zero so the body settles between beats and fully rests after the pulse tail.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BeatSway::is_active` (pub, L83)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BeatSway::locomotion_speed_multiplier` (pub, L90)
  - 役割: Scales the clip toward the reference tempo (120 BPM); returns `1.0` once the sway tail expires so a dead beat never freezes the avatar at the last detected tempo.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `BeatSway::apply_to` (pub, L100)
  - 役割: Bones already posed by the clip keep their rotation (sway multiplies on top); unposed bones gain a sway-only rotation, so the reaction works without any motion asset.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/camera.rs`
- [x] `Trait for OrthographicCamera::default` (private, L38)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `OrthographicCamera::set_aspect` (pub, L50)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `OrthographicCamera::look_at` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `eye` (pub, L59)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `target` (pub, L63)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `debug` (pub, L68)
  - 役割: Diagnostic: returns `(eye, target, viewport_height, aspect)`.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `compute_auto_fit_scale` (pub, L84)
  - 役割: Compute the scale factor that makes an axis-aligned bounding box of the given extents fit the current orthographic viewport, leaving `margin` of the viewport unused on every side.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `uniform` (pub, L98)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `debug_view` (pub, L120)
  - 役割: Diagnostic: just the view matrix (the `look_at` rotation+translation). Exposed so the runtime can dump it without having to plumb the private eye/target/up fields out to the deskto
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `debug_proj` (pub, L127)
  - 役割: Diagnostic: just the orthographic projection matrix. Used by `runtime.rs` to verify the projection side isn't the source of a height-related bug.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for ModelUniform::default` (private, L165)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `ModelUniform::from_position_scale` (pub, L184)
  - 役割: Build a model matrix from a translation (world units) and a uniform scale. No rotation is applied: `VRoid` (Alicia) and other VRM 1.0 humanoid models are exported with their **face
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `from_mat4` (pub, L202)
  - 役割: Wrap an already-composed `Mat4` (e.g. one built by `CharacterRenderer::model_matrix` that folds the loader's `T(-center) * S(normalize_scale)` in alongside the character's translat
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pixel_to_ndc` (pub, L218)
  - 役割: Convert a window-pixel coordinate to normalised device coordinates (Y flipped to match the NDC convention, since winit's cursor origin is top-left and NDC's is bottom-left).
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: pixel_to_ndc_centres_and_flips_y
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ndc_to_view_pos` (pub, L224)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: ndc_to_view_pos_scales_by_viewport_extents, ndc_to_view_pos_viewport_agrees_with_aspect
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ndc_to_view_pos_with_aspect` (pub, L235)
  - 役割: Convert NDC to a view-space position on the orthographic camera's near plane, using a pre-computed `aspect`. The `view_z` parameter lets the caller choose where along the view axis
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `view_pos_to_world` (pub, L242)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: view_pos_to_world_round_trips_through_view
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/debug_renderer.rs`
- [x] `DebugLine::vertices` (private, L79)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: debug_line_vertices_match_endpoints
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `sphere_wireframe_lines_into` (pub, L96)
  - 役割: The sphere is composed of `SPHERE_LONGITUDES` meridians plus `SPHERE_LATITUDES + 1` latitude rings, each broken into `SPHERE_LONGITUDES` segments. All lines share `color`.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `capsule_wireframe_lines_into` (pub, L161)
  - 役割: The capsule is a cylinder of `half_height` capped by two hemispheres of `radius`. Wireframe elements: - top cap (a circle of `radius` at y = +`half_height`), - bottom cap (a circle
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `cross_lines` (pub, L217)
  - 役割: Used to mark the raycast hit point so the precise impact location is obvious.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: cross_lines_produces_three_segments_one_per_axis
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::new` (pub, L271)
  - 役割: The depth format is fixed at `Depth32Float` to match the main VRM pass; mixing depth formats would cause the GPU to reject the depth attachment.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::clear` (pub, L379)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::line_count` (pub, L383)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::push_line` (pub, L387)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::push_sphere_wireframe` (pub, L391)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::push_capsule_wireframe` (pub, L397)
  - 役割: `orientation` rotates the capsule's local +Y to the world direction the bone's "toward-child" axis points in (identity for trunk bones).
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::push_cross` (pub, L417)
  - 役割: The cross is drawn at the raycast hit point to make the precise impact location obvious.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `DebugRenderer::render` (pub, L429)
  - 役割: The depth attachment is `LoadOp::Load` (preserves the model's depth); the pipeline's `CompareFunction::Always` depth test draws the wires on top of the model. `camera_uniform` is u
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/expression.rs`
- [x] `Trait for PrimitiveMorphMeta::default` (private, L111)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: morph_meta_default_is_all_zeros
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `ExpressionName::new` (pub, L163)
  - 役割: Wrap a `&str` in an `ExpressionName`. The string is preserved verbatim — case is **not** normalized here; the loader lower-cases names from the `VRMC_vrm.expressions` ext object.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionName::as_str` (pub, L167)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for ExpressionName::fmt` (private, L173)
  - 役割: Display/Debug
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for ExpressionName::from` (private, L179)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `PrimitiveMorphs::from_targets` (pub, L214)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::new` (pub, L263)
  - 役割: Build a fresh layer from per-primitive data. The model has zero expressions if `per_primitive` is empty.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::expression_names` (pub, L285)
  - 役割: Sorted list of every expression name defined on **any** primitive of the model. De-duplicated. Used by the settings UI's "Manual Expressions (Test)" buttons and by the AI bridge to
  - 視点: 正常系 / 空
  - 既存テスト: expression_names_dedupes_across_primitives
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::set_expression` (pub, L297)
  - 役割: Set a single expression weight. Names not present on the model are **not** stored in `weights`; the call returns `false` so the caller can detect the miss. Names that exist on at l
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: set_expression_clamps_weight, set_expression_unknown_name_returns_false_and_is_not_stored
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::apply_weights` (pub, L310)
  - 役割: Apply every (name, weight) pair in `incoming`, replacing the current weights map. Names that are not in the model's expression list are **dropped** so the weight map cannot grow wi
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: apply_weights_overwrites, apply_weights_drops_unknown_names
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::morphic_primitive_count` (pub, L324)
  - 役割: Number of primitives that have at least one morph target.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::apply_viseme_weights` (pub, L337)
  - 役割: Map audio-driven [`VisemeWeights`](crate::viseme::VisemeWeights) onto the five procedural mouth targets (`aa`, `ih`, `ou`, `ee`, `oh`) via [`set_expression`](Self::set_expression).
  - 視点: 正常系
  - 既存テスト: apply_viseme_weights_maps_all_five_targets, apply_viseme_weights_clamps_and_drops_unknown
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/expression_compositor.rs`
- [x] `ExpressionCompositor::load_card_expression` (pub, L38)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::set_active_expression` (pub, L44)
  - 役割: Pass `None` to clear the selection.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::set_override` (pub, L50)
  - 役割: Overrides take precedence over the active card expression for the same blend-shape key.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::remove_override` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::clear_overrides` (pub, L58)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: clear_overrides_restores_base
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::clear_card_expressions` (pub, L62)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::compose` (pub, L69)
  - 役割: Override keys win over card keys. All weights are clamped to `[0, 1]`.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: base_only_composes_card_weights
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::active_expression_name` (pub, L87)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionCompositor::loaded_expression_names` (pub, L91)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: loaded_expression_names_returns_all
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/expression_override.rs`
- [x] `is_procedural` (pub, L66)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: is_procedural_recognises_all_targets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionOverrideType::from_json_str` (pub, L90)
  - 役割: Parse from a VRM JSON string (`"none"`, `"block"`, `"blend"`). Unknown strings silently fall back to `None` (the spec default) so a malformed file does not blow up the loader.
  - 視点: 正常系 / 空
  - 既存テスト: override_type_from_json_str_block, override_type_from_json_str_blend, override_type_from_json_str_none, override_type_from_json_str_unknown_falls_back_to_none
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionDefinition::new` (pub, L139)
  - 役割: Build a definition with spec-default overrides (`none` everywhere, `is_binary = false`). Callers that parsed a partial JSON block use this as the base and then override individual
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_overrides` (pub, L181)
  - 役割: Evaluate the `isBinary` clamp and the `overrideMouth/Blink/LookAt` block / blend semantics against `weights`, using the per-expression definitions. The function mutates `weights` i
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: apply_overrides_on_direct_weight_map
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides` (pub, L285)
  - 役割: Walk the `VRMC_vrm.expressions.{preset,custom}.<name>` tree and collect the per-expression `{ isBinary, overrideMouth, overrideBlink, overrideLookAt }` fields into a `Vec<Expressio
  - 視点: 正常系 / 空
  - 既存テスト: load_expression_overrides_full_block, load_expression_overrides_missing_fields_default, load_expression_overrides_missing_block_returns_empty, load_expression_overrides_unknown_override_string_falls_back, load_expression_overrides_is_binary_true_parsed
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ExpressionLayer::apply_overrides` (pub, L384)
  - 役割: Apply per-expression override semantics to the current weight map, consuming the definitions parsed from the `VRMC_vrm.expressions` block. Call this every frame **after** writing p
  - 視点: 正常系 / 空
  - 既存テスト: apply_overrides_on_direct_weight_map
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mk` (private, L427)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mk_map` (private, L431)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `def` (private, L435)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: empty_defs_is_no_op, load_expression_overrides_missing_fields_default
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `def_none` (private, L454)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_clamps_above_half_to_one` (private, L459)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_clamps_above_half_to_one
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_clamps_below_half_to_zero` (private, L473)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_clamps_below_half_to_zero
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_false_leaves_weight` (private, L487)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_false_leaves_weight
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `block_zeros_target_when_source_active` (private, L501)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: block_zeros_target_when_source_active
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `block_does_not_affect_target_when_source_inactive` (private, L519)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: block_does_not_affect_target_when_source_inactive
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `blend_attenuates_target_by_source_weight` (private, L536)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: blend_attenuates_target_by_source_weight
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `blend_sums_multiple_sources` (private, L554)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: blend_sums_multiple_sources
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `blend_clamped_to_zero_when_sum_ge_one` (private, L581)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: blend_clamped_to_zero_when_sum_ge_one
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `block_wins_over_blend_when_both_affect_same_target` (private, L606)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: block_wins_over_blend_when_both_affect_same_target
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `block_zeros_all_blink_family_members` (private, L636)
  - 役割: v1-aligned test: an emotion that sets `overrideBlink = block` must zero **every** blink-family expression — the synthetic `blink` and the per-side `blinkLeft` / `blinkRight`. The s
  - 視点: 正常系
  - 既存テスト: block_zeros_all_blink_family_members
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `block_zeros_all_gaze_family_members` (private, L668)
  - 役割: v1-aligned test: a `block` on the look-at family zeros `lookUp` / `lookDown` / `lookLeft` / `lookRight` together. The same-family block covers the whole `GAZE_TARGET_NAMES` set.
  - 視点: 正常系
  - 既存テスト: block_zeros_all_gaze_family_members
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `same_kind_override_is_ignored` (private, L698)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: same_kind_override_is_ignored
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_source_uses_binary_output_for_override` (private, L712)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_source_uses_binary_output_for_override
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_source_below_half_gives_no_override` (private, L731)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_source_below_half_gives_no_override
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_target_suppressed_when_overridden` (private, L749)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_target_suppressed_when_overridden
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_binary_target_suppressed_by_is_binary_source` (private, L773)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_binary_target_suppressed_by_is_binary_source
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_mouth_affects_only_mouth_targets` (private, L798)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: override_mouth_affects_only_mouth_targets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_look_at_affects_only_gaze_targets` (private, L827)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: override_look_at_affects_only_gaze_targets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `blink_override_affects_blink_left_and_right` (private, L856)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: blink_override_affects_blink_left_and_right
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `empty_defs_is_no_op` (private, L882)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: empty_defs_is_no_op
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `empty_weights_is_no_op` (private, L891)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: empty_weights_is_no_op
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `custom_expression_can_override_procedural` (private, L905)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: custom_expression_can_override_procedural
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_type_from_json_str_block` (private, L922)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: override_type_from_json_str_block
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_type_from_json_str_blend` (private, L930)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: override_type_from_json_str_blend
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_type_from_json_str_none` (private, L938)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: override_type_from_json_str_none
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `override_type_from_json_str_unknown_falls_back_to_none` (private, L946)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: override_type_from_json_str_unknown_falls_back_to_none
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_procedural_recognises_all_targets` (private, L954)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: is_procedural_recognises_all_targets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `gltf_from_vrmc` (private, L969)
  - 役割: Wrap a `VRMC_vrm` extension block in a minimal glTF 2.0 document and parse it. Mirrors the pattern in the `look_at.rs` tests.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides_full_block` (private, L980)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: load_expression_overrides_full_block
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides_missing_fields_default` (private, L1036)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: load_expression_overrides_missing_fields_default
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides_missing_block_returns_empty` (private, L1059)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: load_expression_overrides_missing_block_returns_empty
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides_unknown_override_string_falls_back` (private, L1068)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: load_expression_overrides_unknown_override_string_falls_back
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_expression_overrides_is_binary_true_parsed` (private, L1089)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: load_expression_overrides_is_binary_true_parsed
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_overrides_on_direct_weight_map` (private, L1109)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: apply_overrides_on_direct_weight_map
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/humanoid.rs`
- [x] `VrmBone::as_str` (pub, L140)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for VrmBone::fmt` (private, L146)
  - 役割: Display/Debug
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for VrmBone::from` (private, L152)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: registry_built_from_canonicalize_then_insert
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for VrmBone::from` (private, L158)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: registry_built_from_canonicalize_then_insert
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `Trait for BoneRestTransform::default` (private, L184)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: bone_rest_transform_default_is_identity
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `HumanoidBoneRegistry::new` (pub, L222)
  - 役割: Build an empty registry. Most code paths should use [`load_humanoid_bones`] instead.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::insert` (pub, L231)
  - 役割: Insert an entry. Returns `true` if the bone was new, `false` if the same name was already present (the existing entry is left untouched — last-write-wins would silently overwrite a
  - 視点: 正常系 / true/false
  - 既存テスト: registry_built_from_canonicalize_then_insert
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::lookup` (pub, L242)
  - 役割: Look up an entry by its canonical (lower-case) bone name. Use [`Self::by_name`] to accept the spec's mixed-case form too.
  - 視点: 正常系 / None
  - 既存テスト: registry_lookup_by_canonical_name
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::by_name` (pub, L249)
  - 役割: Look up an entry by a possibly-mixed-case / mixed- separator bone name. `canonicalize_bone_name` does the heavy lifting; unknown names return `None`.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::head` (pub, L254)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::hips` (pub, L260)
  - 役割: Convenience accessor for the `hips` bone (the root of the humanoid chain).
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::chest` (pub, L267)
  - 役割: Convenience accessor for the `chest` bone. Used as the body-center fallback when the model has no `head` bone (see `apps/ene-desktop-v2::character::body_center_world`).
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::jaw` (pub, L271)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::left_eye` (pub, L275)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::right_eye` (pub, L279)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::iter` (pub, L284)
  - 役割: Iterate `(bone, entry)` in canonical (sorted) order.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::names` (pub, L289)
  - 役割: Sorted list of registered canonical bone names.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::len` (pub, L293)
  - 役割: trivial getter
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `HumanoidBoneRegistry::is_empty` (pub, L297)
  - 役割: trivial getter
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `canonicalize_bone_name` (pub, L319)
  - 役割: Map a possibly-canonical bone name to the spec's lower-case form. The loader sees a mix of styles in the wild (lower-case per the spec, `PascalCase` from hand-written models, `snak
  - 視点: 正常系 / None / 空
  - 既存テスト: canonicalize_bone_name_handles_known_variants, canonicalize_bone_name_returns_none_for_typo
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_humanoid_bones` (pub, L352)
  - 役割: Parse `Document::extensions()["VRMC_vrm"]["humanoid"]["humanBones"]` and build a [`HumanoidBoneRegistry`]. `skel` is the already-loaded skeleton — the joint-index lookup uses `Skel
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/layer_composer.rs`
- [x] `MotionLayer::as_str` (pub, L50)
  - 役割: Stable display / log label.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::accept_motion` (pub, L102)
  - 役割: `priority` should follow the convention: 4 = llm, 3 = affect, 2 = hysteresis, 1 = fallback.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::cancel_motion` (pub, L128)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::cancel_all_motions` (pub, L136)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::set_expression` (pub(crate), L151)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::remove_expression` (pub(crate), L180)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::clear_expressions` (pub(crate), L188)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::tick` (pub, L195)
  - 役割: Full preempts Upper and Lower — when Full is active, only the Full slot advances and Upper/Lower clocks are paused. Slots are auto-cleared when their playback finishes.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: tick_advances_playing_motions, full_tick_preempts_upper
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::active_motion_names` (pub(crate), L225)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: active_motion_names_full_preempts
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::compose` (pub, L239)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::has_active_motion` (pub(crate), L274)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LayerComposer::place_slot` (private, L278)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/lib.rs`
- [x] `version` (pub, L133)
  - 役割: Returns the crate version. Useful for diagnostics and the `about` panel.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/loader.rs`
- [x] `detect_format_version` (private, L73)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `validate_format` (private, L91)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_vrm` (pub, L139)
  - 役割: Load a `.vrm` file from disk and upload every primitive of every glTF `Mesh` to the GPU. `path` is the on-disk `.vrm` file. Both VRM dialects are accepted: a glTF binary with the r
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_all_meshes` (private, L344)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_primitive_morph_targets` (private, L667)
  - 役割: Read every morph target on a single primitive and return the position displacements in normalized model space. `expected_vertex_count` is the host primitive's vertex count; targets
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_merged_skeleton_and_remaps` (private, L738)
  - 役割: Walk every glTF skin, build a **merged skeleton** whose `joint_to_node` is the union of all unique joint nodes, and pre-compute a per-primitive remap table so each vertex's `JOINTS
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_node_hierarchy` (private, L831)
  - 役割: Walk every glTF `Node` and capture the local rotation / position, the parent index, and the world rest-pose transform. The world transform is computed in a single topologically-ord
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_primitive_base_color_texture` (private, L894)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_image_data` (private, L1015)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_morph_offset` (private, L1079)
  - 役割: Map a raw glTF morph-target POSITION displacement into the vertex buffer's coordinate space. The vertex buffer stays in raw glTF space: the loader never recenters or scales vertice
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_mtoon_gpu_textures` (private, L1086)
  - 役割: Load all `MToon` textures referenced by a material and create the combined GPU bind group. Returns `Ok(None)` if the material has no `MToon` textures at all.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_mtoon_texture_or_dummy` (private, L1367)
  - 役割: Load a single glTF texture by index, or create a 1×1 white dummy if `index` is `None`.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `upload_mtoon_texture` (private, L1393)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/look_at.rs`
- [x] `Trait for LookAtRangeMap::default` (private, L82)
  - 役割: The spec's default range map: `90 → 10` degrees.
  - 視点: 正常系
  - 既存テスト: default_properties_match_vrm_spec, load_look_at_partial_block_uses_spec_defaults
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `LookAtRangeMap::apply` (pub, L104)
  - 役割: Apply the range map to a signed input angle. Input is clamped to `[-input_max_value, +input_max_value]` (absolute value) and the sign is preserved. The spec describes the four maps
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LookAtRangeMapSet::apply_horizontal` (pub, L155)
  - 役割: Apply the appropriate horizontal range map to a signed yaw angle. The caller passes which side wins (the convergent or the divergent eye); we just pick the map.
  - 視点: 正常系 / true/false / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LookAtRangeMapSet::apply_vertical` (pub, L166)
  - 役割: Apply the appropriate vertical range map to a signed pitch angle. The convention matches the spec: `pitch > 0` = looking down, `pitch < 0` = looking up.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for LookAtProperties::default` (private, L210)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: default_properties_match_vrm_spec, load_look_at_partial_block_uses_spec_defaults
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `LookAtEvaluator::new` (pub, L320)
  - 役割: Build an evaluator from a `LookAtProperties`. Returns `None` for an unset `None` so the runtime can chain `model.look_at().map(LookAtEvaluator::new)` without nested `if let`s.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LookAtEvaluator::evaluate` (pub, L348)
  - 役割: Per-frame evaluation. `head_world` is the world-space position of the head bone (the runtime supplies this from the humanoid registry + the current model transform); `target_world`
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `yaw_pitch_to_delta` (private, L483)
  - 役割: Convert a `(yaw, pitch)` pair in degrees to a `Quat` delta, applying the per-axis range maps first. `side` picks `horizontalInner` or `horizontalOuter` for the yaw map; pitch alway
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `calc_yaw_pitch` (pub, L521)
  - 役割: Decompose the world-space gaze direction into a `(yaw, pitch)` pair in degrees. Sign convention: positive yaw = model looks to its own left, positive pitch = model looks down. `hea
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: calc_yaw_pitch_straight_ahead_is_zero, calc_yaw_pitch_target_to_model_left_gives_positive_yaw, calc_yaw_pitch_target_above_gives_negative_pitch
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_look_at` (pub, L584)
  - 役割: Parse the `VRMC_vrm.lookAt` extension object. Returns `None` when the block is absent (e.g. a VRM 0.x model or a stripped-down test fixture) so the runtime can fall back to the spe
  - 視点: 正常系 / None
  - 既存テスト: load_look_at_parses_full_block, load_look_at_returns_none_when_block_missing, load_look_at_partial_block_uses_spec_defaults, load_look_at_malformed_offset_falls_back, load_look_at_unknown_type_falls_back_to_bone
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_range_map` (private, L663)
  - 役割: Parse a single `RangeMap`-shaped `{ inputMaxValue, outputScale }` field. `default` is returned on a missing / malformed entry (and a warning is logged). Used by [`load_look_at`] fo
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/minimal.rs`
- [x] `minimal_vrm_glb_bytes` (pub, L17)
  - 役割: Returns a minimal valid VRM 1.0 GLB (binary glTF with `VRMC_vrm`). The asset contains one quad (two triangles) with `POSITION` / `NORMAL` / `TEXCOORD_0` attributes and a default PB
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `write_glb` (pub, L22)
  - 役割: Write [`minimal_vrm_glb_bytes`] to `path`.
  - 視点: 正常系 / エラー
  - 既存テスト: write_glb_round_trip
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `mesh_bin_chunk` (private, L32)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pack_glb` (private, L73)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/model.rs`
- [x] `VrmFormatVersion::as_label` (pub, L44)
  - 役割: Human-readable label used by GUI surfaces that show which dialect a loaded avatar uses.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AlphaMode::render_phase` (pub, L116)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Skeleton::joint_count` (pub, L179)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeHierarchy::len` (pub, L259)
  - 役割: trivial getter
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeHierarchy::is_empty` (pub, L265)
  - 役割: `true` when no nodes were captured (the model file had zero glTF `Node` objects — malformed).
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeHierarchy::compute_world_transforms` (pub, L274)
  - 役割: Build the world transforms from `local_rotations` / `local_positions` / `parents`. The walk assumes glTF nodes are topologically ordered (parents before children) — which the loade
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmModel::aabb` (pub, L380)
  - 役割: Raw glTF AABB `(min, max)` of every vertex, in model-local space. The runtime's auto-fit scale and the `model.model` matrix both consume this.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `center` (pub, L387)
  - 役割: AABB center in raw glTF space. The runtime subtracts this from every vertex (via the model matrix) so the character pivots around its own midpoint.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalize_scale` (pub, L394)
  - 役割: `1.5 / max_extent` — the uniform scale the runtime applies to map the longest AABB axis to the canonical 1.5 m model size.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `normalized_aabb` (pub, L404)
  - 役割: AABB expressed in **normalized** space (i.e. after applying `T(-center) * S(normalize_scale)` to the raw AABB). The result is centred on the origin with the longest extent equal to
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `joint_count` (pub, L412)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `expressions` (pub, L418)
  - 役割: The runtime writes into `expressions.weights` every frame; the renderer reads it.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `expressions_mut` (pub, L426)
  - 役割: Used by `CharacterRenderer::apply_emotions` in `apps/ene-desktop-v2` to push the latest emotion weights into the model.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `look_at` (pub, L435)
  - 役割: `None` for models without the `VRMC_vrm.lookAt` block (e.g. legacy VRM 0.x). The desktop runtime supplies the spec default in that case via [`LookAtProperties::default`].
  - 視点: 正常系 / None
  - 既存テスト: update_skin_palette_applies_look_at_bone_delta_to_head, update_skin_palette_look_at_composes_onto_vrma_for_head, update_skin_palette_look_at_idempotent_for_zero_delta_or_missing_bones, update_skin_palette_look_at_ignores_missing_humanoid_bones
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format_version` (pub, L441)
  - 役割: The VRM dialect this model was parsed from.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `format_version_label` (pub, L447)
  - 役割: Human-readable dialect label ([`VrmFormatVersion::as_label`]).
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `with_format_version` (pub, L455)
  - 役割: Override the detected dialect. Test fixtures that build a [`VrmModel`] directly keep the default; the loader chains this after [`Self::new`] to record what it found.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `new` (pub, L468)
  - 役割: Construct a `VrmModel` from its already-built pieces plus the raw AABB and the centre/scale the runtime needs to build the model matrix. Used by the loader and by downstream test f
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `update_skin_palette` (pub, L569)
  - 役割: Apply a [`VrmaFrame`] to the node hierarchy and return the per-skin-joint palette `mat4x4<f32>[]` ready for `queue.write_buffer`. The algorithm is: 1. **Reset to rest**: copy `node
  - 視点: 正常系 / None
  - 既存テスト: update_skin_palette_empty_joints_returns_empty, update_skin_palette_empty_nodes_returns_empty, update_skin_palette_no_frame_changes_returns_rest_palette, update_skin_palette_applies_bone_rotation, update_skin_palette_hips_local_does_not_double_rest_height, update_skin_palette_hips_translation_cascades_to_descendants, update_skin_palette_ignores_unknown_bone_names, update_skin_palette_uses_identity_for_out_of_range_joint_node
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `rebuild_skin_palette` (pub, L647)
  - 役割: Recompute world transforms from the current `local_*` buffers and rebuild the skin palette. Call after mutating [`NodeHierarchy::local_rotations`] post-pose (e.g. spring bones) so
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `evaluate_vrma` (pub, L697)
  - 役割: Sample a VRMA clip with NLR retargeting onto this model's rest pose.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `MeshVertex::as_bytes` (pub, L771)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for SkinMatrix::from` (private, L1553)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)







## `ene-vrm` — `crates/ene-vrm/src/mtoon.rs`
- [x] `OutlineWidthMode::from_json_str` (private, L49)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for MToonMaterial::default` (private, L137)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: defaults_when_extension_missing, defaults_when_extension_present_but_empty
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `load_mtoon_materials` (pub, L182)
  - 役割: Parse `VRMC_materials_mtoon` from every glTF material in the document. Returns a `Vec<Option<MToonMaterial>>` indexed by material index. Materials without the extension get `None`.
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_vec3` (private, L333)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_texture_ref` (private, L343)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_texture_ref_from_obj` (private, L348)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for MToonGpuTextures::fmt` (private, L384)
  - 役割: Display/Debug
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `texture_flags` (pub, L391)
  - 役割: Used by the renderer to fill the per-material uniform.
  - 視点: 正常系 / true/false
  - 既存テスト: texture_flags_bitmask
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for MToonUniform::default` (private, L448)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: defaults_when_extension_missing, defaults_when_extension_present_but_empty
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `MToonUniform::from_material` (pub, L466)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: uniform_from_material_roundtrip
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/node_constraint.rs`
- [x] `RollAxis::as_vec3` (private, L44)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: roll_axis_as_vec3, aim_axis_as_vec3
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `RollAxis::from_json_str` (private, L52)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AimAxis::as_vec3` (private, L75)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: roll_axis_as_vec3, aim_axis_as_vec3
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `AimAxis::from_json_str` (private, L86)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraint::source_node` (pub, L133)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraint::weight` (pub, L141)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: parse_default_weight, eval_rotation_full_weight_copies_delta, eval_rotation_zero_weight_keeps_rest, constraint_source_and_weight_accessors
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::len` (pub, L170)
  - 役割: trivial getter
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::is_empty` (pub, L174)
  - 役割: trivial getter
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::evaluate` (pub, L202)
  - 役割: Evaluate all constraints and return the updated local rotations for constrained destination nodes. `node_local_rotations` maps glTF node index to the current local rotation. The ev
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::eval_rotation` (private, L265)
  - 役割: Rotation constraint: copy the source's rotation delta onto the destination. `new_rot = slerp(dst_rest, dst_rest * (src_rest^-1 * src_local), weight)`
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: eval_rotation_identity_when_source_at_rest, eval_rotation_full_weight_copies_delta, eval_rotation_zero_weight_keeps_rest
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::eval_roll` (private, L274)
  - 役割: Roll constraint: extract the rotation component around `roll_axis` from the source and apply it to the destination.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `NodeConstraintRegistry::eval_aim` (private, L293)
  - 役割: Aim constraint: rotate the destination so `aim_axis` points at the source's world position.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: eval_aim_source_at_destination_returns_rest
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_node_constraints` (pub, L324)
  - 役割: Parse `VRMC_node_constraint` from every glTF node and return the registry. Walks `gltf.document.nodes()`, checks each node's `extensions` for `VRMC_node_constraint`, and parses the
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_constraint` (private, L362)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/post_process.rs`
- [x] `PostProcessor::new` (pub, L73)
  - 役割: Build a new post-processor. The intermediate texture matches the swapchain format so the depth-and-color interaction stays identical to the no-FXAA path.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PostProcessor::resize` (pub, L245)
  - 役割: Re-create the post-processor at a new size. Cheap path when the size is unchanged (no-op return). The runtime calls this on `WindowEvent::Resized` (in lock-step with the depth-text
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PostProcessor::render` (pub, L266)
  - 役割: Run the FXAA pass. `src` is the texture the model rendered into (must match the post-processor's intermediate view). `dst` is the swapchain view.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PostProcessor::intermediate_view` (pub, L310)
  - 役割: The view the model must be rendered into. Returned by value; the runtime passes it to the model's `renderer.render` call as the colour attachment.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PostProcessor::size` (pub, L314)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `inv_size` (pub, L323)
  - 役割: Helper: 1/Vec2 inverse for the `inv_size` uniform. The shader reads `vec2<f32> inv_size`; a typed accessor keeps the post-process `init` and `resize` paths symmetric.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/renderer.rs`
- [x] `VrmRenderer::new` (pub, L219)
  - 役割: Build a renderer for the given model. The model's base-color texture (if any) is bound at group `(2)`. Morph-target data is bound at group `(3)` on a per-primitive basis.
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::render` (pub, L900)
  - 役割: Render `model` into `view` with the given camera + model transform. `depth_view` is the depth attachment (must match the pipeline's `Depth32Float` format). `queue` is used to uploa
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::render_mask` (pub, L1021)
  - 役割: Renders the mask into the provided `target_view` using the internal `pipeline_mask`. If the renderer was built without `mask_format`, this is a no-op.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::draw_primitive` (private, L1087)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::upload_morph_meta` (private, L1141)
  - 役割: Build the per-frame [`PrimitiveMorphMeta`] uniform for `morph` from the model's global weight map and upload it. The slot index used to look up weights is the **per-primitive local
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::update_skin_palette` (pub, L1190)
  - 役割: Overwrite the skin-palette storage buffer with the joint world transforms returned by [`VrmModel::update_skin_palette`]. `palette` is a `Vec<Mat4>` of length `VrmModel::joint_count
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VrmRenderer::skin_joint_count` (pub, L1205)
  - 役割: The joint count of the renderer's skin palette. Zero for models built with the identity one-element palette (no skin).
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_morph_gpu` (private, L1220)
  - 役割: Build a one-shot morph bind group for a single primitive that has at least one morph target. The storage buffer is uploaded once with the loader's displacement data; the meta unifo
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `base_color_bind_group_layout` (private, L1276)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_dummy_base_color_gpu` (private, L1300)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_dummy_morph_gpu` (private, L1370)
  - 役割: Shared dummy morph bind group for primitives without morph targets. The bound storage buffer is a single `vec4` of zeros; the bound meta has `target_count = 0` so the shader never
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_skin_gpu` (private, L1420)
  - 役割: Build the per-model skin-matrix palette. For models with a populated `Skeleton` the palette is the pre-baked `bind_matrices` (i.e. `inverse_bind[i].inverse()`). For models with no
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/spring_bone.rs`
- [x] `load_spring_bones` (pub, L143)
  - 役割: Parse `VRMC_springBone` from the glTF root extensions. Returns `None` if the extension is not present. Logs warnings for malformed entries and falls back to defaults. Called intern
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_colliders` (private, L165)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_collider_groups` (private, L204)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_springs` (private, L229)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_vec3` (private, L303)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SpringBoneSimulator::new` (pub, L361)
  - 役割: `node_world_positions` maps glTF node index to world position. `node_world_rotations` maps glTF node index to world rotation. `node_parent_world_rotations` maps glTF node index to
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `SpringBoneSimulator::step` (pub, L483)
  - 役割: `node_world_positions` and `node_world_rotations` are the current per-node world transforms. `node_parent_world_rotations` are the parent's world rotations (identity for roots). `c
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: simulator_step_no_forces_stays_at_rest, simulator_step_updates_state
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/thumbnail.rs`
- [x] `load_vrm_thumbnail` (pub, L12)
  - 役割: Load the encoded thumbnail referenced by a VRM 1.0 model, if it has one. VRM stores the thumbnail as a glTF image index in `VRMC_vrm.meta.thumbnailImage`. The returned bytes are st
  - 視点: 正常系 / None / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `thumbnail_image_index` (private, L47)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/viseme.rs`
- [x] `VisemeAnalyzer::new` (pub, L107)
  - 役割: Build an analyzer for the given `sample_rate` (Hz). The analysis window is sized to roughly 20 ms of audio and rounded up to the next power of two so the FFT stays cheap.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisemeAnalyzer::window_size` (pub, L133)
  - 役割: Number of PCM samples held in one analysis window. Feeding at least this many samples guarantees the analyzer sees a full frame; feeding more simply discards the oldest samples.
  - 視点: 正常系
  - 既存テスト: window_size_is_power_of_two_and_at_least_minimum
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisemeAnalyzer::push_pcm` (pub, L143)
  - 役割: Append PCM samples (mono, `[-1, 1]`) to the internal ring buffer. Samples beyond the analysis window are dropped oldest-first, so the buffer always holds the most recent [`window_s
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisemeAnalyzer::analyze` (pub, L156)
  - 役割: Analyze the buffered audio and return the smoothed mouth-shape weights. Intended to be called once per render frame. With too few buffered samples the weights simply decay toward z
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `VisemeAnalyzer::reset` (pub, L192)
  - 役割: Clear the ring buffer and reset the smoothed weights to zero.
  - 視点: 正常系
  - 既存テスト: reset_clears_buffer_and_weights
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `hann` (private, L199)
  - 役割: Hann window coefficient for sample `idx` of a `len`-sample frame.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `band_fractions` (private, L210)
  - 役割: Split the FFT magnitude spectrum into five normalized frequency bands: `[low, low-mid, mid, mid-high, high]`. The returned fractions sum to `1` (or are all zero for a silent frame)
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `target_weights` (private, L241)
  - 役割: Map the per-frame DSP features (RMS, zero-crossing rate, band fractions) onto raw, un-smoothed viseme weights.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `distribute` (private, L282)
  - 役割: Normalize raw per-vowel scores so they sum to one. When there is no spectral evidence the scores are spread evenly, keeping the mouth neutral rather than lopsided.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `smoothstep` (private, L293)
  - 役割: Hermite smoothstep between `edge0` and `edge1`, clamped to `[0, 1]`.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `smooth_weights` (private, L302)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `smooth_field` (private, L314)
  - 役割: Move one weight field toward its target, attacking faster than it releases to keep the lips responsive yet jitter-free.
  - 視点: 正常系 / 0 / 端 / HiDPI
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-vrm` — `crates/ene-vrm/src/vrm0.rs`
- [x] `AllowedUserName::parse` (private, L71)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `UsagePermission::parse` (private, L95)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `root_vrm` (private, L103)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `as_object` (private, L107)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_vec3` (private, L111)
  - 役割: read body; see 結果
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `vec3_field` (private, L119)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_meta` (pub(crate), L126)
  - 役割: Parse the legacy `meta` block. Called only on detected VRM 0.x documents; missing or malformed fields degrade to their defaults rather than failing the whole load, mirroring how th
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `convert_humanoid` (pub(crate), L186)
  - 役割: Convert the legacy `humanoid.humanBones` array (1.0 uses an object keyed by bone name) into the shared registry. Joint lookup and rest transforms come from the merged skeleton buil
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `convert_blendshapes` (pub(crate), L248)
  - 役割: Convert legacy `blendShapeMaster.blendShapeGroups` into the 1.0 `ExpressionDefinition` list. Each group becomes one named expression; binds keep the shared `(mesh, index, weight)`
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `convert_look_at` (pub(crate), L322)
  - 役割: Convert the legacy `firstPerson` look-at fields. The 1.0 block uses nested `rangeMap*` objects; 0.x stores four flat curve objects with an `xRange`/`yRange` degree pair and targets
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_legacy_range` (private, L340)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `convert_spring_bones` (pub(crate), L356)
  - 役割: Convert legacy `secondaryAnimation` into the 1.0 spring-bone properties. 0.x has no standalone collider list - colliders live inside `boneGroups.colliderGroups` with node, offset a
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_mtoon_materials_vrm0` (pub(crate), L482)
  - 役割: Build MToon materials from the legacy per-material `VRM` extension. 1.0 carries its MToon parameters in `VRMC_materials_mtoon`; a legacy file stores shader identity in `_shader` pl
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/client.rs`
- [x] `ApiClient::new` (pub, L40)
  - 役割: コンストラクタ
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::base` (pub, L54)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::client_id` (pub, L59)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::token` (pub, L64)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::request` (private, L68)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::send_json` (private, L75)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::send_empty` (private, L99)
  - 役割: HTTP GET `/api/v1/souls/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::health` (pub, L104)
  - 役割: HTTP GET `/api/v1/souls/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_souls` (pub, L109)
  - 役割: HTTP GET `/api/v1/souls/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::get_soul` (pub, L114)
  - 役割: HTTP GET `/api/v1/souls/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_greetings` (pub, L119)
  - 役割: HTTP GET `/api/v1/souls/{soul_id}/greetings` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_soul_body` (pub, L124)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_soul_skills` (pub, L132)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_sessions` (pub, L144)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::search_sessions` (pub, L151)
  - 役割: HTTP GET `/api/v1/sessions/{id}` via send_json/send_empty
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::create_session` (pub, L165)
  - 役割: HTTP GET `/api/v1/sessions/{id}` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::get_session` (pub, L173)
  - 役割: HTTP GET `/api/v1/sessions/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_session` (pub, L178)
  - 役割: HTTP PATCH `/api/v1/sessions/{id}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::fork_session` (pub, L190)
  - 役割: HTTP POST `/api/v1/sessions/{id}/fork` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::split_session` (pub, L195)
  - 役割: HTTP POST `/api/v1/sessions/{id}/split` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::end_session` (pub, L200)
  - 役割: HTTP POST `/api/v1/sessions/{id}/end` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::barge_in` (pub, L212)
  - 役割: HTTP POST `/api/v1/sessions/{id}/barge-in` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::listen` (pub, L217)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::listen_stream` (pub, L230)
  - 役割: Open a bulk mic PCM socket. Binary frames are [`PCM_S16LE`] at `sample_rate`.
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::stage` (pub, L249)
  - 役割: HTTP GET `/api/v1/souls/{id}/affect` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::soul_affect` (pub, L254)
  - 役割: HTTP GET `/api/v1/souls/{id}/affect` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::export_session` (pub, L259)
  - 役割: HTTP POST `/api/v1/sessions/{id}/export` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::send_message` (pub, L264)
  - 役割: HTTP POST `/api/v1/sessions/{session_id}/messages` via send_json/send_empty
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::history` (pub, L282)
  - 役割: HTTP GET `/api/v1/sessions/{session_id}/history?depth={depth}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::select_greeting` (pub, L294)
  - 役割: HTTP POST `/api/v1/sessions/{session_id}/greeting` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::cancel_queued` (pub, L309)
  - 役割: HTTP DELETE `/api/v1/sessions/{session_id}/queued/{entry_id}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::compact` (pub, L321)
  - 役割: HTTP POST `/api/v1/sessions/{session_id}/compact` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::cancel_turn` (pub, L329)
  - 役割: HTTP POST `/api/v1/turns/{turn_id}/cancel` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_jobs` (pub, L334)
  - 役割: HTTP GET `/api/v1/jobs/{id}` via send_json/send_empty
  - 視点: 正常系 / None / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::create_job` (pub, L342)
  - 役割: HTTP GET `/api/v1/jobs/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::get_job` (pub, L347)
  - 役割: HTTP GET `/api/v1/jobs/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::cancel_job` (pub, L352)
  - 役割: HTTP POST `/api/v1/jobs/{id}/cancel` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::answer_job` (pub, L357)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::answer_question` (pub, L365)
  - 役割: HTTP POST `/api/v1/jobs/{job_id}/questions/{question_id}/answer` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_tasks` (pub, L381)
  - 役割: HTTP GET `/api/v1/tasks/{id}` via send_json/send_empty
  - 視点: 正常系 / None / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::create_task` (pub, L389)
  - 役割: HTTP GET `/api/v1/tasks/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::get_task` (pub, L394)
  - 役割: HTTP GET `/api/v1/tasks/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::cancel_task` (pub, L399)
  - 役割: HTTP POST `/api/v1/tasks/{id}/cancel` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::verify_task` (pub, L404)
  - 役割: HTTP POST `/api/v1/tasks/{id}/verify` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::approve_task_scope` (pub, L409)
  - 役割: HTTP POST `/api/v1/tasks/{id}/scope-approval` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_schedules` (pub, L414)
  - 役割: HTTP PATCH `/api/v1/schedules/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::create_schedule` (pub, L419)
  - 役割: HTTP PATCH `/api/v1/schedules/{id}` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_schedule` (pub, L427)
  - 役割: HTTP PATCH `/api/v1/schedules/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / true/false / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::delete_schedule` (pub, L435)
  - 役割: HTTP DELETE `/api/v1/schedules/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_artifacts` (pub, L440)
  - 役割: HTTP GET `/api/v1/artifacts/{id}/content` via send_json/send_empty
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::artifact_content` (pub, L451)
  - 役割: HTTP GET `/api/v1/artifacts/{id}/content` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_memories` (pub, L456)
  - 役割: HTTP PATCH `/api/v1/memories/{id}` via send_json/send_empty
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_memory` (pub, L468)
  - 役割: HTTP PATCH `/api/v1/memories/{id}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::delete_memory` (pub, L480)
  - 役割: HTTP DELETE `/api/v1/memories/{id}` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_pending_memories` (pub, L485)
  - 役割: HTTP GET `/api/v1/memories/pending?soul_id={soul_id}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::resolve_memory_candidate` (pub, L496)
  - 役割: HTTP POST `/api/v1/memories/candidates/{id}/resolve` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_memory_journal` (pub, L511)
  - 役割: HTTP GET `/api/v1/memories/journal?soul_id={soul_id}` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_tools` (pub, L522)
  - 役割: HTTP POST `/api/v1/tools/{name}/test` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::test_tool` (pub, L527)
  - 役割: HTTP POST `/api/v1/tools/{name}/test` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_plugins` (pub, L535)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_provider_models` (pub, L540)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_provider_assets` (pub, L551)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::install_provider_asset` (pub, L562)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::provider_asset_install_status` (pub, L573)
  - 役割: read body; see 結果
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::refresh_provider_assets_catalog` (pub, L584)
  - 役割: HTTP POST `/api/v1/plugins/{id}/restart` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::set_active_provider_asset` (pub, L595)
  - 役割: HTTP POST `/api/v1/plugins/{id}/restart` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::restart_plugin` (pub, L606)
  - 役割: HTTP POST `/api/v1/plugins/{id}/restart` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::plugin_config` (pub, L611)
  - 役割: HTTP GET `/api/v1/plugins/{id}/config` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::validate_plugin_config` (pub, L616)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::plugin_config_options` (pub, L631)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::apply_plugin_config` (pub, L646)
  - 役割: HTTP PUT `/api/v1/plugins/{id}/config` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::mcp` (pub, L658)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::put_mcp` (pub, L663)
  - 役割: HTTP POST `/api/v1/approvals/{id}/respond` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::mcp_catalog` (pub, L668)
  - 役割: HTTP POST `/api/v1/approvals/{id}/respond` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::probe_mcp` (pub, L673)
  - 役割: HTTP POST `/api/v1/approvals/{id}/respond` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_approvals` (pub, L678)
  - 役割: HTTP POST `/api/v1/approvals/{id}/respond` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::respond_approval` (pub, L683)
  - 役割: HTTP POST `/api/v1/approvals/{id}/respond` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::list_characters` (pub, L691)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::import_character` (pub, L696)
  - 役割: HTTP GET `/api/v1/characters/{id}/export` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::import_character_archive_b64` (pub, L704)
  - 役割: HTTP GET `/api/v1/characters/{id}/export` via send_json/send_empty
  - 視点: 正常系 / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::export_character` (pub, L715)
  - 役割: HTTP GET `/api/v1/characters/{id}/export` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::activate_character` (pub, L720)
  - 役割: HTTP POST `/api/v1/characters/{id}/activate` via send_json/send_empty
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::settings` (pub, L725)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::patch_settings` (pub, L730)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::settings_schema` (pub, L735)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::audit` (pub, L740)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::usage` (pub, L745)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / None / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::diag_spans` (pub, L753)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::backup` (pub, L758)
  - 役割: HTTP POST `/api/v1/exclusive/{}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::restore` (pub, L763)
  - 役割: HTTP POST `/api/v1/exclusive/{}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::exclusive` (pub, L768)
  - 役割: HTTP POST `/api/v1/exclusive/{}` via send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::claim_resource` (pub, L773)
  - 役割: HTTP POST `/api/v1/exclusive/{}` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::release_resource` (pub, L788)
  - 役割: HTTP DELETE ` ` via send_json/send_empty
  - 視点: 正常系 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::openapi` (pub, L803)
  - 役割: HTTP wrapper over request() + send_json/send_empty
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::events` (pub, L809)
  - 役割: Open a depth-filtered event socket. `depth` is `surface` or `detail`.
  - 視点: 正常系 / None / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiClient::connect_ws` (private, L825)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EventSocket::recv_json` (pub, L877)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ListenStream::send_pcm` (pub, L908)
  - 役割: Send one mono `f32` frame packed as [`PCM_S16LE`].
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空 / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ListenStream::recv` (pub, L916)
  - 役割: Drive ping/pong and observe a server close. `None` means the socket ended.
  - 視点: 正常系 / None / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `session_search_query` (private, L933)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/error.rs`
- [x] `ApiError::error_class` (pub, L25)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ApiError::from_problem` (pub, L34)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/lib.rs`
- [x] `openapi_json` (pub, L43)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/pcm.rs`
- [x] `encode_pcm_s16le` (pub, L13)
  - 役割: Pack mono `f32` (`-1.0..=1.0`) as little-endian `i16`.
  - 視点: 正常系 / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `decode_pcm_s16le` (pub, L41)
  - 役割: Unpack little-endian `i16` mono into `f32` (`-1.0..=1.0`). # Errors Returns [`ApiError::Codec`] when `bytes` is not an even length.
  - 視点: 正常系 / エラー / 0 / 端 / HiDPI / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/question_event.rs`
- [x] `QuestionEventKind::as_str` (pub, L21)
  - 役割: Wire name of the live-bus event.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `QuestionEventKind::parse` (private, L28)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: parses_resolved_event_without_optional_fields
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `QuestionEvent::parse` (pub, L61)
  - 役割: Parse a raw live-bus JSON object into a typed question event.
  - 視点: 正常系 / None
  - 既存テスト: parses_resolved_event_without_optional_fields
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `QuestionEvent::to_value` (pub, L89)
  - 役割: Serialize into the canonical live-bus JSON shape.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `string_array` (private, L122)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-api` — `crates/ene-api/src/types.rs`
- [x] `Problem::new` (pub, L19)
  - 役割: コンストラクタ
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `T::of` (pub, L41)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `default_tz` (private, L303)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `default_action` (private, L307)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `default_true` (private, L493)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ResourceKind::as_str` (pub, L637)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ResourceKind::parse` (pub, L645)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/config.rs`
- [x] `update_global_config` (pub, L23)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `get_global_config` (pub, L33)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `path` (private, L48)
  - 役割: The full path from the root.
  - 視点: 正常系 / 空
  - 既存テスト: env_uppercase_folds_to_lowercase_path, set_path_writes_dotted_json_value, set_schema_via_set_path_round_trips
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `get_global_section` (pub, L51)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `registered_settings_section_keys` (pub, L84)
  - 役割: Top-level `settings.json` section keys registered via `define_config!`. Nested registrations (`parent_key` is `Some`) are omitted so PATCH/GET allowlists stay aligned with the sche
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `registered_schemas_for` (pub, L97)
  - 役割: Exposed so `ene-card`'s character-schema generator can merge `ConfigTarget::Character` registrations without sharing the registry itself.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register_config_schema` (pub, L118)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register_runtime_schema` (pub, L136)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `runtime_rules_is_default` (private, L158)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `Trait for EneConfig::default` (private, L199)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: malformed_settings_json_returns_error_not_default, empty_settings_json_extracts_defaults, defaults_not_forced_to_disk_on_save
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `EneConfig::get_section` (pub, L220)
  - 役割: Returns `Ok(T::default())` when the key/path is absent. Refuses types whose `TARGET` is `Character`; those sections live in `CharacterConfig::extra` and must go through `CharacterC
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::set_section` (pub, L276)
  - 役割: Only the section's *declared* fields are written; unknown *immediate child* keys already present at the section path are preserved. The merge is one level deep: declared fields tha
  - 視点: 正常系 / エラー
  - 既存テスト: set_section_preserves_unknown_subkeys, set_section_identical_write_is_noop, set_section_writes_clean_floats_into_extra
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::set_path` (pub, L308)
  - 役割: `value` is parsed as JSON when possible; otherwise treated as a string. Used by CLI `/config set`. `$schema` is routed to the declared [`schema`](Self::schema) field rather than `e
  - 視点: 正常系 / エラー / 空
  - 既存テスト: set_path_writes_dotted_json_value, set_schema_via_set_path_round_trips
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::set_section_value` (pub, L340)
  - 役割: This is the generic counterpart of [`set_section`](Self::set_section) for callers that hold a section as an opaque JSON value (the runtime's unified settings-apply path diffs secti
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::section_value` (pub, L360)
  - 役割: `None` when the section is absent (i.e. all defaults). Top-level declared fields (`character`, `user_name`, …) are *not* returned; use [`get_path`](Self::get_path) with the exact k
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::remove_section` (pub, L368)
  - 役割: Returns whether the key was present. Used by the unified settings-apply path when a section disappears from the proposed config (a deleted plugin entry or a cleared section), where
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `EneConfig::get_path` (pub, L375)
  - 役割: Walks the map directly instead of serialising the entire `extra` map into a JSON `Value` tree. `$schema` reads from the declared [`schema`](Self::schema) field, mirroring [`set_pat
  - 視点: 正常系 / None / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `section_to_value` (pub, L410)
  - 役割: Serialises a typed config section into a [`serde_json::Value`] without the f32→f64 widening artefact that `serde_json::to_value` introduces. # Why not `to_value` directly? `serde_j
  - 視点: 正常系
  - 既存テスト: section_to_value_f32_shortest_representation, section_to_value_f32_round_trips_exactly
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_at_path` (pub, L423)
  - 役割: Returns `None` when any key is absent or a non-object is encountered before the final key.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `merge_section` (pub, L455)
  - 役割: When both sides are JSON objects, the section's declared fields are layered on top of the existing object so unknown sibling sub-keys survive; the section struct only ever serialis
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_nested` (pub, L471)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: set_nested_through_non_object_leaf_errors
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `three_way_merge` (private, L578)
  - 役割: Applies the user's in-session edits onto the raw on-disk JSON layer. This is a three-way merge keyed on `base` — the layered config (defaults → JSON → env) the loader produced — wh
  - 視点: 正常系
  - 既存テスト: three_way_merge_keeps_raw_for_unchanged_and_drops_cleared, three_way_merge_recurses_into_nested_objects
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `serialize_json_layer` (private, L644)
  - 役割: Serialises only the JSON layer of `config` for persistence. The in-memory [`EneConfig`] is the result of layering defaults → JSON file → `ENE_` env vars, so serialising it directly
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `serialize_json_layer_with_env` (private, L650)
  - 役割: Variant of [`serialize_json_layer`] whose layered baseline uses an injected env layer instead of the process environment.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `snapshot_schema_registry` (private, L705)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `generate_schema_json` (pub, L716)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_config` (pub, L778)
  - 役割: Returns [`EneConfigError`] if the on-disk `settings.json` is malformed, env-var parsing fails, or required fields cannot be deserialised.
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_config_from` (pub, L783)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_full_config` (pub, L787)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `migrate_settings_file` (private, L804)
  - 役割: Migration happens on the *raw JSON* — before deserialisation into [`EneConfig`] — because a schema change may alter a field's type and make the old file undecodable by the current
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `load_full_config_from` (pub, L895)
  - 役割: # Config-version migration Before the figment pipeline runs, `migrate_settings_file` reads the raw file and applies any registered [config-version migrations](crate::migration), pe
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `extract_layered_config_with_env` (private, L905)
  - 役割: Shared by `load_full_config_from` (the load path) and `serialize_json_layer` (the save path). The save path needs the exact same baseline the loader produced so it can isolate the
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `real_env_layer` (private, L937)
  - 役割: The `.map(...)` makes env vars case-insensitive against the lowercase config keys, matching the documented `ENE_AI__TASKS__CHAT__MODEL` examples.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `extract_layered_config` (private, L945)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_top_level_order` (private, L953)
  - 役割: Returns an empty `Vec` when the file is missing, unreadable, not valid JSON, or not an object — ordering restoration then simply leaves `extra` in the order figment produced, which
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `write_schemas` (pub, L972)
  - 役割: Character schemas are written by the `ene-card` crate's `write_character_schemas`. Guarded by a process-wide [`std::sync::Once`] so the (idempotent but wasteful) schema regeneratio
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `write_schemas_inner` (private, L977)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `atomic_write` (pub, L1012)
  - 役割: Atomically writes `contents` to `path` by first writing to a temporary file in the same directory, then renaming over the target. The rename is atomic on POSIX when source and dest
  - 視点: 正常系 / エラー / 空
  - 既存テスト: atomic_write_produces_final_contents_and_no_tmp_leftover, atomic_write_overwrites_existing_file, atomic_write_preserves_existing_permissions
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `tmp_counter` (private, L1057)
  - 役割: Monotonic counter so successive temp files in the same process get distinct names even when written within the same millisecond.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `preserve_permissions` (private, L1069)
  - 役割: Copies the permission bits from an existing `src` file onto `dst`. A missing source (first write) or any metadata error is ignored: the temp file simply keeps the default mode. Thi
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `preserve_permissions` (private, L1088)
  - 役割: Non-Unix platforms: nothing to preserve beyond the default mode.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `fsync_dir` (private, L1095)
  - 役割: Best-effort `fsync` of a directory so the preceding `rename` is durable. Not all platforms/filesystems support syncing a directory; any error is logged at debug level and ignored.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `restore_top_level_order` (private, L1123)
  - 役割: Keys listed in `order` come first (in that order); any key absent from `order` — a section added after load — keeps its existing relative position but sorts after the recorded ones
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `save_full_config` (pub, L1152)
  - 役割: Only the JSON layer is persisted: `ENE_` env-var overrides and defaults are excluded so a transient env override never becomes permanent. See `serialize_json_layer` for the layer-r
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `update_section` (pub, L1163)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/lib.rs`
- [x] `name::default` (private, L106)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::path` (private, L118)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register` (private, L129)
  - 役割: # Safety Called by `ctor` before `main`. Only safe registration code is executed; no I/O, TLS, or cross-ctor ordering assumed.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::default` (private, L158)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::path` (private, L170)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register` (private, L181)
  - 役割: # Safety Called by `ctor` before `main`. Only safe registration code is executed; no I/O, TLS, or cross-ctor ordering assumed.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::default` (private, L210)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::path` (private, L222)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register` (private, L239)
  - 役割: # Safety Called by `ctor` before `main`. Only safe registration code is executed; no I/O, TLS, or cross-ctor ordering assumed.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::label` (pub, L276)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `name::label` (pub, L315)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/migration.rs`
- [x] `registry` (private, L93)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `current_version` (private, L102)
  - 役割: Returns the effective current schema version. In production this is always [`CURRENT_CONFIG_VERSION`]. Under `cfg(test)` an override may be installed via `TEST_VERSION_OVERRIDE` to
  - 視点: 正常系
  - 既存テスト: current_version_is_untouched
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register_migration` (pub, L131)
  - 役割: Registers a migration step that rewrites a version-`from` document into version `from + 1`. Registering a second step for the same `from` version replaces the previous one, so star
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_version` (private, L155)
  - 役割: Reads the `version` field out of a raw settings document. A missing `version` is treated as `1`: the earliest shipped schema was version 1, and very early hand-written files predat
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `document_version` (pub(crate), L179)
  - 役割: The schema version a raw settings document declares, for callers that need it before [`apply_migrations`] consumes the document. A document whose `version` is missing or malformed
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `set_version` (private, L183)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `apply_migrations` (pub, L214)
  - 役割: Migrates a raw settings document forward to the current schema version. Returns the (possibly rewritten) document with its `version` field stamped to the current version. If the do
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `migrate_drop_legacy_plugin_list` (pub(crate), L263)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `migrate_clear_echo_bindings` (pub(crate), L291)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `register_config_migrations` (private, L326)
  - 役割: # Safety Called by `ctor` before `main`. Only safe registration code is executed; no I/O, TLS, or cross-ctor ordering assumed.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/paths.rs`
- [x] `app_data_dir` (pub, L17)
  - 役割: OS-standard user data directory (`~/.local/share` on Linux, `%APPDATA%` on Windows). Release fallback for [`assets_dir`] only. Runtime code should call [`data_dir`] so debug builds
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `data_dir` (pub, L30)
  - 役割: Runtime root for `settings.json`, databases, vault, and workspace. `ENE_DATA_DIR` overrides when set. Otherwise this is [`assets_dir`]: debug builds use source-tree `assets/`; rele
  - 視点: 正常系
  - 既存テスト: config_file_path_is_under_data_dir, user_plugin_and_tool_dirs_are_under_data_dir, resolve_data_dir_prefers_override_then_assets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_data_dir` (private, L34)
  - 役割: read body; see 結果
  - 視点: 正常系 / None / 空
  - 既存テスト: resolve_data_dir_prefers_override_then_assets
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_assets_dir_impl` (private, L41)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `assets_dir` (pub, L64)
  - 役割: Debug: source-tree `assets/`. Release: [`app_data_dir`] (never the repository `assets/` folder). Returns a `&'static Path` to avoid cloning the cached `PathBuf` on every call.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `models_dir` (pub, L69)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `config_file_path` (pub, L73)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: config_file_path_is_under_data_dir
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pattern_pack_path_in` (pub(crate), L82)
  - 役割: Runtime pattern pack for a language within an explicit base assets directory (`base/lang/{code}/patterns.json`). These packs are the runtime source of truth for [`crate::PatternLib
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `pattern_pack_path` (pub, L86)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `schema_file_path` (pub, L90)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_schema_file_path` (pub, L94)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_card_schema_file_path` (pub, L100)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `builtin_tools_dir` (pub, L105)
  - 役割: Same directory as the executable (debug) or its `tools/` subdirectory (release)
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `user_tools_dir` (pub, L120)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `tool_socket_dir` (pub, L124)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `builtin_plugins_dir` (pub, L129)
  - 役割: Same directory as the executable (debug) or its `plugins/` subdirectory (release).
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `user_plugins_dir` (pub, L144)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `plugin_socket_dir` (pub, L148)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `socket_dir_for` (private, L152)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_settings_path` (pub, L166)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_settings_path_in` (pub, L170)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_dir` (pub, L176)
  - 役割: Returns the directory containing the character card and runtime data (`assets_dir/characters/{name}/`).
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `character_dir_in` (pub, L180)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/patterns.rs`
- [x] `compile_forget_patterns` (private, L112)
  - 役割: Compiles every forget-pattern regex; a pattern whose regex fails to compile is skipped with a warning so a bad pack entry cannot break extraction.
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_embedded_patterns` (private, L176)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::load` (pub, L194)
  - 役割: Loads the pattern library for the given language code (e.g. `"en"`). The runtime pack at `assets/lang/{lang}/patterns.json` is preferred so patterns can be tuned without recompilin
  - 視点: 正常系 / 空
  - 既存テスト: load_prefers_runtime_asset_over_embedded, load_falls_back_to_embedded_when_asset_missing, load_falls_back_to_english_for_unknown_language, load_normalizes_language_aliases
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::load_from` (private, L221)
  - 役割: Loads the pattern library for `lang`, resolving the runtime pack against an explicit base assets directory (`base/lang/{code}/patterns.json`). This is the testable core of [`Patter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::load_from_assets` (private, L245)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::built_in` (pub, L268)
  - 役割: Returns the compile-time embedded pack for a language code. These are the same patterns shipped under `assets/lang/{code}/` but embedded at compile time as a fallback so the applic
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::built_in_english` (pub, L275)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::built_in_japanese` (pub, L279)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::lang` (pub, L283)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: load_falls_back_to_english_for_unknown_language, load_normalizes_language_aliases
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::forget_patterns` (pub, L287)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::compiled_forget_patterns` (pub, L293)
  - 役割: Forget-detection patterns with their regexes pre-compiled, in match order. Prefer this on hot paths; see [`Self::load`].
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `PatternLibrary::intent_keywords` (pub, L302)
  - 役割: Recall-intent keyword substrings for this language. One list per intent (`episodic`, `preference`, `relationship`, `affective`, `procedure`), matched case-insensitively as substrin
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/prompts.rs`
- [x] `resolve_language_alias` (pub, L13)
  - 役割: Resolves a free-form language tag to the directory code used under `assets/lang/`. Matching is case-insensitive and keeps only the primary subtag, so `"ja"`, `"JA"`, and `"ja-JP"`
  - 視点: 正常系 / 空
  - 既存テスト: resolve_language_alias_rejects_non_ascii_alphabetic
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `system_language` (pub, L34)
  - 役割: Resolves the app-wide default language from the OS locale, cached on first use.
  - 視点: 正常系 / 空
  - 既存テスト: resolve_system_language_selects_ja_only_for_japanese_locale
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `resolve_system_language` (pub, L45)
  - 役割: Maps an optional OS locale string to the app-wide default language. Only a primary subtag of `ja` selects Japanese; every other value keeps the English default. Kept pure so tests
  - 視点: 正常系 / None / 空
  - 既存テスト: resolve_system_language_selects_ja_only_for_japanese_locale
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `is_embedded_language` (pub(crate), L52)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/resources.rs`
- [x] `ensure_resource_dirs` (pub, L10)
  - 役割: In debug builds the source-tree `assets/` is used directly. In release builds, the assets are copied from a location next to the binary into the OS-standard data directory on first
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `find_source_dir` (private, L67)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `copy_dir_all` (private, L94)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/store.rs`
- [x] `Trait for ConfigStore::fmt` (private, L24)
  - 役割: Display/Debug
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `ConfigStore::load` (pub, L40)
  - 役割: Creates a new store by loading the global config from disk. Uses the standard figment pipeline (defaults → `settings.json` → `ENE_` env vars). On any extract failure, falls back to
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::try_load` (pub, L58)
  - 役割: Like [`Self::load`] but propagates the load error. Use this when the caller wants the user to see the error directly (e.g. CLI startup, where failing fast is preferable to silently
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::from_config` (pub, L66)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::config` (pub, L73)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::with_config_mut` (pub, L77)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::set_config` (pub, L82)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::get_section` (pub, L87)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::set_section` (pub, L94)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::flush_if_dirty` (pub, L110)
  - 役割: Saves the global config to disk if it has been modified. Returns `Ok(true)` if any write occurred, `Ok(false)` if nothing was dirty. The dirty flag is only cleared **after** a succ
  - 視点: 正常系 / エラー / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::flush` (pub, L121)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::is_dirty` (pub, L127)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `ConfigStore::mark_dirty` (pub, L133)
  - 役割: Use when the caller knows the in-memory state has diverged from disk and will call `flush_if_dirty` on the next cycle.
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-config` — `crates/ene-config/src/user_persona.rs`
- [x] `Trait for UserPersona::default` (private, L20)
  - 役割: Default
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-stage --lib (354 passed)

- [x] `UserPersona::render_lines` (pub, L36)
  - 役割: Single canonical field rendering shared by CBS `{{user_persona}}` macro expansion (empty prefix) and prompt-budget injection (`"- "` bullets) so the two never diverge. Empty option
  - 視点: 正常系 / 空
  - 既存テスト: render_lines_omits_empty_optional_fields, render_lines_applies_prefix_consistently
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-tray-linux` — `crates/ene-tray-linux/src/icon.rs`
- [x] `rgba_to_icon` (pub(crate), L3)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: rgba_to_icon_rotates_pixels
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-tray-linux` — `crates/ene-tray-linux/src/tray.rs`
- [x] `Trait for TrayService::id` (private, L66)
  - 役割: trivial getter
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: cargo test -p ene-tray-linux --lib (2 passed)

- [x] `Trait for TrayService::tool_tip` (private, L70)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-tray-linux --lib (2 passed)

- [x] `Trait for TrayService::icon_pixmap` (private, L80)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: cargo test -p ene-tray-linux --lib (2 passed)

- [x] `Trait for TrayService::menu` (private, L84)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系 / 空
  - 既存テスト: menu_slots_clone
  - 結果: cargo test -p ene-tray-linux --lib (2 passed)

- [x] `Trait for TrayService::activate` (private, L89)
  - 役割: thin accessor / one-line wrapper
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: cargo test -p ene-tray-linux --lib (2 passed)

- [x] `LinuxTrayHandle::spawn` (pub, L118)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: minimal_glb_loads_with_wgpu, companion_avatar_loads_the_minimal_fixture, silent_viseme_does_not_keep_the_overlay_dirty passed; lavapipe overlay render in lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LinuxTrayHandle::try_recv` (pub, L146)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LinuxTrayHandle::take_interactions` (pub, L151)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `LinuxTrayHandle::set_item_label` (pub, L159)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `build_ksni_menu` (private, L184)
  - 役割: read body; see 結果
  - 視点: 正常系 / 空
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-ctl` — `apps/ene-ctl/src/core.rs`
- [x] `pid_file_path` (pub, L42)
  - 役割: read body; see 結果
  - 視点: 正常系
  - 既存テスト: pid_file_path_is_under_data_dir
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `binary_in_dir` (pub, L47)
  - 役割: read body; see 結果
  - 視点: 正常系 / None
  - 既存テスト: binary_in_dir_finds_sibling
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `find_ene_core_binary` (pub, L52)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `parse_api_json` (pub, L74)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / 空
  - 既存テスト: parse_api_json_reads_url_and_token_file
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `read_api_ready` (pub, L78)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: read_api_ready_loads_token_from_sibling_file, read_api_ready_rejects_empty_url
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `wait_for_api_json` (pub, L100)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `process_alive` (private, L119)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `process_alive` (private, L124)
  - 役割: read body; see 結果
  - 視点: 正常系 / true/false
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `start_core` (pub, L129)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー / true/false / タイムアウト
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `stop_core` (pub, L189)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `kill_pid` (private, L211)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4

- [x] `kill_pid` (private, L222)
  - 役割: read body; see 結果
  - 視点: 正常系 / エラー
  - 既存テスト: なし
  - 結果: lavapipe smoke DISPLAY=:1 WGPU_BACKEND=vulkan: overlay PreMultiplied transparency, Companion+Chat windows, hover without cyan AABB, drag on avatar area, 15s idle stable; video stage-v2-gui-smoke-lavapipe.mp4







## `ene-stage-ui` — generated
- [x] `slint::include_modules!()` generated bindings (`crates/ene-stage-ui/src/lib.rs`)
  - 役割: Slint コンポーネントの生成コード。個別 fn は列挙しない
  - 視点: ホスト側のプロパティ / コールバック（chat, caption, spotlight, detail, overlay shell）
  - 既存テスト: なし（隔離クレート）
  - 結果: generated bindings; Chat/Detail Slint in GUI smoke; cargo test -p ene-stage --lib (354 passed)






