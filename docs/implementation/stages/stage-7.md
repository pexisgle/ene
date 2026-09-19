# Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 7 の**実装範囲、PR 分割、依存関係、検証 gate**を管理します。製品 behavior は [requirements](../../requirements/README.md)、受け入れ範囲は [acceptance](../../requirements/acceptance.md)、authority と状態の意味は [design](../../design/README.md) を優先します。計画の存在は着手・完了を意味しません。

first-party desktop の process / trust / 依存 / 性能の分母は [First-party desktop](../../design/concrete/first-party-desktop.md) が所有します。本文書は slice 順と gate だけを持ち、同じ契約本文をコピーしません。

## 1. 目的と開始条件

Stage 0〜6 の機能を、初回セットアップから日常の会話・管理・障害復旧まで使える first-party GUI に結び、Milestone 1 を完了します。CLI の一覧を画面へ貼り付けるだけでなく、実際の GUI 入力・提示・確認を通して既存の安全性を維持することが目的です。

計画・調査は Stage 6 の最終修正と並行してよいですが、Stage 7 の production slice は Stage 6 の完了 gate を満たした統合 base に積みます。Stage 6 に属する不具合を GUI 側の回避処理で隠さず、先に元の owner で修正します。Stage 6 の完了記録は Stage 6 closeout が担当します。`PROGRESS.md` は Stage 6 が current のままです。

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
- high-privilege の最終確認を remote input、LLM、tool、plugin、Computer Use 結果電文、通常の Client、席が埋まっているときの第二接続が代理しない。nonce は freshness だけとし、完了は exclusive `FirstPartyControlSeat` に束縛する。空席先着の真正性は証明しない。
- UI / renderer の障害で Host-only Task や text / management の利用を壊さない。
- usage / cost / cap、Targeted Deletion、Task recovery 等は既存 semantic owner の query / command / currentness を使う。
- GUI 専用の authoritative DB、permission registry、presence registry、accounting registry を作らない。
- acceptance と performance gate を実装都合で弱めない。

## 3. A0 結論（design。production Rust はまだ書かない）

A0 の成果物は [First-party desktop](../../design/concrete/first-party-desktop.md) と、それを指す CM / IPC / IB の更新です。`.old/` の `ene-stage` は移植しない。未検証 crate を恒久 contract として固定しない。

### 3.1 Process / 名前（確定）

- **Host**: `apps/ene-core`。GUI / wgpu を持たない。serving 中に control listener、exclusive `FirstPartyControlSeat`、`ConfirmationSession` の minter を持つ。
- **text GUI**: `apps/ene-desktop`。Client channel と control を話す。Body の親。first-party 確認面。
- **VRM overlay**: `apps/ene-body`。Host にも Client protocol にも接続しない。GUI の任意 child。
- **CLI**: `apps/ene-ctl`。Client channel のみ。control は話さない。
- **共有**: `crates/ene-client`（A1 で `ene-ctl` から抽出）、`crates/ene-local-control`（`ene-api` に載せない。秘密フィールドは redacted。確認完了は seat に束縛した session）。

作らない: `ene-stage`, `ene-stage-ui`, `ene-vrm`, `ene-tray-linux`。

製品起動: `ene-desktop` が Host 未起動なら `ene-core serve` を detach 起動する。Host を GUI の子にしたままにしない。

同一 process に avatar を入れる案、Body を第2の paired Client にする案、Body を Host に入れる案は不採用。根拠は first-party-desktop 第3節。

### 3.2 依存（確定する不採用と provisional）

| 領域 | 状態 | 内容 |
|---|---|---|
| Text GUI | provisional preferred: Slint（winit、Royalty-free Desktop + AboutSlint） | 確定不採用: egui/eframe、GTK/relm4、Tauri/webview。iced は Slint 技術成立 probe 失敗時の fallback 候補のみ |
| VRM | provisional preferred: `vrm-runtime` + 自前 wgpu。SpringBone は落とさない | 確定不採用: Bevy / bevy_vrm、`ene-vrm` 名。gltf extras 手書きは probe 失敗後の再選定 |
| Overlay | Body の wgpu は確定。OS 手段は provisional: Windows layered/DWM、KDE `zwlr_layer_shell_v1` + input region | 確定不採用: winit AlwaysOnTop on Wayland、text GUI への overlay 合成 |
| Secrets | OS 保護ストア抽象は確定。`keyring` crate は provisional adapter | 製品 GUI 経路の `EnvCredentialStore` 正本化は不採用 |

### 3.3 Trust / credential（確定）

