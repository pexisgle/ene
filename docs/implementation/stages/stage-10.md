# Stage 10: Voice と会話中心の Task 操作

[実装ガイド](../README.md) / [進捗](../PROGRESS.md) / [親 Issue #1686](https://github.com/pexisgle/ene/issues/1686)

ステータス: **実装計画。D0 の要件・設計変更は docs-only PR の merge で確定し、実装・実機検証の完了を意味しません。** この文書だけで製品要件や authority を変更しません。正本は [requirements](../../requirements/README.md)、[acceptance](../../requirements/acceptance.md)、[design](../../design/README.md) です。

## 1. 目的・範囲・開始条件

Companion との音声対話から、Task の依頼・状況確認・途中変更・中止・再開と、その結果の報告までを通す最小経路を実装します。ユーザーに Task 番号や作業一覧の選択・更新を要求しない体験を、#1686 の要件・設計変更として先に確定します。

実装範囲は [#1688](https://github.com/pexisgle/ene/issues/1688)（Voice）、[#1689](https://github.com/pexisgle/ene/issues/1689)（対象特定）、[#1691](https://github.com/pexisgle/ene/issues/1691)（Workspace/権限）、[#1690](https://github.com/pexisgle/ene/issues/1690)（報告）、[#1687](https://github.com/pexisgle/ene/issues/1687)（作業 GUI の削除）です。通常の会話を Task Agent の実行から独立して続けられることも対象です。

成果物の確認・プレビュー・保存操作の改善、Web/MCP/PC 操作、Schedule、Skill、Companion 間協調、remote Client はこの最小経路に含めません。後者は #1692〜#1696 と [roadmap](../README.md#4-最初の-milestone-後の-roadmap) で追跡します。既存の成果物確認機能を GUI 整理に巻き込みません。

Stage 10 の番号は維持し、Schedule より優先します。Stage 7 の外部アセット・指定 OS の検証待ちでも D0 と独立した開発を進めてよいですが、Stage 7 の完了判定 [#1706](https://github.com/pexisgle/ene/issues/1706) は残します。production slice は D0 の merge と、依存する公開境界が固定された `main` を前提にします。未マージの cleanup や GUI 改善を取り込んだつもりで計画を進めません。

## 2. D0: 要件と設計を先に確定する

D0 は Task 管理画面・進捗表示と画面報告を前提としていた記述を上位文書から改める **docs-only PR** です。要件・設計の merge より先に実装だけで意味を変えません。

| 確定すること | 更新・照合する正本 |
|---|---|
| 通常の Task 操作・報告に作業 GUI を要求しないこと。テキスト代替、アクセシビリティ、設定・認証・緊急操作との区別 | [製品要件](../../requirements/requirements.md) の会話/UI・Voice・Task、[受け入れ条件](../../requirements/acceptance.md) |
| 対象を特定する責任、曖昧な場合の確認、依頼の保留と再開、保存する事実と一時的な会話状態の境界 | [状態の所有権](../../design/architecture/state-ownership.md)、[実行フロー](../../design/architecture/runtime-flows.md)、[Companion / Task](../../design/subsystems/companion-task-learning.md) |
| 音声の生成・送信・再生・提示確認、未伝達の保持と再提示、mute・静粛時間・割り込み・再接続の扱い | [入出力・提示](../../design/subsystems/client-presence-io-observation.md)、[Client 遷移](../../design/critical-areas/client-presence-transition.md)、[IPC](../../design/concrete/host-client-ipc.md) |
| 依頼内容や Workspace 候補の確認と、第一者による正式な許可を分離すること | [権限・実行](../../design/subsystems/permission-credential-execution-inference.md)、[First-party desktop](../../design/concrete/first-party-desktop.md) |
| 必要な command / query / receipt の意味、currentness、保存・復旧、音声を含む一時データの消去 | [Interface Boundaries](../../design/concrete/interface-boundaries.md)、[並行制御](../../design/concrete/concurrency-control.md)、[永続化・復旧](../../design/concrete/persistence-recovery.md)、[Targeted Deletion](../../design/critical-areas/targeted-deletion.md) |

D0 の完了条件:

- 通常操作、音声障害時の同一会話でのテキスト代替、キーボード等による確実な停止・拒否を受け入れ条件に落とす。代替経路を新しい Task ダッシュボードとして設計しない。
- 発話を生成・送信しただけでは「伝達済み」にしない。receipt の対象事項の要約本文全体が会話タイムラインへ実際に表示されたか、音声再生が完了した場合に提示済みとする。音声だけの失敗・途中停止でもテキスト提示が成立すれば提示済み、両方が不明なら未伝達とする。提示を既読・理解・承認・Task 成功と混同しない。
- 音声での対象指定・通常の Task 指示と、新たなアクセス許可・高権限操作の最終確認を区別する。音声認識やモデルの出力を話者認証・第一者確認の証拠にしない。専用の安全な入力・確認が必要な操作は、その理由と続きの経路を会話で案内する。
- 生成/再生の中断、Task のキャンセル受付、実際の作用停止を区別する。結果不明の外部作用は自動再実行しない。
- 上位設計から具象設計まで意味を揃え、Stage 7 C2/E/F と acceptance の変更対応も明記する。既存の検証記録を音声 UX の証拠に書き換えたり、旧画面前提の gate を理由なく削ったりしない。
- Voice に関する既存要件を確認し、各要件を後続 slice / 検証へ割り当てる。音声 Task の最小経路だけを理由に、未対応の Voice 要件まで完了扱いにしない。

音声サービスやライブラリは、この境界を満たす最小経路を比較して選びます。先に新しい汎用 manager、Task 制御系、話者認証を作って設計の不足を埋めません。

| 既存 Voice 要件 | 後続 slice と検証 |
|---|---|
| 低遅延の割り込み会話、読み上げ停止と Task 中止の区別 | V1、R1、統合 F の S10-01 / S10-04 / S10-06 |
| マイク状態の常時表示とワンクリックミュート | V1、統合 F の S10-06 と実 Client での表示・操作確認 |
| キーボードによるミュート・音声停止・会話中断・拒否 | V1、P1、統合 F の S10-03 / S10-06 |
| マイク・音声サービス障害時のテキスト切替 | V1、R1、統合 F の S10-04 / S10-06 |
| 音声 Task 操作と音声・テキスト併用報告 | V1 / T1 / P1 / R1、統合 F の S10-01〜S10-08。U1 は代替経路成立後 |

## 3. PR 分割と gate

| Slice | Issue・実装範囲 | 前提と完了 gate |
|---|---|---|
| V1: 音声の入出力 | [#1688](https://github.com/pexisgle/ene/issues/1688)。実マイク入力から既存の会話/Task 制御へ接続し、Companion の音声応答を返す | D0 後。barge-in・再生停止・Task 中止を区別し、マイク/サービス障害で受付済み Task を失わない。Task 全操作の end-to-end は T1/P1 と統合して検証 |
| T1: 文脈からの対象特定 | [#1689](https://github.com/pexisgle/ene/issues/1689)。依頼内容・直前の会話・保存済み事実から候補を確かめる | D0 後。複数 Task、曖昧な指示、Host restart、先頭ページ外の対象を確認。検索は owner 側で bounded にし、対象特定と操作可能な revision の検証を分ける |
| P1: Workspace / 許可の対話 | [#1691](https://github.com/pexisgle/ene/issues/1691)。不足する対象・条件を尋ね、その依頼の続きとして扱う | D0 後。既存の許可を利用し、曖昧さ・拒否・取消・専用確認への移行を検証。音声だけで正式な許可を補わず、依頼の再入力や無断再実行を要求しない |
| R1: 状況・完了・確認待ちの報告 | [#1690](https://github.com/pexisgle/ene/issues/1690)。保存された Task の事実から自然な言葉で返答・通知する | V1/T1 後。D0 の提示確認と未伝達を接続し、cancel 受付≠停止完了、Unknown≠成功/失敗を維持。P1 の確認待ちも統合時に接続 |
| U1: 作業 GUI の削除 | [#1687](https://github.com/pexisgle/ene/issues/1687)。番号付き一覧、対象選択、更新、進捗/報告の専用画面を除く | V1/T1/P1/R1 が成立した後。通常の依頼から終了まで Task 番号や作業画面が不要。内部の Task・結果・未伝達記録、成果物確認、設定・認証・緊急操作を維持 |
| F: 統合受け入れ | D0 で更新した acceptance を実 Client / Host / 音声で確認 | 全 slice の統合 tip。下記 §5 の正常・失敗経路と、D0 の要件対応を満たす |

V1/T1/P1 は共通 interface / schema を先に固定した範囲で並列化できます。R1 は V1/T1 の後に進め、P1 と独立な部分を待たせません。同じ会話状態や receipt schema を別の PR で二重に所有しません。

```text
D0 → 共通境界 → V1 ─┐
              T1 ─┴→ R1 ─┐
              P1 ────────┴→ U1 → F
WSS / 入力境界の確定 ─────────→ 音声 Client 統合・F
```

## 4. 他の作業との接点

[Stage 7 A2](stage-7.md#a2-通常-client-通信の-wss-統一) と [基盤計画 F1](../follow-ups/foundation.md) の round 指定を音声入力の接続前に調整します。Host / Client 間の音声統合は WSS の公開境界を使い、旧 transport 用と新 transport 用の二つの production 経路を作りません。D0、対象検索、音声デバイスの技術成立 probe は独立して先行できます。

[基盤計画](../follow-ups/foundation.md) の未知 payload と表示言語・起動パスは、各境界の変更時に調整します。primitive 整理など無関係な follow-up の全件完了を、この Stage の一律な開始条件にはしません。

Body には必要な表示指示だけを渡します。音声処理を理由に Body を Task / credential の所有者にせず、描画障害と会話・Task の継続を分離したままにします。Host-local の専用確認 channel は通常の音声 Client 通信に統合しません。

## 5. 統合受け入れと完了判定

D0 で確定した acceptance に対応させ、少なくとも次の経路を検証します。具体的な型や新しい性能閾値をこの文書で追加しません。

| 経路 | 必要な確認 |
|---|---|
| 音声による通常操作 | 実マイク → 音声認識 → 依頼 → Task 実行 → 音声応答を通し、作成・状況確認・変更・中止・再開を行う。Task 実行中の雑談も継続できる |
| 割り込みと停止 | 「説明はもういい」で再生だけを止め、Task を勝手にキャンセルしない。明示した Task 中止では受付と停止結果を区別する。キーボード等の代替操作も利用できる |
| 対象と権限 | 複数 Task、曖昧な呼称、古い revision、Workspace 不足、許可拒否/取消を扱い、別 Task や未許可の対象へ作用しない |
| 報告の提示 | mute・静粛時間・barge-in・再生失敗では、テキストの全文提示または音声再生完了の事実に従う。両方が不明、または切断中なら未伝達を保持する。復帰後は現在の状況へまとめ直し、変化のない進捗を話し続けない |
| 切断・復旧 | Host / Client restart と音声サービス障害を扱う。古い入力・receipt・操作を新しい接続へ replay せず、結果不明の作用を無断で再実行しない |
| データ・描画の障害 | 音声に追加された本文コピーや提示待ちデータも D0 の消去契約に従う。Body の停止でテキスト代替・安全操作・Host-only Task が失われない |
| GUI 整理 | Task 番号の表示/読み上げや一覧選択を要求しない。成果物確認・管理・専用確認・緊急操作を利用でき、代わりの Task ダッシュボードを作っていない |

failure path は fake provider や入力注入でも決定的に検証しますが、それだけで実マイク・音声認識・再生を含む受け入れの代用にはしません。Windows / Linux の対象経路を、OS・音声デバイス・provider/model・言語・exact SHA とともに記録し、実施済み・失敗・未実施を区別します。性能は該当する既存 gate と D0 で確定した受け入れ条件に従います。

V1〜U1 と F、および D0 で割り当てた Voice 要件を満たしてから Stage 10 を完了にします。個別 Issue はその完了条件と merge / 検証の証拠で close し、計画の追加だけでは close しません。親 #1686 は #1692〜#1696 を含む子 Issue の追跡として残し、Stage 7 の残 acceptance も別途維持します。
