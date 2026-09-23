# PR #1698 テスト削減後の保持理由

2026-09-23 時点のソースにある Rust テスト 260 件を列挙する。保持理由は、そのテストを外すと直接の検証が失われる条件を記したもの。絶対的な必要性の証明ではない。

空行とコメントを除いた Rust 行数の概算は、製品コード 60,372 行、テストコード 15,091 行（4.00:1）。`/tests/` 以下、`tests.rs` などのテスト専用ファイル、`#[cfg(test)] mod tests` 以降をテストとして数えた。OS別の `cfg` もソース行として含む。

`cargo clippy --workspace --all-targets -- -D warnings` と `cargo test --workspace --quiet` は Linux 上で通過。Windows 条件付きテストの実行結果は PR の CI で別途確認する。

## `apps/ene-body/src/ipc.rs`

| テスト | 保持理由 |
| --- | --- |
| [`parent_commands_roundtrip`](../../../apps/ene-body/src/ipc.rs#L493) | 親コマンドのフレーム往復で型と内容が保たれることを確認 |
| [`body_events_roundtrip`](../../../apps/ene-body/src/ipc.rs#L526) | Bodyイベントのフレーム往復で型と内容が保たれることを確認 |
| [`trailing_bytes_are_the_next_frame`](../../../apps/ene-body/src/ipc.rs#L582) | 連結フレームの余剰バイトを次のフレームとして残すことを確認 |
| [`truncated_prefix_and_body_are_distinct`](../../../apps/ene-body/src/ipc.rs#L595) | 短い長さヘッダと短い本文を別の失敗にすることを確認 |
| [`oversize_claim_is_rejected_before_body_work`](../../../apps/ene-body/src/ipc.rs#L607) | 過大長宣言を本文処理前に拒否することを確認 |
| [`unknown_parent_variant_is_rejected`](../../../apps/ene-body/src/ipc.rs#L618) | 未知親コマンドを既知命令として実行しないことを確認 |
| [`invalid_placement_is_detectable`](../../../apps/ene-body/src/ipc.rs#L632) | 不正なBody配置を検出することを確認 |
| [`motion_set_shape_is_checked_before_any_file_is_opened`](../../../apps/ene-body/src/ipc.rs#L657) | モーション集合の構造検証がファイル操作に先行することを確認 |
| [`motion_set_lookup_is_pose_scoped`](../../../apps/ene-body/src/ipc.rs#L717) | ポーズ別モーション参照が他ポーズを選ばないことを確認 |
| [`unknown_motion_pose_and_extra_fields_are_rejected`](../../../apps/ene-body/src/ipc.rs#L734) | 未知ポーズや余分なフィールドを受理しないことを確認 |

## `apps/ene-body/src/lib.rs`

| テスト | 保持理由 |
| --- | --- |
| [`manifest_does_not_depend_on_host_or_client_protocol`](../../../apps/ene-body/src/lib.rs#L23) | Body独立プロセスにHost/Clientプロトコル依存を入れないことを確認 |

## `apps/ene-body/src/window/dwm.rs`

| テスト | 保持理由 |
| --- | --- |
| [`native_region_contains_only_supplied_visible_runs`](../../../apps/ene-body/src/window/dwm.rs#L706) | DWM入力領域が指定可視範囲だけを含むことを確認 |
| [`native_empty_region_owns_no_point`](../../../apps/ene-body/src/window/dwm.rs#L720) | 空のDWM領域が入力点を奪わないことを確認 |
| [`invalid_native_rectangle_fails_before_ownership_transfer`](../../../apps/ene-body/src/window/dwm.rs#L730) | 不正矩形で所有権を移す前に失敗することを確認 |
| [`input_region_refreshes_immediately_then_on_alternate_presented_frames`](../../../apps/ene-body/src/window/dwm.rs#L736) | 入力領域の初回即時更新と交互フレーム更新を確認 |

## `apps/ene-body/src/window/wayland.rs`

| テスト | 保持理由 |
| --- | --- |
| [`sync_output_wins_when_the_compositor_sends_it`](../../../apps/ene-body/src/window/wayland.rs#L1156) | Wayland compositorの同期出力を優先することを確認 |
| [`entered_output_is_the_sync_output_fallback`](../../../apps/ene-body/src/window/wayland.rs#L1161) | 同期出力欠落時にentered出力へ正しく戻ることを確認 |

## `apps/ene-body/tests/crash_isolation.rs`

| テスト | 保持理由 |
| --- | --- |
| [`dummy_parent_sees_disconnect_and_keeps_running_after_body_kill`](../../../apps/ene-body/tests/crash_isolation.rs#L15) | Body強制終了で親が切断を検知し生存することを確認 |
| [`dummy_parent_receives_ready_and_clean_exit_on_shutdown`](../../../apps/ene-body/tests/crash_isolation.rs#L59) | Body起動完了と正常停止通知を実IPCで確認 |
| [`unix_path_ipc_connects`](../../../apps/ene-body/tests/crash_isolation.rs#L136) | Unix path IPC接続が実際に成立することを確認 |

## `apps/ene-core/src/host_lock.rs`

| テスト | 保持理由 |
| --- | --- |
| [`second_acquisition_of_the_same_directory_is_already_running`](../../../apps/ene-core/src/host_lock.rs#L123) | 同じディレクトリの二重Host起動を拒否することを確認 |
| [`refused_startup_never_opens_the_store`](../../../apps/ene-core/src/host_lock.rs#L146) | 起動拒否がStoreを開く副作用を持たないことを確認 |
| [`refused_mutation_does_not_touch_presence_or_pendings`](../../../apps/ene-core/src/host_lock.rs#L173) | 二重所有拒否がPresenceと保留状態を変更しないことを確認 |
| [`second_serve_startup_is_refused_before_startup_mutation`](../../../apps/ene-core/src/host_lock.rs#L224) | 配信起動の所有権確認が起動時変更より先に行われることを確認 |

## `apps/ene-core/src/task_run.rs`

| テスト | 保持理由 |
| --- | --- |
| [`shutdown_joins_started_blocking_work_and_closes_launch_admission`](../../../apps/ene-core/src/task_run.rs#L1197) | 終了時に新規起動を閉じ既存blocking作業を待つことを確認 |
| [`admitted_launch_is_owned_until_shutdown`](../../../apps/ene-core/src/task_run.rs#L1266) | 開始済み子作業が終了まで監督対象から外れないことを確認 |
| [`dropping_launcher_aborts_owned_async_work`](../../../apps/ene-core/src/task_run.rs#L1299) | launcher破棄時に所有中の非同期作業を残さないことを確認 |
| [`emergency_abort_breaks_running_host_ownership_cycle`](../../../apps/ene-core/src/task_run.rs#L1320) | 緊急中断でHost参照循環を解消できることを確認 |
| [`join_failure_is_returned_only_after_remaining_children_finish`](../../../apps/ene-core/src/task_run.rs#L1352) | 一件失敗しても他の子作業を待ってから失敗を返すことを確認 |

## `apps/ene-core/tests/stage5_e2e.rs`

| テスト | 保持理由 |
| --- | --- |
| [`s5_01_disconnect_mid_wait_then_absence_completion_presents`](../../../apps/ene-core/tests/stage5_e2e.rs#L687) | 切断中の待機結果が不在完了として後から提示されることを確認 |
| [`s5_05_old_close_never_clears_new_current_both_orders`](../../../apps/ene-core/tests/stage5_e2e.rs#L844) | 旧接続closeと新認証の両順序で現行接続を守ることを確認 |
| [`s5_07_stale_round_input_and_ack_never_replay`](../../../apps/ene-core/tests/stage5_e2e.rs#L931) | 古いround入力とackを新接続へ再適用しないことを確認 |
| [`s5_09_progress_ack_never_presents_later_completion`](../../../apps/ene-core/tests/stage5_e2e.rs#L1007) | 進捗のackを後着完了の提示済み扱いにしないことを確認 |
| [`s5_17_18_resume_gates_and_retry_idempotency`](../../../apps/ene-core/tests/stage5_e2e.rs#L1166) | 再開条件と再試行の一回性を実接続で確認 |
| [`s5_19_late_arrival_stays_with_original_execution`](../../../apps/ene-core/tests/stage5_e2e.rs#L1397) | 遅延結果が別の実行に紐付かないことを確認 |
| [`s5_subscription_pushes_a_new_arrival_without_a_request`](../../../apps/ene-core/tests/stage5_e2e.rs#L1610) | 新着結果の通知が明示ポーリングなしに届くことを確認 |

## `apps/ene-core/tests/stage5_windows_pipe_e2e.rs`

| テスト | 保持理由 |
| --- | --- |
| [`pipe_bind_pair_authenticate_and_serve_the_current_connection`](../../../apps/ene-core/tests/stage5_windows_pipe_e2e.rs#L529) | Windows Pipeで結合・ペアリング・認証・現行接続処理が成立することを確認 |
| [`second_host_cannot_create_the_same_pipe`](../../../apps/ene-core/tests/stage5_windows_pipe_e2e.rs#L611) | 同じPipe名を二重Hostが所有できないことを確認 |
| [`superseded_pipe_connection_answers_typed_stale_connection`](../../../apps/ene-core/tests/stage5_windows_pipe_e2e.rs#L654) | Windowsの旧接続が型付き失効応答になることを確認 |
| [`reconnect_reauths_without_restoring_presence_and_a_fresh_summon_serves`](../../../apps/ene-core/tests/stage5_windows_pipe_e2e.rs#L749) | 再接続に再認証が必要でPresenceを勝手に戻さないことを確認 |
| [`os_peer_token_check_admits_a_same_user_pipe_client`](../../../apps/ene-core/tests/stage5_windows_pipe_e2e.rs#L806) | 同一ユーザーのOSトークン検査を実Pipeで確認 |

## `apps/ene-core/tests/stage6_e2e.rs`

| テスト | 保持理由 |
| --- | --- |
| [`stage6_targeted_deletion_completes_system_wide`](../../../apps/ene-core/tests/stage6_e2e.rs#L1395) | keep: checks client request, trusted confirmation, system-wide deletion, all-participant verification, and fresh origin through served Host |
| [`stage6_deletion_races_provider_wait_and_delayed_result`](../../../apps/ene-core/tests/stage6_e2e.rs#L1555) | 削除中の待機中推論結果が後から公開されないことを確認 |
| [`stage6_deletion_presentation_ack_after_condition_holds`](../../../apps/ene-core/tests/stage6_e2e.rs#L1653) | 削除条件成立後の提示確認が対象を復活させないことを確認 |
| [`stage6_deletion_during_learning_formation_never_forms_target_memory`](../../../apps/ene-core/tests/stage6_e2e.rs#L1730) | 学習形成中の削除で対象記憶が作られないことを確認 |
| [`stage6_delayed_formation_after_completion_is_refused`](../../../apps/ene-core/tests/stage6_e2e.rs#L1796) | 完了後に到着した旧形成結果の採用拒否を確認 |
| [`stage6_delayed_task_result_after_completion_is_collected`](../../../apps/ene-core/tests/stage6_e2e.rs#L1868) | 完了後に到着した旧タスク結果の回収を確認 |
| [`listener_abort_stops_the_deletion_driver`](../../../apps/ene-core/tests/stage6_e2e.rs#L1966) | Listener異常停止時に削除ドライバが孤児化しないことを確認 |
| [`restart_has_exactly_one_deletion_driver`](../../../apps/ene-core/tests/stage6_e2e.rs#L1989) | 再起動後の削除ドライバ重複起動を防ぐことを確認 |
| [`shutdown_waits_for_started_deletion_store_work`](../../../apps/ene-core/tests/stage6_e2e.rs#L2027) | 終了処理が開始済みStore書込の完了を待つことを確認 |
| [`stage6_client_incarnation_confirmed_in_serving_host_and_verified`](../../../apps/ene-core/tests/stage6_e2e.rs#L2319) | Client世代の消去証明が実Hostの完了条件になることを確認 |
| [`stage6_client_incarnation_unreachable_holds_across_restart`](../../../apps/ene-core/tests/stage6_e2e.rs#L2386) | 到達不能Clientを再起動で完了扱いにしないことを確認 |
| [`stage6_management_view_memory_body_is_a_required_client_participant`](../../../apps/ene-core/tests/stage6_e2e.rs#L2489) | 管理画面へ届いた本文を必須消去参加者に含めることを確認 |
| [`stage6_usage_cap_reservation_refuses_the_second_concurrent_send`](../../../apps/ene-core/tests/stage6_e2e.rs#L2743) | 同時要求の上限予約が二件目を確実に拒否することを確認 |
| [`stage6_usage_cap_unknown_accounting_and_update_currentness`](../../../apps/ene-core/tests/stage6_e2e.rs#L2934) | 未知使用量の計上と上限変更前提の現行性を確認 |
| [`stage6_dialogue_claim_before_completion_refuses_the_delayed_paraphrase`](../../../apps/ene-core/tests/stage6_e2e.rs#L3247) | 削除前に取得した推論claimの言い換え結果を拒否することを確認 |
| [`stage6_active_deletion_keeps_covered_context_from_the_provider`](../../../apps/ene-core/tests/stage6_e2e.rs#L3431) | 削除進行中に対象文脈をproviderへ送らないことを確認 |
| [`stage6_task_transient_observation_after_completion_is_collected`](../../../apps/ene-core/tests/stage6_e2e.rs#L3517) | 一時観測由来の遅延タスク結果を削除後に回収することを確認 |
| [`stage6_task_sealed_observation_paraphrase_is_erased_after_workspace_rewrite`](../../../apps/ene-core/tests/stage6_e2e.rs#L3664) | 元ファイル書換後も封印済み観測の言い換えを追跡して消すことを確認 |
| [`stage6_observation_write_across_deletion_stays_old_origin`](../../../apps/ene-core/tests/stage6_e2e.rs#L3800) | 削除をまたぐ観測書込が旧由来を失わないことを確認 |
| [`stage6_reconciliation_erases_paraphrase_pinned_past_the_page`](../../../apps/ene-core/tests/stage6_e2e.rs#L3955) | keep: checks late-pinned semantic Summary/Memory erasure through served Host and independent database reads |
| [`stage6_task_result_commits_under_the_credential_set_current_at_its_scrub`](../../../apps/ene-core/tests/stage6_e2e.rs#L4237) | タスク結果のscrubと保存が同じ資格情報集合に拘束されることを確認 |

## `apps/ene-desktop-ui/tests/surfaces.rs`

| テスト | 保持理由 |
| --- | --- |
| [`independent_surfaces_preserve_drafts_and_erase_native_trees`](../../../apps/ene-desktop-ui/tests/surfaces.rs#L15) | 独立画面の下書き保持とネイティブツリー消去を同時に確認 |

## `apps/ene-desktop/src/bin/ene-measure.rs`

| テスト | 保持理由 |
| --- | --- |
| [`mandatory_processes_and_outputs_parse`](../../../apps/ene-desktop/src/bin/ene-measure.rs#L493) | 計測CLIが必須プロセスと出力指定を受け取ることを確認 |
| [`terminal_feedback_after_the_window_end_still_counts`](../../../apps/ene-desktop/src/bin/ene-measure.rs#L524) | 窓内提出の完了応答が窓後でも集計されることを確認 |
| [`unresolved_submission_inside_the_window_is_missing`](../../../apps/ene-desktop/src/bin/ene-measure.rs#L548) | 窓内の未解決提出を欠測として扱うことを確認 |
| [`submissions_outside_the_window_are_ignored`](../../../apps/ene-desktop/src/bin/ene-measure.rs#L559) | 計測窓外の提出が結果を汚さないことを確認 |

## `apps/ene-desktop/src/host_launch.rs`

| テスト | 保持理由 |
| --- | --- |
| [`detach_does_not_kill_child_on_drop`](../../../apps/ene-desktop/src/host_launch.rs#L127) | 起動ハンドル破棄でHost子プロセスを殺さないことを確認 |

## `apps/ene-desktop/src/measure.rs`

| テスト | 保持理由 |
| --- | --- |
| [`pass_is_only_produced_after_all_evidence_is_evaluated`](../../../apps/ene-desktop/src/measure.rs#L1336) | 計測合格が全証拠の評価前に出ないことを確認 |
| [`missing_evidence_does_not_hide_independent_gate_failures`](../../../apps/ene-desktop/src/measure.rs#L1344) | 証拠欠落が別の不合格理由を隠さないことを確認 |
| [`missing_presentation_feedback_cannot_pass`](../../../apps/ene-desktop/src/measure.rs#L1384) | 提示フィードバック欠落を合格にしないことを確認 |
| [`unresolved_body_submission_becomes_missing_feedback`](../../../apps/ene-desktop/src/measure.rs#L1413) | 未解決のBody提出を提示済みと数えないことを確認 |
| [`dropped_frames_use_presented_count_not_request_count`](../../../apps/ene-desktop/src/measure.rs#L1444) | 落ちたフレームを要求数で水増ししないことを確認 |
| [`any_discarded_feedback_prevents_pass_even_above_thirty_fps`](../../../apps/ene-desktop/src/measure.rs#L1476) | 高FPSでも破棄された提示があれば合格にしないことを確認 |
| [`presentmon_missing_display_timing_is_not_presented`](../../../apps/ene-desktop/src/measure.rs#L1501) | 表示時刻のないPresentMon行を提示済みにしないことを確認 |
| [`presentmon_default_millisecond_columns_use_fixed_window`](../../../apps/ene-desktop/src/measure.rs#L1517) | PresentMonのミリ秒列を固定計測窓へ正しく換算することを確認 |
| [`linux_sampler_reads_current_process`](../../../apps/ene-desktop/src/measure.rs#L1540) | Linux計測器が対象プロセスを取り違えないことを確認 |

## `apps/ene-desktop/tests/bundled_asset.rs`

| テスト | 保持理由 |
| --- | --- |
| [`bundled_asset_is_seed_san_vrm_1_with_required_features`](../../../apps/ene-desktop/tests/bundled_asset.rs#L16) | 配布VRMの実体と必要機能が揃うことを確認 |

## `apps/ene-desktop/tests/stage7_c1.rs`

| テスト | 保持理由 |
| --- | --- |
| [`memory_gui_confirms_acceptance_3_1_to_3_10`](../../../apps/ene-desktop/tests/stage7_c1.rs#L383) | 実GUIで記憶の登録・編集・履歴・忘却が一連で成立することを確認 |
| [`memory_gui_pages_at_the_host_instead_of_scanning`](../../../apps/ene-desktop/tests/stage7_c1.rs#L698) | 記憶一覧のページングがHost側で境界付けられ全件走査に戻らないことを確認 |
| [`learning_barrier_gui_does_not_invent_formation`](../../../apps/ene-desktop/tests/stage7_c1.rs#L795) | 学習保留時にGUIが未成立の記憶を表示しないことを確認 |

## `apps/ene-desktop/tests/stage7_c2.rs`

| テスト | 保持理由 |
| --- | --- |
| [`acceptance_4_workspace_task_gui_path`](../../../apps/ene-desktop/tests/stage7_c2.rs#L295) | ワークスペース選択からタスク結果までの実GUI経路を確認 |
| [`cancel_admission_is_not_stop_complete`](../../../apps/ene-desktop/tests/stage7_c2.rs#L423) | 取消受付を停止完了と表示しないことを確認 |
| [`gui_close_reconnect_and_ack_only_after_present`](../../../apps/ene-desktop/tests/stage7_c2.rs#L487) | GUI再接続後も実提示前に受領確認しないことを確認 |
| [`resume_is_bound_to_the_displayed_premise`](../../../apps/ene-desktop/tests/stage7_c2.rs#L593) | 再開承認が表示された前提と同じ対象に限られることを確認 |

## `apps/ene-desktop/tests/stage7_e.rs`

| テスト | 保持理由 |
| --- | --- |
| [`targeted_deletion_wipes_gui_copies_and_reports_wiped_after_erase`](../../../apps/ene-desktop/tests/stage7_e.rs#L258) | 削除後のGUIキャッシュ消去と完了表示の順序を確認 |
| [`receive_without_present_is_not_presented_ack`](../../../apps/ene-desktop/tests/stage7_e.rs#L382) | 受信しただけの本文を提示済みと誤認しないことを確認 |
| [`host_restart_and_reconnect_delivery_evidence_holds`](../../../apps/ene-desktop/tests/stage7_e.rs#L435) | 再起動と再接続をまたぐ受信証拠の保持をGUI境界で確認 |
| [`registered_secret_never_appears_on_any_page`](../../../apps/ene-desktop/tests/stage7_e.rs#L471) | 登録済み秘密がどのGUIページにも出ないことを確認 |
| [`killing_body_leaves_chat_settings_and_cancel_alive`](../../../apps/ene-desktop/tests/stage7_e.rs#L510) | Body異常終了がチャット・設定・取消を巻き込まないことを確認 |
| [`closed_confirmation_cannot_be_reused`](../../../apps/ene-desktop/tests/stage7_e.rs#L561) | 閉じた確認を再利用して別の操作を確定できないことを確認 |
| [`stale_rows_do_not_select_another_task_or_memory`](../../../apps/ene-desktop/tests/stage7_e.rs#L585) | 古い行選択が別タスク・記憶に化けないことを確認 |

## `apps/ene-desktop/tests/stage7_f_motion.rs`

| テスト | 保持理由 |
| --- | --- |
| [`the_bundled_motion_pack_is_projected_to_the_body`](../../../apps/ene-desktop/tests/stage7_f_motion.rs#L23) | 配布モーションがBodyへ実際に渡ることを確認 |

## `crates/ene-action/src/filesystem.rs`

| テスト | 保持理由 |
| --- | --- |
| [`traversal_and_absolute_requests_are_malformed`](../../../crates/ene-action/src/filesystem.rs#L715) | 作業領域からの相対逸脱と絶対パスを不正要求として拒否することを確認 |
| [`read_and_edit_require_an_existing_regular_file`](../../../crates/ene-action/src/filesystem.rs#L738) | 読出し・編集の対象を既存の通常ファイルへ限定することを確認 |
| [`create_requires_a_contained_existing_parent_and_an_absent_destination`](../../../crates/ene-action/src/filesystem.rs#L761) | 新規作成で内包された親と未使用の宛先を要求することを確認 |
| [`create_refuses_a_reentry_symlink_path`](../../../crates/ene-action/src/filesystem.rs#L794) | 再入symlink経路から作業領域外へ抜けないことを確認 |
| [`create_refuses_a_reentry_symlink_path`](../../../crates/ene-action/src/filesystem.rs#L815) | 再入symlink経路から作業領域外へ抜けないことを確認 |
| [`symlinks_that_leave_the_workspace_are_refused`](../../../crates/ene-action/src/filesystem.rs#L842) | 領域外を指すsymlinkを操作対象にしないことを確認 |
| [`execute_reads_and_writes_with_honest_effects`](../../../crates/ene-action/src/filesystem.rs#L873) | 実ファイル操作と報告効果が一致することを確認 |
| [`execute_maps_missing_reads_to_confirmed_failure_without_an_effect`](../../../crates/ene-action/src/filesystem.rs#L948) | 存在しないファイルの読出しを効果なしの確認済み失敗にすることを確認 |
| [`forged_outside_targets_cannot_read_or_list_at_effect_time`](../../../crates/ene-action/src/filesystem.rs#L969) | 検証済み対象を偽装しても実行時に領域外を読めないことを確認 |
| [`list_requires_a_directory_and_observes_a_sorted_listing`](../../../crates/ene-action/src/filesystem.rs#L1006) | 一覧対象をディレクトリに限り決定的な順序で返すことを確認 |
| [`list_excludes_symlink_entries_without_following_them`](../../../crates/ene-action/src/filesystem.rs#L1049) | 一覧でsymlinkを辿らず結果から除くことを確認 |
| [`linux_mount_boundary_rejects_nested_mount_points`](../../../crates/ene-action/src/filesystem.rs#L1077) | Linuxの入れ子mountを領域境界の内側と誤認しないことを確認 |
| [`windows_inside_targets_share_the_root_volume_serial`](../../../crates/ene-action/src/filesystem.rs#L1111) | Windows上の対象がルートと同じvolumeに属することを確認 |
| [`windows_reparse_points_are_excluded_and_escapes_refused`](../../../crates/ene-action/src/filesystem.rs#L1137) | Windows再解析点経由の領域外逸脱を拒否することを確認 |

## `crates/ene-api/src/v1/deletion.rs`

| テスト | 保持理由 |
| --- | --- |
| [`deletion_target_roundtrips_with_colons_and_unicode`](../../../crates/ene-api/src/v1/deletion.rs#L363) | 削除対象のcolonとUnicodeをwire往復で保持することを確認 |
| [`deletion_target_parser_rejects_other_families_and_shapes`](../../../crates/ene-api/src/v1/deletion.rs#L384) | 異なる削除対象種別や不正構文を拒否することを確認 |
| [`deletion_target_bounds_the_exact_text`](../../../crates/ene-api/src/v1/deletion.rs#L406) | 正確な削除対象文字列の長さを制限することを確認 |
| [`parsed_request_debug_redacts_the_owner_body`](../../../crates/ene-api/src/v1/deletion.rs#L421) | 解析済み削除要求の本文をDebug出力から隠すことを確認 |
| [`phase_tokens_are_stable`](../../../crates/ene-api/src/v1/deletion.rs#L441) | 削除phaseのwire語彙が意図せず変わらないことを確認 |
| [`client_temp_classes_round_trip_through_their_closed_vocabulary`](../../../crates/ene-api/src/v1/deletion.rs#L449) | Client一時データ種別を閉じた語彙で往復させることを確認 |

## `crates/ene-api/src/v1/handshake.rs`

| テスト | 保持理由 |
| --- | --- |
| [`auth_proof_debug_redacts_the_proof`](../../../crates/ene-api/src/v1/handshake.rs#L134) | 認証証明をDebug出力から隠すことを確認 |
| [`pairing_provision_secret_debug_is_redacted`](../../../crates/ene-api/src/v1/handshake.rs#L146) | ペアリング配布秘密をDebug出力から隠すことを確認 |

## `crates/ene-api/src/v1/management.rs`

| テスト | 保持理由 |
| --- | --- |
| [`intent_debug_redacts_rationale`](../../../crates/ene-api/src/v1/management.rs#L470) | 管理意図の理由本文をDebug出力に出さないことを確認 |
| [`omitted_confirmed_deserializes_false_and_true_is_never_host_confirmation`](../../../crates/ene-api/src/v1/management.rs#L491) | confirmed省略をfalseとし外部trueもHost確認に格上げしないことを確認 |
| [`deletion_intent_debug_redacts_the_owner_body_target`](../../../crates/ene-api/src/v1/management.rs#L516) | 削除意図の対象本文をDebug表示で伏せることを確認 |
| [`section_debug_redacts_body`](../../../crates/ene-api/src/v1/management.rs#L541) | 管理セクション本文をDebug表示で伏せることを確認 |
| [`credential_target_spelling_and_parse_preserve_the_label`](../../../crates/ene-api/src/v1/management.rs#L559) | 資格情報対象のラベルがwire往復で変わらないことを確認 |
| [`consent_target_spelling_and_parse_preserve_the_credential_id`](../../../crates/ene-api/src/v1/management.rs#L569) | 同意対象の資格情報IDがwire往復で変わらないことを確認 |
| [`credential_parser_rejects_blanks_and_wrong_shapes`](../../../crates/ene-api/src/v1/management.rs#L584) | 空白や形違いの資格情報対象を拒否することを確認 |
| [`consent_parser_rejects_blanks_and_wrong_shapes`](../../../crates/ene-api/src/v1/management.rs#L600) | 空白や形違いの同意対象を拒否することを確認 |
| [`task_target_roundtrips_and_rejects_other_text`](../../../crates/ene-api/src/v1/management.rs#L619) | タスク対象の往復と異種文字列拒否を確認 |
| [`workspace_target_roundtrips_with_colons_and_rejects_empty`](../../../crates/ene-api/src/v1/management.rs#L639) | colon入り作業領域を保持し空対象を拒否することを確認 |
| [`usage_cap_target_roundtrips_both_scopes`](../../../crates/ene-api/src/v1/management.rs#L658) | 上限対象の両scopeをwire往復で区別することを確認 |
| [`usage_cap_target_rejects_other_shapes_and_never_guesses`](../../../crates/ene-api/src/v1/management.rs#L689) | 不正な上限対象を推測補完せず拒否することを確認 |

## `crates/ene-api/src/v1/round.rs`

| テスト | 保持理由 |
| --- | --- |
| [`input_debug_keeps_refs_and_redacts_body`](../../../crates/ene-api/src/v1/round.rs#L263) | round入力の参照は残し本文だけ伏せることを確認 |
| [`frame_debug_redacts_delta`](../../../crates/ene-api/src/v1/round.rs#L284) | stream frameの本文差分をDebug出力から隠すことを確認 |
| [`history_item_debug_redacts_text`](../../../crates/ene-api/src/v1/round.rs#L300) | 履歴項目の本文をDebug出力から隠すことを確認 |
| [`presentation_detail_debug_redacts_value`](../../../crates/ene-api/src/v1/round.rs#L319) | 提示詳細の値をDebug出力から隠すことを確認 |

## `crates/ene-client/src/tests.rs`

| テスト | 保持理由 |
| --- | --- |
| [`session_starts_unobserved_and_tracks_latest`](../../../crates/ene-client/src/tests.rs#L63) | 未観測状態から最新提示までのClient状態遷移を確認 |
| [`local_erasure_demand_wipes_the_deferred_buffer_and_reports_classes`](../../../crates/ene-client/src/tests.rs#L100) | 消去要求で保留本文を消し消去クラスを正しく報告することを確認 |
| [`boot_incarnation_is_one_per_process_and_advances_per_boot`](../../../crates/ene-client/src/tests.rs#L161) | 同一プロセスのClient世代が一つで次回起動時に進むことを確認 |
| [`concurrent_boot_advances_serialize_without_loss`](../../../crates/ene-client/src/tests.rs#L234) | 同時起動でも世代の更新が失われないことを確認 |
| [`prepared_retry_reuses_command_with_fresh_transport_ids`](../../../crates/ene-client/src/tests.rs#L307) | 再試行が同一操作IDと新しい輸送IDを両立することを確認 |
| [`decide_frame_classifies_facts_answers_and_deferrals`](../../../crates/ene-client/src/tests.rs#L361) | 着信フレームを事実・応答・保留へ誤分類しないことを確認 |
| [`proof_derives_from_the_secret_and_the_single_use_nonce`](../../../crates/ene-client/src/tests.rs#L398) | 認証証明が秘密と一回用nonceの両方に結び付くことを確認 |
| [`session_debug_redacts_the_secret`](../../../crates/ene-client/src/tests.rs#L417) | ClientセッションのDebug出力に秘密を出さないことを確認 |

## `crates/ene-client/src/transport.rs`

| テスト | 保持理由 |
| --- | --- |
| [`pipe_name_is_stable_and_directory_scoped`](../../../crates/ene-client/src/transport.rs#L752) | Windows Pipe名が同一ディレクトリで安定し別ディレクトリと衝突しないことを確認 |

## `crates/ene-companion/src/dialogue.rs`

| テスト | 保持理由 |
| --- | --- |
| [`a_completed_report_names_the_changes_the_location_and_no_remainder`](../../../crates/ene-companion/src/dialogue.rs#L1564) | 完了報告が変更内容と場所を示し残余なしを表すことを確認 |
| [`an_unknown_effect_is_reported_as_unknown_not_as_success_or_failure`](../../../crates/ene-companion/src/dialogue.rs#L1596) | 結果不明を成功や失敗へ偽装しないことを確認 |
| [`the_debug_rendering_redacts_the_result_body`](../../../crates/ene-companion/src/dialogue.rs#L1635) | 結果本文をDebug出力で伏せることを確認 |
| [`an_ordinary_reply_is_conversation_unchanged`](../../../crates/ene-companion/src/dialogue.rs#L1662) | 通常会話を管理コマンドとして誤解析しないことを確認 |
| [`a_marker_after_prose_is_invalid`](../../../crates/ene-companion/src/dialogue.rs#L1672) | 先頭以外のマーカーをコマンドとして受理しないことを確認 |
| [`every_closed_world_command_parses_from_the_first_line`](../../../crates/ene-companion/src/dialogue.rs#L1691) | 許可された全コマンド形式が先頭行から解析されることを確認 |
| [`extra_fields_are_invalid_for_every_command_shape`](../../../crates/ene-companion/src/dialogue.rs#L1728) | 各コマンドへの余分なフィールド注入を拒否することを確認 |
| [`malformed_or_non_first_markers_are_invalid`](../../../crates/ene-companion/src/dialogue.rs#L1748) | 壊れたマーカーと非先頭マーカーを拒否することを確認 |
| [`the_command_debug_redacts_instruction_bodies`](../../../crates/ene-companion/src/dialogue.rs#L1768) | コマンド内指示本文をDebug出力で伏せることを確認 |

## `crates/ene-credential/src/os_store.rs`

| テスト | 保持理由 |
| --- | --- |
| [`the_item_name_carries_the_version`](../../../crates/ene-credential/src/os_store.rs#L267) | OS秘密ストア項目名が版を含み上書きを避けることを確認 |
| [`the_namespace_separates_installations`](../../../crates/ene-credential/src/os_store.rs#L275) | 別インストールのOS秘密が混ざらないことを確認 |
| [`a_bare_put_is_refused_without_touching_the_store`](../../../crates/ene-credential/src/os_store.rs#L286) | 版管理なしの直接書込を副作用なしで拒否することを確認 |
| [`a_read_without_an_active_version_is_unavailable`](../../../crates/ene-credential/src/os_store.rs#L305) | 有効版なしの読出しを利用不能として扱うことを確認 |
| [`the_real_os_store_round_trips_when_available`](../../../crates/ene-credential/src/os_store.rs#L322) | 利用可能な実OS秘密ストアとの往復を確認 |

## `crates/ene-credential/src/scrub.rs`

| テスト | 保持理由 |
| --- | --- |
| [`registered_values_are_redacted_and_revision_names_the_set`](../../../crates/ene-credential/src/scrub.rs#L332) | 登録値の伏せ字と証明対象の集合改訂を確認 |
| [`the_longer_value_is_removed_whole`](../../../crates/ene-credential/src/scrub.rs#L346) | 重なる秘密文字列で長い値を部分残存させないことを確認 |
| [`an_unreadable_registry_fails_closed`](../../../crates/ene-credential/src/scrub.rs#L359) | 秘密台帳の読出し不能を安全側の拒否へ変換することを確認 |
| [`an_empty_bearer_cannot_prove_absence`](../../../crates/ene-credential/src/scrub.rs#L372) | 空bearerを秘密不存在の証明に使わないことを確認 |
| [`proof_construction_is_conservative_only`](../../../crates/ene-credential/src/scrub.rs#L380) | 伏せ字証明が根拠のある範囲に限られることを確認 |
| [`revision_exhaustion_reports_none_instead_of_aliasing`](../../../crates/ene-credential/src/scrub.rs#L411) | 改訂番号枯渇時に過去の版へ巻き戻らないことを確認 |

## `crates/ene-credential/src/tests.rs`

| テスト | 保持理由 |
| --- | --- |
| [`availability_requires_both_registry_and_store`](../../../crates/ene-credential/src/tests.rs#L24) | 資格情報の利用可能性に台帳と秘密ストアの両方を要求することを確認 |
| [`credential_ref_grammar_is_fixed`](../../../crates/ene-credential/src/tests.rs#L54) | 資格情報参照に曖昧な構文を許さないことを確認 |
| [`bearer_closure_receives_the_inserted_secret`](../../../crates/ene-credential/src/tests.rs#L76) | 秘密値が許可されたbearer閉包だけへ渡ることを確認 |
| [`active_version_is_an_immutable_snapshot_not_a_fresh_backend_read`](../../../crates/ene-credential/src/tests.rs#L85) | 有効版の取得が後続のbackend変更で揺れないことを確認 |
| [`failed_snapshot_publication_does_not_fall_back_to_the_old_version`](../../../crates/ene-credential/src/tests.rs#L106) | 新版公開失敗時に旧版へ暗黙に戻さないことを確認 |
| [`pairing_proof_conforms_to_rfc4231_and_rejects_mismatch`](../../../crates/ene-credential/src/tests.rs#L129) | ペアリング証明の標準ベクトルと不一致拒否を確認 |
| [`device_auth_roundtrip_preserves_secret_bytes`](../../../crates/ene-credential/src/tests.rs#L182) | 端末認証秘密の永続化往復でバイト列が変わらないことを確認 |
| [`device_auth_malformed_files_error_never_default`](../../../crates/ene-credential/src/tests.rs#L201) | 壊れた認証ファイルを空の既定値として扱わないことを確認 |
| [`device_auth_open_tightens_lax_permissions`](../../../crates/ene-credential/src/tests.rs#L264) | 緩いファイル権限を開く際に制限することを確認 |
| [`device_auth_concurrent_approvals_keep_both_devices`](../../../crates/ene-credential/src/tests.rs#L281) | 異なる端末の同時承認を片方失わないことを確認 |
| [`device_auth_same_device_rotations_leave_one_current_secret`](../../../crates/ene-credential/src/tests.rs#L317) | 同じ端末の同時更新で有効秘密が一つだけ残ることを確認 |
| [`device_auth_mutation_lock_serializes_writers`](../../../crates/ene-credential/src/tests.rs#L361) | 認証ファイルの書込がロックで直列化されることを確認 |
| [`device_auth_write_failure_recovers_on_the_next_approval`](../../../crates/ene-credential/src/tests.rs#L408) | 書込失敗後の次回承認で状態が回復することを確認 |
| [`device_auth_debug_carries_no_secret_or_descriptor`](../../../crates/ene-credential/src/tests.rs#L433) | 認証状態のDebugに秘密や識別子を載せないことを確認 |
| [`memory_put_stores_without_echoing_the_secret_in_debug`](../../../crates/ene-credential/src/tests.rs#L454) | メモリ秘密ストアの書込とDebug伏せ字を確認 |
| [`lookup_respects_provider_gating_and_presence`](../../../crates/ene-credential/src/tests.rs#L474) | provider指定と存在状態で秘密参照を制限することを確認 |
| [`put_fail_closes_and_never_echoes_the_secret`](../../../crates/ene-credential/src/tests.rs#L494) | 秘密保存失敗時に拒否し値をエラーへ出さないことを確認 |

## `crates/ene-inference/src/cost.rs`

| テスト | 保持理由 |
| --- | --- |
| [`components_and_total_do_not_double_count_cached_input`](../../../crates/ene-inference/src/cost.rs#L288) | cached入力の費用を通常入力と二重計上しないことを確認 |
| [`fractional_micro_units_round_up_exactly_once_per_component`](../../../crates/ene-inference/src/cost.rs#L322) | 端数の切上げを構成要素ごとに一回だけ行うことを確認 |
| [`large_counts_and_rates_fail_closed_instead_of_wrapping`](../../../crates/ene-inference/src/cost.rs#L347) | 大きな利用量で算術wrapを起こさず拒否することを確認 |
| [`total_overflow_fails_closed_even_when_each_component_fits`](../../../crates/ene-inference/src/cost.rs#L365) | 構成要素は収まっても合計overflowなら拒否することを確認 |
| [`unknown_usage_or_missing_rate_stays_unknown_never_zero`](../../../crates/ene-inference/src/cost.rs#L378) | 不明利用量や価格欠落を0円へ変換しないことを確認 |
| [`another_models_rate_is_never_applied`](../../../crates/ene-inference/src/cost.rs#L409) | 異なるmodelの料金を流用しないことを確認 |
| [`malformed_usage_facts_are_refused_not_zero_filled`](../../../crates/ene-inference/src/cost.rs#L431) | 不正利用量を0埋めして確定しないことを確認 |
| [`cost_follows_the_bound_snapshot_not_the_current_catalog`](../../../crates/ene-inference/src/cost.rs#L451) | 費用を現行価格表で再計算せず入場時snapshotへ拘束することを確認 |
| [`estimate_upper_bound_charges_the_more_expensive_input_rate`](../../../crates/ene-inference/src/cost.rs#L502) | 上限見積りに高い入力単価を使うことを確認 |
| [`estimate_upper_bound_never_falls_below_the_settled_total`](../../../crates/ene-inference/src/cost.rs#L527) | 上限見積りが確定費用を下回らないことを確認 |
| [`estimate_upper_bound_rounds_up_and_fails_closed_on_overflow`](../../../crates/ene-inference/src/cost.rs#L575) | 見積りの切上げとoverflow時拒否を確認 |

## `crates/ene-inference/src/lib.rs`

| テスト | 保持理由 |
| --- | --- |
| [`priced_route_claims_with_the_reviewed_snapshot`](../../../crates/ene-inference/src/lib.rs#L1790) | 価格付き推論claimが審査済みsnapshotを保持することを確認 |
| [`held_and_indeterminate_cap_admissions_stay_distinct_refusals`](../../../crates/ene-inference/src/lib.rs#L1825) | 上限保留と結果不明を別の拒否理由として保持することを確認 |
| [`transport_failure_records_unknown_counts`](../../../crates/ene-inference/src/lib.rs#L1883) | 輸送失敗の利用量を未知として保存することを確認 |
| [`stale_credential_set_never_calls_the_provider`](../../../crates/ene-inference/src/lib.rs#L1908) | 秘密集合が古い場合にprovider呼出し自体を止めることを確認 |
| [`completed_records_reported_counts_even_when_adoption_moves`](../../../crates/ene-inference/src/lib.rs#L1938) | 採用先が変わっても完了試行の利用量を記録することを確認 |
| [`task_agent_claim_carries_the_durable_correlation`](../../../crates/ene-inference/src/lib.rs#L1977) | Task Agentのclaimが永続的な相関IDを持つことを確認 |
| [`abort_during_the_provider_wait_records_unknown_usage_and_drops_the_call`](../../../crates/ene-inference/src/lib.rs#L2023) | provider待機中の中断で未知利用量を記録し呼出しを破棄することを確認 |

## `crates/ene-inference/src/provider.rs`

| テスト | 保持理由 |
| --- | --- |
| [`concatenates_output_text_and_reports_usage`](../../../crates/ene-inference/src/provider.rs#L551) | provider断片を順序通り連結し利用量を保持することを確認 |
| [`request_body_disables_server_side_storage`](../../../crates/ene-inference/src/provider.rs#L591) | providerへの要求でサーバー側保存を無効化することを確認 |
| [`usage_estimate_bounds_input_by_bytes_plus_framing_and_output_by_the_request_maximum`](../../../crates/ene-inference/src/provider.rs#L602) | 見積りが入力フレームと最大出力を上限に含めることを確認 |
| [`non_completed_statuses_fail_without_text_or_usage`](../../../crates/ene-inference/src/provider.rs#L629) | 未完了statusを本文や利用量付き成功と見なさないことを確認 |
| [`rate_limited_maps_to_unavailable`](../../../crates/ene-inference/src/provider.rs#L695) | rate limitを利用不能に分類することを確認 |
| [`malformed_body_maps_to_decode_failure`](../../../crates/ene-inference/src/provider.rs#L709) | 不正provider本文をdecode失敗に分類することを確認 |
| [`stream_assembler_emits_deltas_in_order_and_reports_usage`](../../../crates/ene-inference/src/provider.rs#L725) | stream差分の順序と最終利用量を確認 |
| [`stream_assembler_maps_failure_events_without_body_text`](../../../crates/ene-inference/src/provider.rs#L751) | 失敗イベントの本文を漏らさず失敗へ変換することを確認 |
| [`debug_rendering_carries_no_bearer_material`](../../../crates/ene-inference/src/provider.rs#L779) | provider構造体のDebugからbearer値を除くことを確認 |
| [`bearer_stays_at_the_http_authorization_boundary`](../../../crates/ene-inference/src/provider.rs#L887) | bearerがHTTP認証境界以外の要求本文へ出ないことを確認 |

## `crates/ene-permission/src/lib.rs`

| テスト | 保持理由 |
| --- | --- |
| [`matching_consent_allows_for_exactly_one_use`](../../../crates/ene-permission/src/lib.rs#L707) | keep: matching consent mints an evaluation token consumed once |
| [`unknown_id_does_not_consume`](../../../crates/ene-permission/src/lib.rs#L725) | keep: a token from another tracker cannot consume this tracker admission |
| [`mismatched_fingerprint_rejects_without_burning_the_id`](../../../crates/ene-permission/src/lib.rs#L743) | keep: wrong fingerprint is refused without consuming the valid one-use token |
| [`stale_expected_consent_needs_revalidation`](../../../crates/ene-permission/src/lib.rs#L762) | keep: stale expected revision yields revalidation |
| [`missing_expected_consent_needs_revalidation_when_stored_exists`](../../../crates/ene-permission/src/lib.rs#L777) | keep: absent expected premise cannot silently use existing consent |
| [`provider_mismatch_denies_as_stale_consent`](../../../crates/ene-permission/src/lib.rs#L792) | keep: provider mismatch yields typed stale-consent denial |
| [`missing_stored_consent_denies_as_stale_consent`](../../../crates/ene-permission/src/lib.rs#L810) | keep: removed consent cannot authorize a pending attempt |
| [`learning_formation_requires_a_learning_consent`](../../../crates/ene-permission/src/lib.rs#L833) | keep: learning candidate consumes only a learning-scoped evaluation |
| [`dialogue_consent_never_authorizes_learning`](../../../crates/ene-permission/src/lib.rs#L855) | keep: dialogue consent cannot cross the capability boundary |
| [`dialogue_and_learning_consumers_are_not_interchangeable`](../../../crates/ene-permission/src/lib.rs#L876) | keep: consumer/capability/purpose triples are closed-world |
| [`task_agent_turn_inherits_the_dialogue_consent`](../../../crates/ene-permission/src/lib.rs#L923) | keep: task agent inherits dialogue consent but receives its own one-use fingerprint |
| [`task_agent_turn_is_not_allowed_with_a_learning_consent`](../../../crates/ene-permission/src/lib.rs#L945) | keep: learning consent cannot authorize task-agent work |
| [`task_agent_turn_does_not_masquerade_as_dialogue_or_learning`](../../../crates/ene-permission/src/lib.rs#L963) | keep: task-agent triple cannot masquerade as dialogue or learning |
| [`consumer_and_purpose_storage_names_are_closed_world`](../../../crates/ene-permission/src/lib.rs#L1004) | keep: durable attribution vocabulary round-trips and rejects unknown tags |
| [`base_view_expectation_covers_capability_marks_and_stale_faces`](../../../crates/ene-permission/src/lib.rs#L1024) | keep: capability-specific marks and stale faces map to distinct expectations |
| [`mark_helpers_round_trip_both_capabilities`](../../../crates/ene-permission/src/lib.rs#L1089) | keep: combined marks parse regardless of segment order and cannot cross capability |
| [`revision_exhaustion_is_reported_not_aliased`](../../../crates/ene-permission/src/lib.rs#L1130) | keep: consent revision cannot wrap and alias a previous premise |

## `crates/ene-plugin-ipc/src/lib.rs`

| テスト | 保持理由 |
| --- | --- |
| [`roundtrip_preserves_envelope_and_payload`](../../../crates/ene-plugin-ipc/src/lib.rs#L161) | keep: frame payload and envelope survive the codec round trip |
| [`prefix_is_big_endian_body_length`](../../../crates/ene-plugin-ipc/src/lib.rs#L170) | keep: exact on-wire prefix format is stable |
| [`short_prefix_is_truncated_needing_four`](../../../crates/ene-plugin-ipc/src/lib.rs#L182) | keep: prefix truncation reports the correct need without body decode |
| [`short_body_is_truncated_needing_frame_total`](../../../crates/ene-plugin-ipc/src/lib.rs#L196) | keep: body truncation reports the total frame need |
| [`oversize_prefix_rejected_without_large_read`](../../../crates/ene-plugin-ipc/src/lib.rs#L212) | keep: oversize claimed length is rejected before body-sized work |
| [`corrupt_body_is_decode_failed_without_payload_echo`](../../../crates/ene-plugin-ipc/src/lib.rs#L226) | keep: corrupt body causes typed decode failure without leaking payload bytes |
| [`concatenated_frames_decode_sequentially`](../../../crates/ene-plugin-ipc/src/lib.rs#L249) | keep: decoder consumes exactly one frame from a concatenated stream |
| [`oversize_body_rejected_on_encode`](../../../crates/ene-plugin-ipc/src/lib.rs#L266) | keep: encoder enforces the frame cap on an actual oversized payload |

## `crates/ene-preservation/src/request.rs`

| テスト | 保持理由 |
| --- | --- |
| [`staged_request_debug_never_renders_the_target_body`](../../../crates/ene-preservation/src/request.rs#L330) | 削除要求の途中状態をDebug表示しても対象本文を漏らさないことを確認 |
| [`confirmation_binds_to_its_own_request_identity`](../../../crates/ene-preservation/src/request.rs#L340) | 確認が別の削除要求へ流用されないことを確認 |
| [`staging_command_exposes_no_confirmation`](../../../crates/ene-preservation/src/request.rs#L375) | 要求の受付段階で確定権限を配らないことを確認 |

## `crates/ene-store/src/migrate.rs`

| テスト | 保持理由 |
| --- | --- |
| [`initialize_once_and_reject_unsupported_schemas_without_changes`](../../../crates/ene-store/src/migrate.rs#L645) | Store初期化が一回で未対応schemaを無変更で拒否することを確認 |
| [`failed_initialization_rolls_back_and_can_be_retried`](../../../crates/ene-store/src/migrate.rs#L673) | 初期化失敗時のrollbackと再試行可能性を確認 |
| [`concurrent_initializers_publish_one_schema_and_seed`](../../../crates/ene-store/src/migrate.rs#L702) | 同時初期化でschemaとseedが二重作成されないことを確認 |

## `crates/ene-store/src/tests.rs`

| テスト | 保持理由 |
| --- | --- |
| [`supersession_probe_is_index_backed_not_a_scan`](../../../crates/ene-store/src/tests.rs#L139) | 接続置換判定が索引境界で完結し全件走査へ退化しないことを確認 |
| [`load_message_reads_one_row_by_primary_key_and_fails_closed`](../../../crates/ene-store/src/tests.rs#L202) | 本文読出しが主キー一件に限定され欠損時に閉じることを確認 |
| [`concurrent_same_id_assigns_fork_nothing`](../../../crates/ene-store/src/tests.rs#L280) | 同じ操作IDの並行割当が二重効果を作らないことを確認 |
| [`memory_revision_pages_and_summary_batches_are_bounded`](../../../crates/ene-store/src/tests.rs#L396) | 記憶改訂と要約読出しが上流境界で件数制限されることを確認 |
| [`recall_candidates_are_bounded_and_reach_old_relevant_rows`](../../../crates/ene-store/src/tests.rs#L484) | 想起候補を制限しつつ古い関連行を取りこぼさないことを確認 |
| [`startup_sweep_fails_closed_when_a_registered_value_is_unreadable`](../../../crates/ene-store/src/tests.rs#L567) | 秘密が読めない起動sweepを成功扱いしないことを確認 |
| [`rotation_between_scrub_and_provider_claim_refuses_and_a_rescrub_claims`](../../../crates/ene-store/src/tests.rs#L641) | scrub後の秘密集合更新で旧claimを拒否し再scrubで通すことを確認 |

## `crates/ene-store/src/tests/publication.rs`

| テスト | 保持理由 |
| --- | --- |
| [`activation_commits_the_reference_version_and_revision_together`](../../../crates/ene-store/src/tests/publication.rs#L42) | 資格情報の参照版と改訂番号が同時に確定することを確認 |
| [`a_rotation_retires_the_previous_version_for_cleanup`](../../../crates/ene-store/src/tests/publication.rs#L80) | 資格情報ローテーションが旧版を清掃対象へ移すことを確認 |
| [`a_stale_premise_abandons_the_candidate_and_stays_decided`](../../../crates/ene-store/src/tests/publication.rs#L128) | 古い前提で候補を放棄し後から復活しないことを確認 |
| [`mutation_ids_are_write_once_and_unknown_ids_are_refused`](../../../crates/ene-store/src/tests/publication.rs#L176) | 変更IDが一回だけ使われ未知IDを拒否することを確認 |

## `crates/ene-store/src/tests/report_reads.rs`

| テスト | 保持理由 |
| --- | --- |
| [`report_reads_are_read_only_over_a_running_and_stopped_store`](../../../crates/ene-store/src/tests/report_reads.rs#L113) | 稼働中・停止中の報告読出しに書込副作用がないことを確認 |
| [`report_rows_order_attempts_before_results_and_page_by_keyset`](../../../crates/ene-store/src/tests/report_reads.rs#L206) | 試行と結果の順序・keysetページングを確認 |
| [`report_source_pages_are_byte_bounded_on_utf8_boundaries`](../../../crates/ene-store/src/tests/report_reads.rs#L275) | 報告ページがUTF-8境界を守りバイト量で制限されることを確認 |

## `crates/ene-store/src/tests/task_result.rs`

| テスト | 保持理由 |
| --- | --- |
| [`fresh_task_starts_started_and_delegation_advances_to_in_progress`](../../../crates/ene-store/src/tests/task_result.rs#L136) | タスク開始と委任時の状態遷移をStore境界で確認 |
| [`completed_task_refuses_delegation_and_steering_without_writes`](../../../crates/ene-store/src/tests/task_result.rs#L178) | 完了済みタスクへの委任・操舵が書込なしで拒否されることを確認 |
| [`arrival_is_durable_before_adoption_and_reopen_preserves_the_body`](../../../crates/ene-store/src/tests/task_result.rs#L260) | 結果到着が採用前に永続化され再開後も本文が残ることを確認 |
| [`result_retry_is_idempotent_and_identity_reuse_fails_closed`](../../../crates/ene-store/src/tests/task_result.rs#L329) | 結果再送は一回扱いで別本文への同一ID流用を拒否することを確認 |
| [`confirmed_success_attempts_adopt_as_completion`](../../../crates/ene-store/src/tests/task_result.rs#L403) | 成功が確認された効果だけを完了として採用することを確認 |
| [`unknown_and_failure_attempts_withhold_completion`](../../../crates/ene-store/src/tests/task_result.rs#L441) | 結果不明と失敗を完了成功に変換しないことを確認 |

## `crates/ene-store/src/tests/usage_query.rs`

| テスト | 保持理由 |
| --- | --- |
| [`summary_attributes_dialogue_learning_and_task_agent`](../../../crates/ene-store/src/tests/usage_query.rs#L249) | 利用集計が対話・学習・Task Agentへ正しく帰属することを確認 |
| [`cap_status_breaks_down_consumption_and_reflects_admission`](../../../crates/ene-store/src/tests/usage_query.rs#L337) | 上限表示の内訳が実際の入場判断と一致することを確認 |

## `crates/ene-task/src/result.rs`

| テスト | 保持理由 |
| --- | --- |
| [`the_premise_binds_the_scrubbed_body_to_its_revision`](../../../crates/ene-task/src/result.rs#L297) | 結果採用前提が伏せ字本文と集合改訂を同時に拘束することを確認 |
| [`a_stale_outcome_names_only_the_revision`](../../../crates/ene-task/src/result.rs#L310) | 古い結果の通知で本文を漏らさず改訂だけ示すことを確認 |

