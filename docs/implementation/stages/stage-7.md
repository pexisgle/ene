# Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 7 の**実装範囲、PR 分割、依存関係、検証 gate**を管理します。製品 behavior は [requirements](../../requirements/README.md)、受け入れ範囲は [acceptance](../../requirements/acceptance.md)、authority と状態の意味は [design](../../design/README.md) を優先します。計画の存在は着手・完了を意味しません。

first-party desktop の process / trust / 依存 / 性能の分母は [First-party desktop](../../design/concrete/first-party-desktop.md) が所有します。本文書は slice 順と gate だけを持ち、同じ契約本文をコピーしません。

## 1. 目的と開始条件

Stage 0〜6 の機能を、初回セットアップから日常の会話・管理・障害復旧まで使える first-party GUI に結び、Milestone 1 を完了します。CLI の一覧を画面へ貼り付けるだけでなく、実際の GUI 入力・提示・確認を通して既存の安全性を維持することが目的です。

計画・調査は Stage 6 の最終修正と並行してよいですが、Stage 7 の production slice は Stage 6 の完了 gate を満たした統合 base に積みます。Stage 6 に属する不具合を GUI 側の回避処理で隠さず、先に元の owner で修正します。Stage 6 の完了記録と現在のステージは [PROGRESS.md](../PROGRESS.md) を参照します。

### 対象

- 初回セットアップ、同梱キャラクター `ene`、一対一のテキスト会話、日本語/英語の切り替え。
- Memory と由来・変更履歴、Task と進捗・結果・明示的な再開、usage / cost / cap、Targeted Deletion と復旧操作の text-first 管理画面。
- VRM 1.0 の透明 desktop avatar、移動・resize・一時非表示、待機/応答中の表情・仕草、描画故障から独立したテキスト操作。
- acceptance の全 Milestone 1 scenario と性能基準。対象は Windows 11 x86-64 と NixOS 26.11 x86-64 / KDE Wayland、日本語と英語。

### 非対象

Stage 8 以降の schedule、backup / restore 本体、Host の OS 自動起動、Voice、Observation、group conversation、Global Memory、skill 自動生成、長期 emotion / relationship、remote Client、character editor / package distribution、multi-provider / automatic fallback は追加しません。同梱 `ene` の静的定義・表示アセットを使う最小経路は必要ですが、汎用の配布・編集基盤を先取りしません。

## 2. 固定する契約

Stage 7 は Stage 0〜6 の domain authority と安全性契約を再設計しません。

- Host が durable domain state と最終 authority を持ち、Client の表示状態を master にしない。
- connection / incarnation / presence / presentation を同一視しない。
- credential raw value を通常の Host↔Client payload、DB、ログ、エラーへ流さない。秘密寿命を通常 DTO・Debug・Targeted Deletion に依存させない。
- high-privilege の最終確認は Host が起動した GUI に限定継承した専用 channel と、ユーザーの直接確認を必要とする。通常 Client / requester / CLI / LLM / tool / plugin / Computer Use は確認者にならず、空席でも任意の同一 UID 接続者へ seat を与えない。
- UI / renderer の障害で Host-only Task や text / management の利用を壊さない。
- usage / cost / cap、Targeted Deletion、Task recovery 等は既存 semantic owner の query / command / currentness を使う。
- GUI 専用の authoritative DB、permission registry、presence registry、accounting registry を作らない。
- acceptance と performance gate を実装都合で弱めない。

## 3. A0 結論（design。production Rust はまだ書かない）

A0 の成果物は [First-party desktop](../../design/concrete/first-party-desktop.md) と、それを指す CM / IPC / IB の更新です。旧実装の `ene-stage` は移植しない。未検証 crate を恒久 contract として固定しない。

### 3.1 Process / 名前（確定）

- **Host**: `apps/ene-core`。GUI / wgpu を持たず、要求専用 listener、GUI spawn、専用 `FirstPartyControlSeat` と `ConfirmationSession` を所有する。
- **text GUI**: `apps/ene-desktop`。Host が起動し、Client channel と継承した専用確認 channel を話す。Body の親。通常起動時は Host 起動 / OpenDesktop を行う短命 launcher。
- **VRM overlay**: `apps/ene-body`。Host にも Client protocol にも接続しない。GUI の任意 child。
- **CLI**: `apps/ene-ctl`。Client channel のみ。control は話さない。
- **共有**: `crates/ene-client`（A1 で `ene-ctl` から抽出）、`crates/ene-local-control`（`ene-api` に載せない。秘密フィールドは redacted。確認完了は seat に束縛した session）。

