# Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 7 の**実装範囲、PR 分割、依存関係、検証 gate**を管理します。製品 behavior は [requirements](../../requirements/README.md)、受け入れ範囲は [acceptance](../../requirements/acceptance.md)、authority と状態の意味は [design](../../design/README.md) を優先します。計画の存在は着手・完了を意味しません。

## 1. 目的と開始条件

Stage 0〜6 の機能を、初回セットアップから日常の会話・管理・障害復旧まで使える first-party GUI に結び、Milestone 1 を完了します。CLI の一覧を画面へ貼り付けるだけでなく、実際の GUI 入力・提示・確認を通して既存の安全性を維持することが目的です。

計画・調査は Stage 6 の最終修正と並行してよいですが、Stage 7 の production slice は Stage 6 の完了 gate を満たした統合 base に積みます。Stage 6 に属する不具合を GUI 側の回避処理で隠さず、先に元の owner で修正します。Stage 6 の完了記録は Stage 6 closeout が担当します。

### 対象

- 初回セットアップ、同梱キャラクター `ene`、一対一のテキスト会話、日本語/英語の切り替え。
- Memory と由来・変更履歴、Task と進捗・結果・明示的な再開、usage / cost / cap、Targeted Deletion と復旧操作の text-first 管理画面。
- VRM 1.0 の透明 desktop avatar、移動・resize・一時非表示、待機/応答中の表情・仕草、描画故障から独立したテキスト操作。
- acceptance の全 Milestone 1 scenario と性能基準。対象は Windows 11 x86-64 と NixOS 26.11 x86-64 / KDE Wayland、日本語と英語。

### 非対象

Stage 8 以降の schedule、backup / restore 本体、Host の OS 自動起動、Voice、Observation、group conversation、Global Memory、skill 自動生成、長期 emotion / relationship、remote Client、character editor / package distribution、multi-provider / automatic fallback は追加しません。同梱 `ene` の静的定義・表示アセットを使う最小経路は必要ですが、汎用の配布・編集基盤を先取りしません。

## 2. 引き継ぐ境界と実装方針

Stage 6 統合 base で次の接続点を再確認してから変更します。型の正確な形は source / rustdoc を読み、Stage 文書にコピーして固定しません。

| 接続点 | Stage 7 で行うこと | 境界の参照先 |
|---|---|---|
| `apps/ene-ctl/src/{client,device,incarnation}` の接続・認証・相関処理 | CLI と GUI で必要な部分だけ共有化し、既存の lifecycle 回帰を維持する | [IPC](../../design/concrete/host-client-ipc.md)、[crate 分割](../../design/concrete/crate-module-decomposition.md) |
| `ene-api` の会話、History、management、Task query / resume | GUI から同じ owner へ接続する。構造化項目が不足するときだけ owner query と DTO を追加する | IPC §18、[interface boundaries](../../design/concrete/interface-boundaries.md) |
| usage query と cap command | Reported / Unknown / Reserved、価格 snapshot、cap revision をそのまま表示・比較する | [usage-cost-cap](../../design/concrete/usage-cost-cap.md) §16–§17 |
| serving Host の削除確認、status、driver、Client erasure | GUI に確認・状態確認・再開と local erasure を接続する | [deletion lifecycle](../../design/concrete/targeted-deletion-lifecycle.md) §8–§15、IPC §17–§18 |
| Host-local の setup / credential / pairing | CLI の秘密値手渡しを GUI の完成経路と見なさず、保護された初期設定経路を具体化する | IPC §18.1、interface boundaries K-C |
| `apps/ene-stage`、`ene-stage-ui`、`ene-vrm` の配置案 | 実際に使う GUI と描画 adapter だけを追加する。空 crate 一式を作らない | crate 分割 §3、[runtime topology](../../design/architecture/runtime-topology.md) |

UI は Host の DB や domain crate を直接読まず、CLI の表示文や `ViewSection.body` を解析して制御状態を推測しません。Host 発行の view / revision / cursor は表示と照合の前提であり、Client の authority ではありません。GUI 専用 DB、長期的な会話・Memory cache、第二の permission / presence / accounting registry は作りません。

## 3. PR 分割と各 slice の gate

各 ID は実装作業の参照名であり、新しい要件 ID ではありません。1 slice が大きくなる場合も、producer → consumer → production-path test をレビュー可能な PR に分け、未配線のまま slice 完了にしません。

### A0: GUI 着手前の契約具体化と実機 probe