Client channel（`ene-api`）では high-priv 最終確定を成立させない。control channel は request / 秘密 intake / `ConfirmationSession` 完了返送に使うが、**nonce は freshness だけ**である。同一 OS ユーザー、data dir を読めること、control socket を開けたことは Owner 本人の確認ではない。Host は exclusive `FirstPartyControlSeat`（同時に高々1つの control 接続）に session を束縛し、その同一接続・同一 peer PID からの完了だけを受理する。席の exclusive 性は取得者を公式 `ene-desktop` だと証明しない。空席へ先着した同一 UID process が席を取る。席が埋まっているときの第二接続の nonce 提示、製品 `ene-ctl`、通常 Client、LLM、tool、plugin、Computer Use の `EffectReport` は完了ではない。ene 認可の Computer Use は確認面・秘密面を対象にできない（クリック後の区別はできないため、対象から外す）。空席先着と、seat 保持 process への ene 外入力注入はプロトコルの残差であり、Owner とは再分類しない。任意の同一 UID process を必ず排除できるとは書かない。

credential 生値の区間と破棄は first-party-desktop 第5.2節。ene 所有の C1–C3–C5 は破棄する。通常 DTO の Debug、log、`app.db`、GUI 永続 state、Targeted Deletion に秘密寿命を依存させない。widget を erasure participant に見立てない。toolkit / IME / OS が作る複製は保証できない残差として書き、無いとはしない。

### 3.4 Probe と acceptance の分離（未実施。合格したとは書かない）

NixOS 26.11 は Support Matrix の Linux 対象だが、この時点では正式リリース前である。公式 26.11 が無いことを Stage 7 全体の停止理由にしない。正本は first-party-desktop 第9節。

**今実施する技術成立 probe**: 今使える KDE Wayland（実際の nixpkgs / Plasma を記録。26.11 の代用ではない）と、用意できた Windows 11。IME / overlay / VRM runtime+SpringBone / OS store adapter / crash isolation / 測り方。B/D の production だけが該当 probe を待つ。

**A1**: GUI / overlay probe を待たない。Stage 6 完了後の統合 base に積んでよい。

**後日の最終 acceptance**: slice F。Windows 11 と正式リリースされた NixOS 26.11 KDE Wayland。今の KDE Wayland probe 成功を 26.11 合格と書かない。26.11 が F 時点で未リリースなら Linux 最終 acceptance は open のまま残す。

**A0 design gate（本 PR）**: 名前と境界が要件から説明できる。同一 UID を Owner 確認に弱めていない。`ConfirmationSession` が freshness 以上の seat 束縛を持つ。席の exclusive 性と接続束縛を取得者の真正性だと書いていない。空席先着を残差として明記している。任意 local process を必ず排除できると書いていない。Computer Use は文言禁止ではなく denylist で強制する。credential 区間と toolkit/OS 残差が書かれている。未検証依存が provisional である。probe と最終 acceptance が分離されている。current design と異なる箇所は production より先に更新されている。

**A0 技術成立 probe（残。Stage 7 全体の blocker ではない）**: 上記。記録しただけで潰したことにしない。

## 4. PR 分割と各 slice の gate

各 ID は実装作業の参照名です。crate / module / binary 名は §3.1 に従います。

### A1: Client 接続と serving 中 control

[First-party desktop](../../design/concrete/first-party-desktop.md) に従い、GUI が必要とする Host 接続を `ene-client` に抜き、serving Host に control listener を足す。`ene-core approve-*` は serving 中 control を使い、未起動時だけ現行の offline lock を使う。Host は exclusive `FirstPartyControlSeat` と、それに束縛した `ConfirmationSession` を持つ。GUI toolkit / overlay の production は含めない。

- GUI event loop を接続待ち・provider 待ちで塞がない。
- connection / incarnation / presence を混同しない。
- 古い connection の入力・receipt・cursor・選択状態を継承しない。
- 再接続で未送信本文や mutation command を自動 replay しない。
- bounded queue と cancellation を持ち、遅い Client が Host-only Task を止めない。
- 製品 `ene-ctl` は control を話さない（公式バイナリの契約。同一 UID の任意 process が空席を取れないことではない）。既存 Client 回帰を維持する。
- seat は高々1つ。席が埋まっているときの第二の control speaker は `SeatOccupied` / `DeniedByBoundary`。
- 完了は mint 時の同一 control 接続・同一 peer PID からだけ。別接続の nonce 提示、live session 無しの `{ confirmed: true }`、Client `confirmed=true` は拒否する。seat 再接続は outstanding session を無効化する。
- `EffectReport` および Client 経路は session を完了できない。
- credential 生値は `ene-api` に載せない。control の秘密フィールドは redacted。通常 DTO の Debug / log に出さない。secret intake も seat 接続だけ。
- 空席への同一 UID 先着を公式 GUI だと認証しない。A1 で真正性証明を足さない。

**gate**: 実 socket / named pipe で既存 Client 回帰が維持され、GUI 側でも connection replacement、response correlation、slow consumer、待機中の deletion demand を検証できること。serving 中に control 経由の approve / credential `put` が、seat に束縛した Host session でのみ通ること。session 無し・席が埋まっているときの別接続の nonce・Client `confirmed=true`・製品 `ene-ctl` からの control 接続は拒否されること。接続の取り直しで旧 session が使えないこと。空席先着を公式 GUI 合格と書かないこと。

### B: 初回セットアップからテキスト会話までの縦断 GUI