作らない: `ene-stage`, `ene-stage-ui`, `ene-vrm`, `ene-tray-linux`。

製品起動と再起動は first-party-desktop 第2・5節。launcher が必要時に Host を detach 起動し、Host が GUI へ専用 endpoint を渡す。GUI close で Host を止めない。公開 listener は要求専用であり、GUI の再接続を任意 process の先着競争にしない。

同一 process に avatar を入れる案、Body を第2の paired Client にする案、Body を Host に入れる案は不採用。根拠は first-party-desktop 第3節。

### 3.2 依存（確定する不採用と provisional）

| 領域 | 状態 | 内容 |
|---|---|---|
| Text GUI | provisional preferred: Slint（winit、Royalty-free Desktop + AboutSlint） | 確定不採用: egui/eframe、GTK/relm4、Tauri/webview。iced は Slint 技術成立 probe 失敗時の fallback 候補のみ |
| VRM | provisional preferred: `vrm-runtime` + 自前 wgpu。SpringBone は落とさない | 確定不採用: Bevy / bevy_vrm、`ene-vrm` 名。gltf extras 手書きは probe 失敗後の再選定 |
| Overlay | Body の wgpu は確定。OS 手段は provisional: Windows layered/DWM、KDE `zwlr_layer_shell_v1` + input region | 確定不採用: winit AlwaysOnTop on Wayland、text GUI への overlay 合成 |
| Secrets | OS 保護ストア抽象は確定。`keyring` crate は provisional adapter | 製品 GUI 経路の `EnvCredentialStore` 正本化は不採用 |

### 3.3 Trust / credential（確定）

Client channel（`ene-api`）は高権限の候補だけを受け、最終確認は成立させない。Host-local control は複数 requester の要求専用 listener と、Host-spawned GUI だけの専用確認 channel を分ける。`ene-core approve-*` は要求 / 非秘密 outcome だけを扱い、offline mutation fallback を持たない。Host は起動した child と専用 endpoint から seat を発行し、session を操作・前提・Host incarnation / seat generation に束縛する。GUI の直接操作後だけ owner が確定する。保証前提は Runtime Topology、手順は first-party-desktop 第5節を正本とする。

credential 生値の区間は first-party-desktop 第5.2節、有効化・失効・並行制御・crash 復旧は [Credential publication](../../design/concrete/credential-publication.md) を正本とする。OS candidate の保存だけで登録完了にせず、sweep / active version / revision の commit と snapshot 公開後に応答する。秘密を通常 DTO、log、DB、GUI 永続 state へ出さず、破棄を Targeted Deletion に預けない。

### 3.4 Probe と acceptance の分離（未実施。合格したとは書かない）

NixOS 26.11 は Support Matrix の Linux 対象だが、この時点では正式リリース前である。公式 26.11 が無いことを Stage 7 全体の停止理由にしない。正本は first-party-desktop 第9節。

**今実施する技術成立 probe**: 現在使える KDE Wayland（nixpkgs / Plasma を記録）と Windows 11。Host-spawned child への限定継承・空席時の不正接続拒否は A1b の gate。IME / 秘密入力面 / OS store adapter は B の gate。overlay / VRM runtime + SpringBone / 実表示計測は D の gate。26.11 最終 acceptance の代用にはしない。

**A1**: GUI / overlay probe は待たない。Stage 6 完了後に A1a → A1b / A1c の順で進める。control 起動由来と credential publication の fault / race gate は省略しない。製品 GUI がまだ無い時点の高権限要求は ConfirmationUnavailable とする。

**後日の最終 acceptance**: slice F。Windows 11 と正式リリースされた NixOS 26.11 KDE Wayland。今の KDE Wayland probe 成功を 26.11 合格と書かない。26.11 が F 時点で未リリースなら Linux 最終 acceptance は open のまま残す。