**成果物**: 下表の判断を対応する design に反映する docs-only PR と、両 OS の小さな実行可能 probe / 検証記録。調査結果だけで製品機能の実装済み扱いにはしません。

| 先に決めること | 判断・検証内容 | 反映先 |
|---|---|---|
| trusted Host-local setup / management | 未ペアリングから起動できる入口、Host が管理する第一者画面の識別、確認対象・前提への束縛、秘密入力から credential owner への経路を具体化する。同一 UID / SameMachine / paired / 自己申告の `trusted` flag だけで最終確認を許可しない。認証秘密の平文は保護された Host-local 設定経路内だけで扱い、通信電文の payload に載せない | IPC §18.1、interface boundaries K-C、必要な persistence 境界 |
| UI と avatar の障害分離 | 推奨する実装案は、text / management 側とは別の optional renderer process。同じ実行ファイルの別 mode でもよい。起動・停止・異常終了、bounded な表示指示、戻りの故障通知、管理画面が GPU 初期化を待たない経路を具体化する。単なる別 thread や `catch_unwind` を process crash 隔離の証明にしない | runtime topology、[入出力・提示](../../design/subsystems/client-presence-io-observation.md)、crate 分割 |
| UI / renderer backend | 既存の Slint UI / VRM adapter 案を出発点に、採用 version と backend を決める。日本語 IME、透過、drag / resize、非表示からの再表示、入力領域、focus を Windows / KDE Wayland で試す。native Wayland / XWayland のどちらを使ったか明記し、別 session の成功を KDE Wayland の証拠にしない | crate 分割の adapter 選定箇所と probe PR |
| 同梱 `ene` と VRM | 静的定義、VRM 1.0 アセット、利用・同梱条件、必要な material / expression / animation 機能を確認する。不足は具体的な取得・作成作業として Issue 化し、一般モデルや placeholder の表示で `ene` 完了としない | [character distribution](../../design/subsystems/character-distribution.md) の静的供給境界と asset の所在 |
| 測定と受け入れの実行環境 | §5–§6 のテスト割り当て、実機、CPU 百分率の分母、メモリ集計、計測区間を固定する。必要な依存・アセット・配布物の license とビルド再現性も確認する | test / probe PR、必要な設計箇所 |