`ene-desktop` で、avatar に依存せず最初の製品経路を通します。text GUI toolkit の技術成立 probe が潰れてから入る（既定候補 Slint。失敗時のみ iced を再 probe）。NixOS 26.11 公式 acceptance は待たない。

1. 新規 data directory から、言語・同梱 `ene`・送信データと費用の説明・credential 登録・model 割り当てまでを案内する。Host の明示起動とローカル接続の準備も扱い、手動の環境変数設定や DB 編集を通常の完了手順にしない。
2. credential 登録と provider 利用への同意・割り当てを分ける。登録だけでは provider 呼出しを行わず、初期設定の失敗や再起動で暗黙に同意を補わない。
3. 一対一 timeline、入力、stream、履歴 page、接続・presence の表示を実装する。管理画面は Companion 停止中・provider 不通・avatar 不在でも開けるようにする。
4. 日本語 IME の未確定入力を誤送信せず、日英のラベル・拒否理由を切り替えられるようにする。言語切り替えで会話本文や domain state を書き換えない。

**gate**: acceptance §1 を新規環境から GUI で通し、キー登録直後の provider 呼出しゼロ、割り当て後の会話、Host 再起動後の履歴、日英切り替えを確認する。キーを通常本文・ログ・エラー・保存した GUI state に残さない。入力 widget は C1 の自己破棄を行い、Targeted Deletion に秘密寿命を預けない。toolkit / IME / アクセシビリティに残る複製は残差として記録し、無いとはしない。高権限は seated first-party 面と Host の seat 束縛 session に限る。

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

**gate**: acceptance §2 を両実 desktop で通すこと。renderer の異常終了・hang・初期化失敗を注入しても、text 入力、Task cancel、設定/復旧が利用できることを確認します。別の Client surface が生存しているだけでは GUI fallback の合格にしません。

### E: GUI の提示・一時データ・障害経路の横断検証

**範囲**: 新しい GUI を Stage 5 / 6 の presentation・erasure・secret safety の参加者として仕上げます。安全な受信・破棄は B / C の各 PR に含め、この slice まで未実装のまま後回しにはしません。

- timeline、Memory の根拠/履歴、Task report、検索/入力 draft、IME composition、undo、deferred frame など、GUI が実際に所有する本文コピーを inventory する。
- deletion demand では該当コピーと旧 receipt / view を無効化・消去し、実際の消去確認後にだけ `wiped` を返す。
- presentation ACK は、その receipt の項目を実際に提示した経路から返す。単なる受信を Presented にしない。

**gate**: 実 GUI adapter と serving Host を使い、Stage 6 完了後の遅延結果、Host restart 越しの Client delivery evidence、登録秘密の各画面/エラーへの非露出を回帰すること。

### F: Milestone 1 の実機 acceptance / performance と closeout

**範囲**: §5 の全行を統合 tip の GUI から確認し、§6 の生データと結果を残します。測り方は [First-party desktop](../../design/concrete/first-party-desktop.md) 第8節。閾値は acceptance の Performance Gates。Body を除外しない。Linux 最終合格は正式リリースされた NixOS 26.11 KDE Wayland。今の KDE Wayland 技術成立 probe で代替しない。

**gate**: 両 OS・両言語の対象 scenario と性能基準を満たすこと。未実施、失敗、暫定回避は成功と区別します。26.11 が未リリースなら Linux 最終 acceptance は open のまま残し、他 slice の完了記録を偽らない。完了後にだけ `PROGRESS.md` を Stage 8 へ進めます。

## 5. 依存順と並列化

```text
A0 architecture / technology decision
        ├→ A1 → B → C1 ─┐
        │          ├→ C2 ─┼→ E → F
        │          └→ C3 ─┤
        └────────────→ D ──┘
```

A0 の design は本 PR。A0 の技術成立 probe は B/D の blocker であり、Stage 7 全体および A1 の blocker ではない。A1 は Stage 6 完了後の統合 base に積む。NixOS 26.11 公式 desktop は F の Linux 最終 acceptance であり、A1/B/D を止めない。

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
- avatar: present 時刻で平均 ≥ 30 FPS。要求した redraw 回数を FPS と数えない。
- 操作受付: 入力から 1 秒以内に受付表示。Body frame を待たない。

通常 avatar 表示中の idle を基準にし、非表示・renderer 停止時は補助測定として分けます。

## 8. docs と closeout の運用

| 場所 | 更新する内容 |
|---|---|
| この文書 | Stage 7 の範囲、slice、依存、検証対応。A0 結論は §3 |
| `docs/design/` | first-party-desktop と CM / IPC / IB。production code より先 |
| `docs/requirements/acceptance.md` | 原則変更しない |
| Issues / PRs | 個別不足、probe 結果、exact tip。pass / fail / 未実施を区別する |
| `PROGRESS.md` | current / completed / blocker / next の短い index のみ。Stage 6 完了前に Stage 7 を current としない |

次に実装する slice は **A1**（Stage 6 完了後）。B は text GUI の技術成立 probe の後。D は overlay / VRM runtime の技術成立 probe の後。NixOS 26.11 公式 acceptance は F まで待ってよい。