**A0 design gate**: 要件の本人による直接確認を維持する。要求接続と確認 channel、Host による GUI 起動由来、OS・インストール完全性の保証前提、秘密の有効化時点と復旧を確定する。CM / IPC / IB / PR / CCT に同じ契約を反映する。provisional 依存と未実施 probe を合格扱いしない。

**A0 技術成立 probe（残。Stage 7 全体の blocker ではない）**: 上記。記録しただけで潰したことにしない。

## 4. PR 分割と各 slice の gate

各 ID は実装作業の参照名です。crate / module / binary 名は §3.1 に従います。

### A1: Client 接続・確認経路・credential publication

[First-party desktop](../../design/concrete/first-party-desktop.md) 第5節と [Credential publication](../../design/concrete/credential-publication.md) に従い、次の小さな PR に分ける。GUI toolkit / overlay の production は含めない。

| Slice | 範囲 | gate |
|---|---|---|
| A1a | `ene-ctl` から `ene-client` を抽出。GUI から使う非同期接続・correlation・device identity・erasure adapter | 実 socket / named pipe の既存 Client 回帰、connection replacement、slow consumer、待機中の deletion demand。event loop を塞がず bounded queue / cancellation を持つ |
| A1b | 要求専用 listener、公式 GUI spawn / 限定継承、seat と session、`approve-*` の requester 化 | 空席 / 占有中とも一般 requester は承認不能。CLI と GUI は同時利用可能。GUI 不在時の起動、起動不能の明示 outcome、切断 / restart / stale / 二重完了を検証 |
| A1c | OS-store port、candidate / version / mutation 記録、approval sweep と revision の commit、snapshot / lease の公開、startup / cleanup | Credential publication 第6節の全 race / crash / no-secret gate。OS write の成否不明を自動再実行せず、read-only は repair しない |

A1b / A1c は A1a 後、共通 session / mutation outcome の最小 interface が確定した範囲で並列化してよい。A1c の実 OS adapter は provisional のまま port と fault-injection test を先行できるが、fake / env store を製品経路へ配線しない。実 adapter が無い場合は unavailable。両 OS adapter probe の合格は B の製品セットアップ有効化の前提である。

共通 gate:

- connection / incarnation / presence を混同せず、古い connection の入力・receipt・cursor・選択を引き継がない。未送信本文や mutation を reconnect で自動 replay しない。
- `ene-ctl` は control を話さない。GUI は seat の有無を Client の会話権限にしない。
- requester は要求と非秘密 outcome のみ。GUI の起動中も含め seat は高々1つ。公開 listener に completion / secret frame を送っても拒否し、nonce や PID の自己申告でも昇格しない。
- 両 OS の実 child process / 継承 endpoint で provenance を検証する。Body 等への handle 漏れ、同時 OpenDesktop、child exit、channel 断、Host restart、旧 endpoint の完了を検証する。GUI 不在のテスト child は test-only に限定し、製品の本人確認を代行しない。
- GUI close / session expiry と owner commit の両順序を barrier で検証する。commit が先なら確定 outcome を保持し、切断が先なら mutation ゼロ。
- `ene-core approve-*` は serving Host の requester とし、Host 不在は HostUnavailable、確認面が無ければ ConfirmationUnavailable。offline DB mutation / CLI の自己承認を製品 fallback に残さない。
- credential の有効化・失効と旧世代の scrub / claim / 遅延結果、OS put / DB commit / snapshot 公開 / 応答の各 crash point を検証する。詳細な期待結果は Credential publication に従う。

A1 の完了は Host / transport / publication 基盤の成立であり、実 GUI の直接確認と初回セットアップの合格は B で記録する。

### B: 初回セットアップからテキスト会話までの縦断 GUI

`ene-desktop` で avatar に依存しない製品経路を通す。A1a–c、text GUI toolkit、秘密入力面、両 OS の実 credential-store adapter probe の後に製品セットアップを有効にする（toolkit は Slint が既定候補。失敗時に iced を再 probe）。NixOS 26.11 公式 acceptance は待たない。

