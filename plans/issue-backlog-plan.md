# 未対応issue 18件 実装計画（検証前）

作成日: 2026-08-29
対象: open issue 27件中 PR/branchなし 18件 → 全件PR作成でgoal
検証: 計画作成後に内容をレビューし、抜け・重複・境界違反を潰してから実装

## 前提
- open 27件中 9件はPR作成済み（1213,1214,1218,1219,1223,1224,1225,1227,1228）→ 本計画の対象外
- 残り18件: #717 #1187 #1198 #1199 #1200 #1201 #1202 #1203 #1204 #1205 #1206 #1207 #1208 #1209 #1210 #1181 #1179 #1177
- plans/product-convergence/* が参照されているが実体は plans/harness-redesign/* のみ存在 → 不整合を吸収する
- workspace lintsは厳格（clippy deny系）→ 新コードは必ず cargo check/clippy を通す

## 分類と並列トラック

### Track A: Desktop v1.0 bug（即時並列）
| issue | タイトル | 方針 |
| #1177 | Readiness矛盾 | Home/Conversation/Voiceのreadinessを「保存されたか」ではなく「実利用可否+probe」で判定。active badge/disabled化、Alicia-B誤メッセージ修正、Micガード（STT未設定はONにせずCTA）、再起動後の整合を保証するテスト追加。既存 #1211 の残差を埋める |
| #1179 | FileDialogが常に背面 | rfd FileDialogに正しいowner HWNDを渡し、winit WindowLevel AlwaysOnTopより前面に出す。親無効化→focus復帰、session exportは日時+companion名を含むファイル名提案 |
| #1181 | responsive/contrast/raw値露出 | chrome.rs minimum_inner_size検証、Detail/Homeのscroll確保、provider model pickerのvirtualized化、StatusTone contrast修正、HomeカードCTA優先表示、raw値(Usage/Diagnostics/UUID/schema/path)をAdvanced/Copy diagnosticsへ隔離、enum→select、MCP description折りたたみ |

### Track B: 製品決定・要件・方針（docs中心、並列）
| issue | 方針 |
| #1200 | plans/harness-redesign/product/vision.md, decisions.md, features.md, done.md を同期。PC-D1〜D6をD番号付きでdecisionsへ反映、初期OS/model/音声/複数体/Webのv1境界を10前後の体験で言語化 |
| #1208 | P-1xx〜P-10xxを V1-Core/V1-Safety/Presence/Learning/Later/Form-only へ再分類。新規 Task Contract/Attention/Grant/Computer Action/Learning CandidateにP番号付与、features.md/done.md同期、Later→v1復帰禁止規則明記 |
| #1210 | Epic: 01 vertical sliceを正とし、子issueの推奨順を固定。代表体験のE2E gateを明文化、既存bugは参照のみでcloseしない方針を維持 |
| #1209 | crate boundary/typed state/REST投影/WS差分/runner scope/cancel/verification/stale-safe/Grant/Attention配信/copy/accessibility/privacy-safe spans/DoD をチェックリスト化し、各子issueが参照するポリシー文書を整備 |

### Track C: harness v1.0 完成定義（検証・テスト中心）
| issue | 方針 |
| #717 | done.md v1.0チェックの「機構 vs 実プロセス」区別を徹底。各未チェックに対応する実装issue or 対象外理由+後継milestoneを対応付け、手動GUIは環境/手順/観測記録を残す。fmt/clippy/test/docのgateを明記 |
| #1187 | #717のblocking項目（offline GGUF/バージイン/自声回避/2体同時/VRM lip-sync/job cancel+追加指示+質問/compact LLM/委譲報告/mcp/skill/fs+exec/タスク発話/監査AI理由/外部通信ゼロ）を個別受入テスト or 手動手順で観測可能にする |

### Track D: Task/Attention/Computer（runtime中心、並列）
| issue | 方針 |
| #1198 | ene-work/ene-session/ene-access-controlに TaskContract/TaskState/verifying/evaluator/artifact registry/workspace confinement/mailbox revision/follow-up/question/answer/cancel/Interrupted/API /tasks 移行を実装。 incomplete拒否/model done≠Completed/scope拡大再承認/workspace外拒否/cancel後副作用停止/restart消失防止 |
| #1199 | Attention Item/Store/state/priority/action_required/dedupe/expiry/task adapter/quiet hours/speaking gate/surface report turn/speech-card-notification-digest/Task Center API。raw完了文を直接出さない、action-required埋没防止、集約、stage未接続でも喪失なし、追跡可能、割込率計測 |
| #1203 | WindowIdentity/ObservationID/UIA backend/screenshot+element tree統合/stale-safe click-type-key-scroll/postcondition/semantic risk/Task Grant/hard confirmation/prompt injection対策。古い要素再利用禁止/focus逸脱停止/結果不明≠成功/scope内連続操作/外部送信の無確認禁止/監査追跡 |
| #1201 | VS-01〜07のvertical sliceを実モデル+実tool+実stageで成立（Markdown生成/並行会話/follow-up等/Attention報告/Task Center/初回設定/実provider受入） |

### Track E: Companion/自己進化/移行/検証/Stage IA（横断）
| issue | 方針 |
| #1204 | body presence( idle/look-at )→TTS/lipsync→memory(scope/provenance)→affect→proactive(Attention尊重)→STT/barge-in/自声回避→decay/reflection の順で接続。矛盾なし/表層発話のみ読む/provenance追跡/quiet尊重/実機観測/textでも状態保持 |
| #1205 | ene-workにLearningCandidate store/state/correction検出/draft生成/static validator/replay/承認UI/versioned activation/canary/rollback。1回成功で昇格させない/未評価で実行しない/permission差分表示/plane迂回禁止/logにversion/旧版へrollback |
| #1206 | Phase1 E2E/PC操作/UIA/metrics baseline/done対応/Echoだけで閉じない、を実provider/実Windows app/UIA観測で証明する検証体系 |
| #1207 | 文書/契約→新型→DB migration→runner→API/SDK→stage→computer→presence→self-evolution→旧path削除の直列移行。旧job並存禁止/用語統一/backup復元/Interrupted報告/targetなし移行禁止/flag削除 |
| #1202 | Conversation/Tasks&Attention/Companion中心IA、active soul一貫表示、setup wizard、scoped approval、Settings/Diagnostics分離、raw ID/pathのAdvanced移動、keyboard/UIA、EN/JA audit |

## 実装順と依存
1. Track A/B/C は相互依存なし → 最初に並列で着手（高速にPRまで）
2. Track D/E はBの用語・Cの受入基準に依存するが、実装は先行してdraft PRを出し、B/Cのdocs確定時に追従
3. 各PRは Closes #NNN を本文に含め、1 issue = 1 branch/PR を原則とする（複数closeの誤爆を避ける）

## 検証観点（計画検証で潰す）
- [ ] 18件すべてにPR方針が割り当てられているか
- [ ] Architecture boundaries違反がないか（session所有、kernel非依存、companion非依存、workゲート、plane承認等）
- [ ] lints/spec違反（unwrap/expect/panic/print、SAFETYコメント、workspace deps）を起こさないか
- [ ] docs/ja 同期が必要な箇所を漏らしていないか
- [ ] 既存mainのfix（#1211/#1216等）との重複・競合を吸収できているか
- [ ] テストが「名前の存在」ではなく「実観測」で受入できるか