特に一般的な window API の呼出しが OS ごとに同じ効果を持つとは仮定しません。例えば [winit 0.30.13 の `set_visible`](https://docs.rs/winit/0.30.13/winit/window/struct.Window.html#method.set_visible) は Wayland を unsupported としています。これは winit 採用の指定ではなく、画面機能を adapter と実機で確認する理由です。

**gate**: setup の trust / secret 経路と UI の障害境界が design 上で説明でき、各採用 backend の probe 結果があること。未決の項目は依存する slice を止めますが、独立した Client 共通化やテスト準備まで止めません。契約の矛盾はこの段階で解消し、実装者のローカル判断へ持ち越しません。

### A1: 最小の共通 Client 接続基盤

**範囲**: `ene-ctl` の transport / framing / authentication / incarnation / request correlation / erasure 応答のうち GUI と共有する実装を、小さな Client 側 library へ抽出します。配置は A0 で crate 分割へ反映し、CLI 引数処理や画面 rendering、Host の意味判断を混ぜません。

- GUI の event loop を接続待ち・provider 待ちで塞がず、応答と unsolicited fact / deletion demand を継続して受け取れるようにする。
- connection / incarnation / presence を混同せず、古い connection の入力・receipt・cursor・選択状態を継承しない。再接続で未送信本文や変更 command を自動 replay しない。
- bounded queue と cancellation を扱い、同じ有効区間での明示 retry だけが元の command identity を再利用する。読み取りと mutation の retry を一律に扱わない。

**gate**: 実 socket / named pipe で CLI 回帰が通り、GUI 用 adapter でも接続置換、応答相関、遅い受信側、待機中の erasure demand を検証できること。新規の汎用 session manager や第二の currentness owner は不要です。

### B: 初回セットアップからテキスト会話までの縦断 GUI

**範囲**: `apps/ene-stage` と必要な UI adapter を追加し、avatar なしで操作できる最初の製品経路を通します。A0 の保護された入口と A1 を使用します。

1. 新規 data directory から、言語・同梱 `ene`・送信データと費用の説明・credential 登録・model 割り当てまでを案内する。Host の明示起動とローカル接続の準備も扱い、手動の環境変数設定や DB 編集を通常の完了手順にしない。
2. credential 登録と provider 利用への同意・割り当てを分ける。登録だけでは provider 呼出しを行わず、初期設定の失敗や再起動で暗黙に同意を補わない。
3. 一対一 timeline、入力、stream、履歴 page、接続・presence の表示を実装する。管理画面は Companion 停止中・provider 不通・avatar 不在でも開けるようにする。
4. 日本語 IME の未確定入力を誤送信せず、日英のラベル・拒否理由を切り替えられるようにする。言語切り替えで会話本文や domain state を書き換えない。

**gate**: acceptance §1 を新規環境から GUI で通し、キー登録直後の provider 呼出しゼロ、割り当て後の会話、Host 再起動後の履歴、日英切り替えを確認する。キーを通常本文・ログ・エラー・保存した GUI state に残さない。必要な Host API の追加は owner で行います。

### C1: Memory と由来の管理画面

**範囲**: 現在の記憶、scope、importance、作成日時、Experience Summary の根拠、過去 revision を確認できるようにします。構造化された bounded owner query が足りない箇所は、query → wire DTO → GUI の順に最小追加します。

**gate**: acceptance §3.1–§3.10 を会話と管理画面で確認すること。訂正・状況変化・重複統合・通常の忘却は既存 Learning / Companion 経路で行い、GUI に独自の形成判断や物理削除 shortcut を作りません。ページを跨ぐ履歴・根拠、不在/消去済み source、read-only の無副作用を検証し、新しい本文表示にも Stage 6 の withholding と delivery evidence を接続します。

### C2: Task と Workspace の管理・復旧画面

**範囲**: Task 一覧、revision、実行登録の有無、Action の確定度、結果と採用状態、変更ファイル・保存先・残作業を表示し、対象選択、追加指示、cancel、明示 resume を既存 command に接続します。Task の作成はパートナーからの既存委任経路を通します。

**gate**: acceptance §4 と §5 の GUI 操作対象を通すこと。Unknown / 中断 / Failed / Cancelled / Completed、cancel の受付と作用の停止完了を区別します。resume は表示した Task revision / purpose に束縛し、stale を最新前提へ自動置換しません。Client を閉じても Host-only Task が継続し、再接続で結果が届き、Host 再起動後は旧 delegation を launch せず明示指示を待つことを、provider 呼出し・ファイル変更回数を数えて検証します。

### C3: usage / cap・削除・安全な復旧の管理画面

**範囲**: token と費用の内訳、provider / model / consumer / purpose、期間・ページ、cap と消費内訳を既存 query で表示します。cap 更新は既存 revision / intent 境界を通し、UI で独自集計した残額を利用許可にしません。

Targeted Deletion は通常の忘却と分け、request → Host-local 最終確認 → operation / participant status → 必要時の明示 resume を接続します。Owner の resume inlet が不足していれば preservation owner の既存 lifecycle command へ最小接続し、画面から sweep や completion を直接書き換えません。再確認・再開の前提は A0 で具体化します。

**gate**: acceptance §3.11–§3.12、§6、§7 を対象に、Unknown をゼロ料金と表示しないこと、古い cap view の拒否、削除の Held / Finalizing / Completed の区別、到達不能 Client が完了の証拠にならないことを確認する。復旧案内は日本語/英語で、秘密値・削除検索材料・raw provider error を含めません。外部作用が Unknown の Task を「再試行」ボタンで無条件に動かさず、認証の修正後も過去の処理を自動 replay しません。

### D: VRM desktop avatar と text fallback

**範囲**: A0 で確定した静的アセット供給と障害境界を使い、同梱 `ene` を表示します。まず透明表示・移動・resize・非表示/再表示を通し、次に既存の応答状態に従う待機/発話中の表情・仕草を接続します。新しい感情推論や Voice pipeline は作りません。

renderer に渡す情報は必要なアセット参照と表示指示に限定し、会話本文、Memory、API key、management authority を渡しません。avatar の非表示・終了を Companion 停止や Task cancel と同一視せず、renderer 再起動で古い会話/操作を replay しません。

**gate**: acceptance §2 を両実 desktop で通すこと。renderer の異常終了・hang・初期化失敗を注入しても、text 入力、Task cancel、設定/復旧が利用できることを確認します。アバターの矩形や透明部分がデスクトップ入力を不必要に奪わず、操作に応じて移動・resize できることを実測します。CLI が生存しているだけでは GUI fallback の合格にしません。

### E: GUI の提示・一時データ・障害経路の横断検証

**範囲**: 新しい GUI を Stage 5 / 6 の presentation・erasure・secret safety の参加者として仕上げます。安全な受信・破棄は B / C の各 PR に含め、この slice まで未実装のまま後回しにはしません。

- timeline、Memory の根拠/履歴、Task report、検索/入力 draft、IME composition、undo、deferred frame、描画用 text model など、GUI が実際に所有する本文コピーを inventory する。機能を持たない分類の架空コピーは作らない。
- deletion demand では該当コピーと旧 receipt / view を無効化・消去し、実際の消去確認後にだけ `wiped` を返す。widget が本文を保持しているのに参照だけ切って Verified にしない。確認不能な範囲は unverified とし、OS や他アプリ所有のデータまで消去保証を広げない。
- 本文を含む画面から erasure / disconnect / connection replacement を競合させ、古い UI callback、page response、入力 draft が再提示・再送・再保存されないことを確認する。
- presentation ACK は、その receipt の項目を実際に提示した経路から返す。単なる受信・モデルへの格納・一覧 query の成功を Presented にしない。GUI crash / ACK loss 後の重複表示と外部作用の再実行を区別する。

**gate**: 実 GUI adapter と serving Host を使い、Stage 6 完了後の遅延結果、Host restart 越しの Client delivery evidence、登録秘密の各画面/エラーへの非露出を回帰すること。消去前に本文が実在したことを確認する positive control と、相関・副作用回数の検査を持ち、空の画面だけで成功するテストにしません。

### F: Milestone 1 の実機 acceptance / performance と closeout

**範囲**: §5 の全行を統合 tip の GUI から確認し、§6 の生データと結果を残します。各 slice の unit / integration test は維持し、最終 E2E だけに検証を集約しません。

**gate**: 両 OS・両言語の対象 scenario と性能基準を満たすこと。未実施、失敗、暫定回避は成功と区別します。acceptance に影響する未解決事項は blocker とし、記録しただけで milestone を完了にしません。完了後にだけ `PROGRESS.md` を Stage 8 へ進めます。

## 4. 依存順と並列化

```text
A0 の共通境界確定 → A1 → B → C1 ─┐
                           ├→ C2 ─┼→ E → F
                           └→ C3 ─┤
A0 の描画/asset gate ────────→ D ──┘
```

A1 の純粋な抽出・既存テストは A0 と並行可能です。D の renderer 単体は A0 の描画 gate 後に進められますが、製品への応答状態接続は B の経路確定を待ちます。E / F の harness と計測準備は最初から並行し、完成済み slice から順に検証を追加します。

C1 / C2 / C3 は異なる owner と画面に分け、共通 Client API・DTO・schema が確定した範囲だけ並列化します。同じ transport loop、Host-local trust boundary、schema version、共有 view を別 branch で同時に再設計しません。共通 prerequisite は先に取り込み、stack の base / head と担当範囲は PR 本文で管理します。

## 5. acceptance と証拠の対応

| acceptance | 主担当 | Stage 7 で追加する first-party 証拠 |
|---|---|---|
| §1: セットアップと最初の会話 | B | 新規環境から GUI だけで同意・登録・割り当て・会話。登録のみでは送信ゼロ。日英切り替えと再起動後の履歴 |
| §2: Desktop Body | A0 / D | 実 desktop の同梱 `ene`、透過・移動・resize・非表示・待機/応答、renderer 故障中の text / management |
| §3.1–§3.10: Memory | C1 | 会話由来の形成・訂正・状況変化・想起・統合・通常忘却と、scope / importance / 根拠 / revision の GUI 確認 |
| §3.11–§3.12: Targeted Deletion | C3 / E | GUI での要求・信頼された確認・状態/復旧、GUI 一時コピーの消去、並行形成/遅延結果からの非復活 |
| §4: Workspace Task | C2 | GUI からの会話→委任→並行会話/追加指示/cancel→結果。許可外アクセス拒否、既存ファイルと成果物の保持 |
| §5: S5-01〜S5-24 | A1 / C2 / E | 既存の全 Host / transport 回帰に加え、GUI close / reconnect / presentation ACK / manual resume / 50件超の表示を実経路で確認。低レベル race を全て座標クリックで再実装する必要はない |
| §6: 障害とデータ保護 | B / C2 / C3 / D / E | 認証失敗、provider 不通、renderer/Agent crash、cancel、保存中終了を注入し、状態読込と安全な明示復旧、無断再実行ゼロ |
| §7.1–§7.2: 費用と機密 | C3 / E | input / output / cached token と費用内訳の表示、Unknown の区別、登録秘密の自動検査と GUI 目視 |
| Performance Gates | A0 / D / F | 両 OS の process 全体計測、実描画 FPS と操作受付反映の時刻記録 |

protocol / currentness / failure は、実 Host・store・Client transport と barrier 制御した fake provider で自動化します。テスト用の trusted-input seam は production から利用できない形にし、GUI 以外の入力が最終確認を偽装できない negative test を持ちます。DB fixture は race の準備に使えても、確認・採用・完了を fixture 書込みで代替しません。

実 desktop では GUI 入力・実描画・IME・focus・ローカル最終確認を検証します。さらに明示同意した OpenAI Responses API で会話・Memory・ファイル Task の smoke を行い、provider / model と結果を記録します。fake provider の成功だけで実 provider の意味的品質を証明したことにはしません。記録にはテスト用データを使い、API key・個人情報・削除対象の実データを画像や CI artifact に残しません。

## 6. 性能測定

閾値の正本は [acceptance の Performance Gates](../../requirements/acceptance.md#性能基準-performance-gates) です。下表は測定方法を具体化し、基準を引き下げません。

| 項目 | gate と記録 |
|---|---|
| idle CPU | セットアップ後・推論なしの連続5分間で、全 ene process の平均 CPU 使用率が10%以下。Host、text GUI、renderer、起動した補助 process を計上し、process CPU time の生値と使用率の分母を記録する |
| resident memory | 同じ待機区間で Host と全 Client process の合計が2 GiB以下。OS ごとの resident 指標、集計方法、時系列と最大値を記録し、renderer を除外しない |
| avatar | 同梱 `ene` の通常表示で平均30 FPS以上。他のデスクトップ操作を1秒以上 block しない。実際の frame 提示時刻と操作遅延を測り、要求した redraw 回数を FPS と数えない |
| 操作受付 | Milestone 1 に含まれる cancel 等は入力から1秒以内に受付状態を表示する。ローカルの「送信待ち」を Host 受付済みと偽らず、受付と完了を区別する |

A0 で各 OS の測定指標・CPU の正規化方法をレビューして固定し、比較不能な数値を同じ百分率として扱いません。release build の exact SHA、build flags、OS / nixpkgs revision、KDE/session、CPU/GPU/driver、解像度・scale、UI言語、asset と描画 backend、warm-up と測定区間を結果に添えます。NixOS の対象版が未確定の時点の probe は暫定値とし、最終検証環境を無断で置き換えません。

通常 avatar 表示中の idle を基準にし、非表示・renderer 停止時は補助測定として分けます。Voice 等で今回提供しない操作例は対象外の理由を明記し、未実装の必須機能を skip して合格にはしません。headless CI の成功は実 GPU / compositor / IME / desktop 性能の合格と別です。

## 7. docs と closeout の運用

| 場所 | 更新する内容 |
|---|---|
| この文書 | Stage 7 の範囲、slice、依存、検証対応。日々の実装ログや全 PR の列挙は加えない |
| `docs/design/` の対応箇所 | A0 と後続で必要になる恒久的な trust / IPC / ownership / failure 契約。実装 PR より先に docs-only PR で確定する |
| `docs/requirements/acceptance.md` | 原則変更しない。実装都合で scenario・OS・性能 gate を削らず、要件変更が必要なら別の明示的な判断にする |
| Issues / PRs、実行可能 test / harness | 個別不足、実装範囲、exact tip、再現手順、テスト対応、実機の測定結果と証拠。結果は pass / fail / 未実施を区別する |
| `PROGRESS.md` | current / completed / blocker / next の短い index のみ。Stage 6 完了前に Stage 7 を current / completed とせず、Stage 7 内部の全 slice を転載しない |

最初の実装作業は A0 の保護された setup と描画/asset probe、および A1 の既存 Client 抽出です。大型の管理画面や VRM renderer を先に作り込み、最後に trust boundary と対象 OS への適合を調整する順序にはしません。

Stage 7 closeout では共通 quality gate に加え、§5 の全対象と§6 の実測、GUI の local erasure、text fallback、後続 Stage が利用する query / command の配線を確認します。Stage 6 を含む既存回帰を維持し、未解決の acceptance blocker がないことを確認してから、Stage 7 を completed index に1行追加し、Stage 8 の詳細計画へ進みます。