1. 新規 data directory から、言語・同梱 `ene`・送信データと費用の説明・credential 登録・model 割り当てまでを案内する。Host の明示起動とローカル接続の準備も扱い、手動の環境変数設定や DB 編集を通常の完了手順にしない。
2. credential 登録と provider 利用への同意・割り当てを分ける。登録だけでは provider 呼出しを行わず、初期設定の失敗や再起動で暗黙に同意を補わない。
3. 一対一 timeline、入力、stream、履歴 page、接続・presence の表示を実装する。管理画面は Companion 停止中・provider 不通・avatar 不在でも開けるようにする。
4. 日本語 IME の未確定入力を誤送信せず、日英のラベル・拒否理由を切り替えられるようにする。言語切り替えで会話本文や domain state を書き換えない。

**gate**: acceptance §1 を新規環境から GUI で通す。Host が起動した GUI の直接確認、CLI requester との併用、キー登録のみでは provider 0 呼出し、割り当て後の会話、restart 後の履歴、日英切り替えを確認する。キー更新中の provider 応答待ち・store locked・応答喪失でも二重有効化や秘密露出がないことを確認する。C1 の入力コピーは intake 後に自己破棄し、toolkit / IME の残差は観測・記録する。OS put 成功を登録完了や本人確認と表示しない。

### C1: Memory と由来の管理画面

**範囲**: 現在の記憶、scope、importance、作成日時、Experience Summary の根拠、過去 revision を確認できるようにします。構造化された bounded owner query が足りない箇所は、query → wire DTO → GUI の順に最小追加します。

**gate**: acceptance §3.1–§3.10 を会話と管理画面で確認すること。訂正・状況変化・重複統合・通常の忘却は既存 Learning / Companion 経路で行い、GUI に独自の形成判断や物理削除 shortcut を作りません。

### C2: Task と Workspace の管理・復旧画面

**範囲**: Task 一覧、revision、実行登録の有無、Action の確定度、結果と採用状態、変更ファイル・保存先・残作業を表示し、対象選択、追加指示、cancel、明示 resume を既存 command に接続します。Task の作成はパートナーからの既存委任経路を通します。

**gate**: acceptance §4 と §5 の GUI 操作対象を通すこと。Unknown / 中断 / Failed / Cancelled / Completed、cancel の受付と作用の停止完了を区別します。resume は表示した Task revision / purpose に束縛し、stale を最新前提へ自動置換しません。

### C3: usage / cap・削除・安全な復旧の管理画面

**範囲**: token と費用の内訳、provider / model / consumer / purpose、期間・ページ、cap と消費内訳を既存 query で表示します。cap 更新は既存 revision / intent 境界を通し、UI で独自集計した残額を利用許可にしません。

Targeted Deletion は通常の忘却と分け、request → Host-local 最終確認（control）→ operation / participant status → 必要時の明示 resume を接続します。

**gate**: acceptance §3.11–§3.12、§6、§7 を対象に、Unknown をゼロ料金と表示しないこと、古い cap view の拒否、削除の Held / Finalizing / Completed の区別、到達不能 Client が完了の証拠にならないことを確認する。

### D: VRM desktop avatar と text fallback

**範囲**: `ene-body` で同梱 `ene` を表示する。overlay / VRM runtime / SpringBone の技術成立 probe が潰れてから入る（既定候補 `vrm-runtime`）。NixOS 26.11 公式 acceptance は待たない。まず透明表示・移動・resize・非表示/再表示を通し、次に既存の応答状態に従う待機/発話中の表情・仕草を接続します。新しい感情推論や Voice pipeline は作りません。

renderer に渡す情報は必要なアセット参照と表示指示に限定し、会話本文、Memory、API key、management authority を渡しません。avatar の非表示・終了を Companion 停止や Task cancel と同一視せず、renderer 再起動で古い会話/操作を replay しません。

**gate**: B の実 GUI へ統合して acceptance §2 を両実 desktop で通すこと。renderer の異常終了・hang・初期化失敗を注入しても、text 入力、Task cancel、設定/復旧が利用できることを確認する。別 Client や test shell が生存しているだけでは GUI fallback 合格にしない。renderer 単体の作業は B と並行してよい。

### E: GUI の提示・一時データ・障害経路の横断検証

**範囲**: 新しい GUI を Stage 5 / 6 の presentation・erasure・secret safety の参加者として仕上げます。安全な受信・破棄は B / C の各 PR に含め、この slice まで未実装のまま後回しにはしません。

- timeline、Memory の根拠/履歴、Task report、検索/入力 draft、IME composition、undo、deferred frame など、GUI が実際に所有する本文コピーを inventory する。
- deletion demand では該当コピーと旧 receipt / view を無効化・消去し、実際の消去確認後にだけ `wiped` を返す。
- presentation ACK は、その receipt の項目を実際に提示した経路から返す。単なる受信を Presented にしない。

**gate**: 実 GUI adapter と serving Host を使い、Stage 6 完了後の遅延結果、Host restart 越しの Client delivery evidence、登録秘密の各画面/エラーへの非露出を回帰すること。

### F: Milestone 1 の実機 acceptance / performance と closeout

**範囲**: §6 の全行を統合 tip の GUI から確認し、§7 の測定生データと結果を残す。測り方は [First-party desktop](../../design/concrete/first-party-desktop.md) 第8節。閾値は acceptance の Performance Gates。Body を除外しない。Linux 検証は Cloud Agent 上の Linux（記録した distro / compositor）で行う。NixOS 26.11 公式 desktop を待たない。KDE Wayland overlay 成功を X11 で代替しない。

**gate**: 実施した OS・言語の対象 scenario と、測れた性能生データを残すこと。未実施、失敗、暫定回避は成功と区別する。Windows 11 と Performance Gate と overlay/VRM が残っていれば Stage 7 を完了とせず、`PROGRESS.md` を Stage 8 へ進めない。Linux 側の実施記録は [reports/stage-7-linux-2026-09-19.md](../reports/stage-7-linux-2026-09-19.md)。

## 5. 依存順と並列化

```text
A0 design + Stage 6 closeout → A1a → A1b / A1c → B → C1 / C2 / C3 → E → F
GUI / secret-input / OS-store probe ──────────────→ B
Overlay / VRM / measurement probe → D renderer ───→ D integration → E
                                                  ↑
                                                  B
```

A1a は GUI / overlay probe を待たない。A1b は child provenance、A1c は credential publication の gate を満たす。B は A1 全体と GUI / 秘密入力 / 実 OS-store probe の後。D の単体 renderer は独立に進められるが、GUI fallback の完了 gate は B への統合後である。Linux 検証は F で実施し、NixOS 26.11 公式 desktop を待たない。

C1 / C2 / C3 は異なる owner と画面に分け、共通 Client boundary・DTO・schema が確定した範囲だけ並列化します。

## 6. acceptance と証拠の対応

| acceptance | 主担当 | Stage 7 で追加する first-party 証拠 |
|---|---|---|
| §1: セットアップと最初の会話 | A0 / B | 新規環境から GUI だけで同意・登録・割り当て・会話。登録のみでは送信ゼロ。日英切り替えと再起動後の履歴 |
| §2: Desktop Body | A0 / D | 実 desktop の同梱 `ene`、透過・移動・resize・非表示・待機/応答、renderer 故障中の text / management |
| §3.1–§3.10: Memory | C1 | 会話由来の形成・訂正・状況変化・想起・統合・通常忘却と、scope / importance / 根拠 / revision の GUI 確認 |
| §3.11–§3.12: Targeted Deletion | C3 / E | GUI での要求・control 確認・状態/復旧、GUI 一時コピーの消去 |
| §4: Workspace Task | C2 | GUI からの会話→委任→並行会話/追加指示/cancel→結果 |
| §5: S5-01〜S5-24 | A1 / C2 / E | 既存の全 Host / transport 回帰に加え、GUI close / reconnect / presentation ACK / manual resume を実経路で確認 |
| §6: 障害とデータ保護 | B / C2 / C3 / D / E | 認証失敗、provider 不通、renderer/Agent crash、cancel、保存中終了。無断再実行ゼロ |
| §7.1–§7.2: 費用と機密 | C3 / E | token と費用内訳の表示、Unknown の区別、登録秘密の非露出 |
| Performance Gates | A0 / D / F | 両 OS の全 process 計測、実描画 FPS と操作受付反映。分母は first-party-desktop 第8節 |

protocol / currentness / failure は、実 Host・store・Client transport と barrier 制御した fake provider で自動化します。headless CI の成功は実 GPU / compositor / IME / desktop 性能の合格と別です。

## 7. 性能測定

閾値の正本は [acceptance の Performance Gates](../../requirements/acceptance.md#性能基準-performance-gates) です。測り方・busy-wait 検出・縮退順は [First-party desktop](../../design/concrete/first-party-desktop.md) 第8節。基準を引き下げません。

- idle CPU: セットアップ後・推論なし 5 分。Host + desktop + Body を全て計上。
- resident: 同じ区間の合計 ≤ 2 GiB。Body を除外しない。
- avatar: 実表示済みの content update と時刻から平均 ≥ 30 FPS を確認する。Wayland presented / discarded、Windows display timing を使い、redraw / frame callback / Present 呼出し回数は代用しない。欠測・計測不能は合格にしない。
- 操作受付: 入力から 1 秒以内に受付表示。Body frame を待たない。

通常 avatar 表示中の idle を基準にし、非表示・renderer 停止時は補助測定として分けます。

## 8. docs と closeout の運用

| 場所 | 更新する内容 |
|---|---|
| この文書 | Stage 7 の範囲、slice、依存、検証対応。A0 結論は §3 |
| `docs/design/` | first-party-desktop、credential-publication、Runtime Topology と CM / IPC / IB / PR / CCT。production code より先 |
| `docs/requirements/acceptance.md` | 原則変更しない |
| Issues / PRs | 個別不足、probe 結果、exact tip。pass / fail / 未実施を区別する |
| `PROGRESS.md` | current / completed / blocker / next の短い index のみ。Stage 6 完了前に Stage 7 を current としない |

次に残る検証は Windows 11、overlay / VRM / IME、Performance Gate。Linux 自動テストと X11 GUI の記録は [reports/stage-7-linux-2026-09-19.md](../reports/stage-7-linux-2026-09-19.md)。NixOS 26.11 を Linux 検証の完了条件にしない。

### A1 trust boundary の実装状況（2026-09-20）

`ene-local-control` は要求専用 listener（`ToHost` / `FromHost`: 要求と非秘密の request state。challenge・秘密・completion の frame 型を持たない）と、Host が起動した GUI にだけ渡す専用確認 channel（`ToConfirmation` / `FromConfirmation`）に分離した。seat は Host の spawn から発行し、空席の先着では取得できない。`ene-core approve-*` は requester であり、pairing secret も credential 生値もその stdout / outcome には出ない。offline mutation fallback は削除した。

初回 pairing の認証専用 provision frame（IPC §9.2）は未実装。現在 pairing secret は Host-spawned GUI の確認 channel に届き、Client 側は `ENE_PAIRING_SECRET` / GUI の `connect_with_bootstrap` 経路のままである。この差分は A1 の残作業として残す。

#### A1c: credential publication（実装済み）

`SecretVersionId` は値から導出しない採番であり、OS item は installation namespace と version ごとに作る。`credential_mutation` は attempt を write-once で記録し、`credential_active` は active / retired version だけを持つ（どちらも非秘密）。`activate_credential` は一つの transaction で、候補と置換対象の sweep、usable ref、active version、credential-set revision、`Activated` とその outcome を一緒に commit する。前提 revision が動いていれば候補は adopt せず sweep し、`Stale` を durable に残すので retry は保存済みの決定を返す。

実 OS store adapter は `keyring` 経由で version item を作り、素の `put` を拒否する（owner の publish→activate だけが値を usable にする）。起動時は登録済み credential の active version を durable 記録から読み、item が読めることを確認してから adapter を向ける: 読めない active version は unactive のままにする。未完了 mutation（`Prepared` / `Staged`）は再実行も activate もしない。

Windows Credential Manager での probe は成功（version 作成・読み戻し・上書き拒否・activate・削除）。Linux Secret Service は adapter を実装済みだが、この環境に service が無いため probe は 未実施であり、合格とは書かない。

#### 未実施のまま残るもの

- 実 overlay: `apps/ene-body` の overlay backend は Headless のまま（Windows DWM / KDE layer-shell は `NotRun`）。VM runtime / SpringBone / 表情も未実装。
- 公式同梱 `ene` VRM（[#1651](https://github.com/pexisgle/ene/issues/1651)）。
- Windows 11 実機 acceptance と Performance Gate（idle CPU / resident / 実表示 FPS / 1 秒受付）。
