# First-party desktop の実行構成 — Host / GUI / Body

本書は、first-party のデスクトップ実行（初回セットアップ、text GUI、透明 VRM overlay、Host-local の高権限確認）について、プロセス境界・信頼境界・依存選定・性能の測り方を具体化する。製品 behavior は [要件](../../requirements/requirements.md) と [受け入れ条件](../../requirements/acceptance.md) が正本である。Host が durable authority であること、connection / incarnation / presence / presentation を同一視しないこと、credential 生値を Client wire・DB・log に出さないことは、[実行トポロジ](../architecture/runtime-topology.md)、[Host↔Client IPC](host-client-ipc.md)、[インターフェース境界](interface-boundaries.md) が既に固定しており、本書はそれを再設計しない。

crate 名と依存方向の表は [Crate / Module 分解](crate-module-decomposition.md) が所有する。本書は **process 寿命、障害境界、Host-local control、Owner 確認、credential の区間と破棄、投影 IPC、性能の分母、provisional 依存、実機 probe と最終 acceptance の分離** を所有する。上位設計との優先順位は [設計文書 README](../README.md#設計文書の優先順位と信頼できる情報源) に従う。

`.old/` の `ene-stage` 画面・3D 合成・`.slint` 資産は参考にも移植対象にもしない。text GUI toolkit を採るのは旧実装の継続ではなく、レイアウト自由度に対する独立選定である。未検証の crate / OS backend は第7節の **provisional** であり、probe 合格前に恒久 contract として固定しない。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- first-party 実行を構成する process と寿命（第2節）。
- text UI と VRM overlay を同一 process にしない理由、および Body を paired Client にしない理由（第3節）。
- desktop と Body の投影 IPC（Host Client プロトコルではない）（第4節）。
- Client channel と Host-local control channel の区別、同一 OS ユーザーを Owner 確認と同一視しないこと、credential の全区間と破棄条件、失敗の非同一視（第5節）。
- application / crate / module の命名と「作らない名前」（第6節）。詳細な Host ドメイン分解は CM が所有する。
- 確定する不採用と、probe 前は provisional に留める依存（第7節）。
- acceptance の Performance Gates を弱めない測り方と縮退順（第8節）。
- 今実施できる技術成立 probe と、Support Matrix 上の最終 acceptance の分離（第9節）。この文書を書いた時点ではどちらも未実施である。

### 1.2 本書が決めないもの

- Host 内部の semantic owner、CAS、store 契約。
- Client wire の DTO 一覧（IPC が所有する）。control のバイト列・メッセージ名の最終形。
- 画面レイアウト、文言、テーマ。
- MToon GPU shader の完成度。未完の間は unlit/PBR fallback を許す。
- 同じ Client に複数個体が居るときの Body process 数（Milestone 1 は同梱 `ene` 1体で Body は1 process。GUI process に戻してはいけない、だけを固定する）。
- Voice、Observation、tray、remote Client、character 配布。Milestone 1 の非対象。
- 未検証 crate のバージョン pin を恒久 contract にすること（第7節）。

## 2. 導出した process 構成

必要な責務は3つで、寿命が違う。この分割自体は確定である。toolkit 名は第7節の provisional を指す。

```mermaid
flowchart TB
  owner[Owner gesture]
  desktop["ene-desktop text GUI / first-party confirmation surface / Client"]
  body["ene-body VRM overlay only"]
  host["ene-core Host composition / OS credential store"]
  ctl["ene-ctl CLI Client"]

  owner -->|"明示確認。同一 UID ではない"| desktop
  owner --> ctl
  desktop -->|"Client channel ene-api pairing/session"| host
  desktop -->|"control: request / secret intake / session completion"| host
  host -->|"ConfirmationSession を first-party 面へ提示"| desktop
  desktop -->|"projection IPC no secrets / no domain"| body
  ctl -->|"Client channel only"| host
```

- **Host (`apps/ene-core`)** は GUI / wgpu を持たない。Client 切断後も Task を続ける。control listener を serving 中に持つ。Body process の有無を知らない。高権限の `ConfirmationSession` を発行する唯一の minter である。
- **First-party GUI (`apps/ene-desktop`)** は通常ウィンドウの text / management。Host より短命。閉じても Host は止まらない。overlay / 3D は持たない。Host の Client channel と control channel の両方を話す。Body の親。高権限の最終確認面はここだが、socket を開けたことや同一 UID であることが Owner 確認ではない（第5.1節）。
- **Body (`apps/ene-body`)** は GUI の任意 child。Host にも Client protocol にも接続しない。落ちても chat / settings は生きる。VRM runtime（provisional: 第7.2節）で VRM を動かし、wgpu に載せる。
- **CLI (`apps/ene-ctl`)** は Client channel のみ。control は話さない。製品 idle に常駐させない。

製品起動: `ene-desktop` が Host 未起動なら `ene-core serve` を detach 起動し、その後 control と Client に接続する。Host を GUI の子にしたままにしない。GUI を閉じても Host の serve は続く。

## 3. VRM 表示と text UI の分離

分離は3層ある。混同しない。

1. **Host vs Client（既存）** — マスターデータと Task 継続は Host。画面は投影。
2. **text UI vs VRM overlay（本書の process 分割）** — 同じ first-party 端末の中で、会話/管理の通常ウィンドウと透明アバターを別 process にする。
3. **描画 vs presence（Body は Client ではない）** — overlay が動いていることは「個体がその端末にいる」ことではない。帰属は Host の Client channel が決める。

IO-1（text）と IO-3（Body）と IO-5（管理）は同じ「入出力・提示」でも、単一の障害境界にまとめてはいけない（[client-presence-io-observation](../subsystems/client-presence-io-observation.md)）。acceptance §2.4 / §6、AD-13、SC-09 が「アバター描画が落ちても chat / 設定 / 復旧が使える」を要求する。crate 境界やスレッドでは足りない。

### 3.1 なぜ同一 process では足りない

GPU driver abort、wgpu device lost からの native panic、Wayland / DWM の overlay 不具合、SpringBone や shader のバグは、同じアドレス空間の chat を巻き込む。`catch_unwind` も別スレッドも、process ごと死ぬ abort は止められない。

ウィンドウの種類も違う。

- **text UI**: 不透明な通常ウィンドウ。IME、テキスト選択、リスト、ウィザード、キーボード。
- **VRM overlay**: 透明、常時最前面相当、非矩形 hit-test、透明部分は click-through。Windows は layered/DWM、KDE は layer-shell + input region を **provisional** に置く（第7.3節）。wgpu 直接。text GUI toolkit に overlay をやらせない。

### 3.2 誰が何を持つか

```mermaid
flowchart LR
  host[ene-core]
  gui[ene-desktop]
  body[ene-body]

  host -->|"Client: BodyStateHint, AssetDescriptor"| gui
  gui -->|"projection: show hide box poseHint assetRef"| body
  body -->|"health: alive hung gpuFail"| gui
  gui -->|"AvailabilityFact optional"| host
```

- **`ene-desktop`**: 画面、Client session、control speaker、Body の親。Host から `BodyStateHint` と asset descriptor を受け、投影に落とす。秘密・会話本文・management 決定は Body に渡さない。Body の起動完了を待たずに chat / 設定を出す。
- **`ene-body`**: overlay 窓、VRM runtime、wgpu。hint から expression / SpringBone / LookAt を局所計算する。Host に繋がない。pairing / presence / credential / Task を知らない。crate 名 `ene-vrm` は使わない。
- **`ene-core`**: hint と静的 asset を Client（desktop）へ出すだけ（IPC §19）。Body crash を Companion stop / Task cancel にしない。

### 3.3 不採用

- 同一 process の2ウィンドウ / 描画スレッド — abort が chat を殺す。
- Body を第2の paired Client にする — presence / pairing を描画に混ぜる。overlay が落ちると「端末が居ない」になる。
- Body を Host に入れる — GPU が durable authority を殺す。
- text GUI toolkit に wgpu overlay を合成する — 旧 `ene-stage` の失敗モード。
- iced_layershell で 3D — iced は 2D。Body は wgpu 直接。

## 4. 投影 IPC

Host の Client プロトコルではない。[ene-plugin-ipc](crate-module-decomposition.md) も Client listener も使わない。親 `ene-desktop` が child 起動時に socketpair / 匿名 pipe を渡す。長さ付きの小さなメッセージだけ。

desktop → body:

- `show` / `hide`
- 配置箱（位置・サイズ・HiDPI scale）
- pose hint（idle / listening / speaking / working / attention）。関節角や blendshape は載せない
- asset ref（パスまたは bytes への一時参照。Host 内部 PK ではない）
- shutdown

body → desktop:

- `ready` / `gpu_fail` / `asset_fail`
- health tick（無ければ hung）
- 局所 UI 事実（overlay の drag / resize / hide）。一般設定の候補であり、Host マスターへの直接書き込みではない
- 正常 exit / panic 相当の切断

禁止: API キー、会話本文、pairing token、Task command、management intent、Host アドレス。Body は秘密の保持者にしない（持たせないので消すものがない）。asset 一時キャッシュは desktop 側の通常 erasure がユーザー内容ファイルを消し、body には drop / restart を指示する。秘密寿命を Targeted Deletion に依存させない（第5.2節）。

## 5. Trust / credential / failure

現行実装の穴（design をコードに合わせない）:

1. `ene-core approve-*` は HostLock で serving 中に失敗する。テストだけ in-process `HostHandle` を使う。
2. 秘密は env にしか無く、Host 起動後に GUI から入れられない。
3. IPC §18 は「公式第一者画面」と書くが、判定手段が無かった。同一 UID や control socket を開けたことをその手段にしてはいけない。

### 5.1 二つの channel と Owner 確認

要件「信頼境界」は、API キー登録・ペアリング・バックアップ復元・全データリセットを **手元のホスト PC 画面でユーザー本人が直接確認** することを必須とする。IPC §18 は SameMachine・paired・自己申告・同一マシン上の別 Client では足りず、Computer Use / tool / plugin / LLM が最終確認を代理できないとする。本書はそれを弱めない。

- **Client channel**（既存 unix socket / named pipe、`ene-api`）: pairing, session, chat, filtered management, Task, erasure。`ene-desktop` と `ene-ctl` が話す。high-priv 最終確定はここで成立させない。`confirmed=true` 自己申告は `DeniedByBoundary`。`ManagementIntent` は候補のまま（IPC M-18）。通常の `ene-desktop` Client 経路も最終確認を成立させない。
- **Control channel**（別ソケット。Linux: runtime dir の control socket + peer UID。Windows: より狭い DACL の named pipe + peer token）: Host-local の request、credential 生値の intake、Host が発行した `ConfirmationSession` の完了返送。DTO は `ene-local-control` に置き、`ene-api` に載せない。remote-capable ではない。`ene-ctl` は話さない。`ene-body` は接続しない。`ene-core approve-*` は serving 中この channel を使い、未起動時だけ現行の offline lock を使う。

**control channel は必要だが十分ではない。** peer UID / DACL / 同一 OS ユーザー / data dir を読めること / SameMachine / paired / control socket を開けたことは、**local transport の適格** であり、Owner 本人の確認ではない。単一ユーザー製品であっても、同一 UID の攻撃者・通常の local Client・任意の local process を Owner と同一視しない。コード署名で `ene-desktop` バイナリだけに socket を制限することは Milestone 1 の確認メカニズムにしない（未検証の OS attestation を確定扱いにしない）。代わりに、確認オブジェクトを任意の local process が合成できないようにする。

**Host 所有の `ConfirmationSession`（確定）:**

1. 高権限操作（ペアリング承認、credential 登録・更新・失効、バックアップ復元と復元後の一括有効化、全データリセット、同等の信頼基点変更）は、まず候補として入る。Client intent でも first-party の request でも、この時点では未確定である。
2. Host だけが one-shot の `ConfirmationSession` を発行する。束縛は操作種別・対象 identity・現在の premise generation / revision。Host 生成の nonce と期限を持つ。再利用・転用・別操作への流用はしない。`ene-api` からは受理しない。
3. Host は session を、自分が提示した first-party 確認面（`ene-desktop` の確認 view）にだけ出す。任意 process が作ったダイアログや、Client が捏造した「確認済み」DTO ではない。
4. 完了は、その確認面での **Owner の明示ジェスチャ** と、Host が発行した nonce の control 完了返送が揃ったときに限る。`{ confirmed: true }` のような fire-and-forget は、live session が無くても live session と無関係でも `DeniedByBoundary`。
5. nonce は Client wire、plugin IPC、tool 結果、LLM 出力、Computer Use コマンド列、通常 log / Debug に置かない。

**最終確認を直接成立させてはいけないもの（確定）:**

- 通常の Client（`ene-desktop` の `ene-api` 経路を含む）
- `ene-ctl`
- `ene-body`
- control socket を開けただけの同一 UID process
- tool / plugin / MCP Apps / LLM
- Computer Use およびその代理入力

Computer Use は確認 view の操作対象にしてはならない。それらの channel から届いた完了は、画面をクリックしたと主張しても受理しない。確認面の in-app ボタンを Computer Use が押せることと、Owner 確認が成立することは同一視しない。

**Owner ジェスチャの結び付け:**

- 意味は「Host が作った first-party 確認面での本人操作」であり、「同じ OS ユーザーの何かが yes を書いた」ではない。
- 秘密が OS 保護ストアへ入る操作、および全データリセット / 復元の確定では、OS が user-presence または保護ストア認可 UI を提供するならそれを通す（Windows Credential UI / Hello、Linux Secret Service / KWallet / polkit 等）。in-app の確認表示は影響の説明と session 束縛に使い、OS prompt の代替にはしない。
- OS prompt の具体 API は probe 対象（第7.4節・第9節）であり、未検証のまま「必ず Windows Hello」などと固定しない。prompt が使えない環境でも、Host session + first-party 面 + 上記の非受理リストは落とさない。
- first-party GUI process や OS ユーザー session の完全compromise を、ene がプロトコルで防ぎ切れるとは書かない。それを正当な Owner 確認へ再分類もしない。

### 5.2 Credential の区間と破棄

要件 Credential、S-1〜S-5、AD-14、IB K-C が正本である。生値はプロンプト・UI 表示・履歴・Memory・Task 結果・log・audit・debug・backup に出さない。`SecretValue` は public に返さない。利用は `with_credential` 型のスコープに閉じる。

本番の durable 置き場は **OS 保護ストア**（CM 既存: DPAPI / libsecret / Keychain 抽象。Windows Credential Manager、Linux Secret Service / KWallet）。Rust crate `keyring` はその adapter の **provisional 候補**であり、probe 前の恒久 contract ではない（第7.4節）。現行 `EnvCredentialStore`（起動時に `ENE_OPENAI_API_KEY` を pin）はテスト / dev に残し、serving 中の `put` が必要な製品セットアップの正本にしない。

生値が存在してよい区間と破棄条件:

| 区間 | 場所 | 生値 | 破棄 |
|---|---|---|---|
| C0 | 確認 session のみ。未入力 | 無い | — |
| C1 | first-party 入力 widget の揮発メモリ | Owner が打った直後だけ | control intake 成功直後に zeroize。cancel / timeout / 窓 close でも破棄。crash 経路は best-effort |
| C2 | control の secret-bearing frame（desktop と Host の当該 RPC メモリだけ） | intake 中だけ | Host credential owner が store put に成功または拒否した直後に frame を drop / zeroize。ディスクに書かない |
| C3 | `ene-credential` クレート私有の `SecretValue` | put 直前と `with_credential` 中だけ | OS store put 成功後、および scoped use 終了時。public API から返さない |
| C4 | OS 保護ストア | at rest の正本 | 明示の失効・差し替え、または全データリセット。backup には入れない。restore は現在ストアを巻き戻さない（S-5） |
| C5 | 認証用途の scoped use（provider adapter 等） | クロージャ内の一時 | クロージャ終了。Task 結果やツール引数へ clone しない |

**禁止（どの区間でも）:**

- `ene-api` Client payload、Body 投影、通常の management DTO
- `app.db` / `ene-store`、通常の GUI 永続 state、IME の確定を chat へ送ること、undo 永続化
- 通常 DTO の `Debug` / `Display` / serde エラー、tracing、audit、debug dump
- バックアップ、crash report の自動外部送信
- Targeted Deletion の検索コーパス・検索トークン・削除対象本文。削除 sweep は秘密デストラクタではない。登録キーを「ユーザー内容」として ingest しない
- widget メモリを `ErasureParticipant` に見立てて秘密寿命を削除協調へ預けること。widget は C1 の自己破棄義務を持つ。ユーザーが chat に貼ったキー類似文字列の除去は既存 scrub（S-3）であり、OS store の破棄ではない

`ene-local-control` の秘密フィールドは redacted 型にする。通常 DTO の Debug 実装・log マクロ・永続化に秘密寿命を依存させない。Control 完了や拒否の監査は `CredentialRef` / session id / outcome だけを残し、生値を残さない。

Setup readiness は wizard boolean ではなく、credential usable + assignment / consent の既存 durable fact から導出する。登録だけでは provider 0 呼出し。

### 5.3 寿命と失敗（混同しない）

- GUI を開く ≠ Body が ready。chat は Body なしで動く。
- Body を hide する ≠ Companion Stop。個体は Host 上で Running のまま。
- Body が exit / crash する ≠ Task cancel。Host は Client 切断とも見なさない（desktop は生きている）。
- Body を restart する ≠ domain replay。同じ hint / asset を再投影するだけ。Host に command を再送しない。
- GUI を閉じる → child Body は落とす。Host は serve 継続。
- Host を止める → desktop は切断を表示する。Body は最後の hint のまま動かすか hide する。どちらも「個体が別端末に移った」ことにはしない。

高負荷や fullscreen は desktop が検知して body に quality down / pause を出す。chat の入力経路は止めない。優先順位は第8節。

## 6. Application / crate / module

| 名前 | 役割 | いつ作るか |
|---|---|---|
| `apps/ene-core` | Host composition。control listener と `ConfirmationSession` の minter を serving 中に持つ | 既存。Stage 7 A1 で control を足す |
| `apps/ene-ctl` | CLI Client。Client channel のみ | 既存。control は話さない |
| `apps/ene-desktop` | 製品 GUI と first-party 確認面 | A1（接続）/ B（画面） |
| `apps/ene-body` | VRM overlay | D。compile 隔離のため別 package |
| `crates/ene-client` | Host Client IPC（handshake, correlation, device identity, erasure participant） | A1 で `ene-ctl` から抽出。GUI は `ene-ctl` に依存しない |
| `crates/ene-local-control` | Host-local control DTO。`ene-api` に載せない。秘密フィールドは redacted。確認完了は session 束縛 | A1 と同時の最小 crate |

作らない: `ene-stage`, `ene-stage-ui`, `ene-vrm`, `ene-tray-linux`。トレイは Milestone 1 に無い。`ene-character` / `ene-plugin-host` も GUI のために先行 scaffold しない。同梱 `ene` の VRM は install asset とし、Host が W-7 descriptor を出し、GUI が Body へパス/バイトだけ渡す。

生成 GUI 束縛が workspace clippy（`-D warnings`）と衝突する場合だけ、生成物専用の compile-isolation モジュール/crate を `ene-desktop` の隣に置いてよい。名前は `ene-stage-ui` に戻さない。ドメイン判断・IPC・秘密は入れない。

`ene-desktop` 内 module: `ui`、`session`（`ene-client`）、`control`、`body_supervise`、`i18n`、`erasure`。`ene-body` 内: `window`（OS overlay）、`vrm`、`render`、`ipc`。

Host ドメイン分解（companion / task / …）は CM を維持する。

## 7. 依存 — 確定する不採用と provisional 候補

process 分割・信頼境界・性能ゲートは確定である。**新しい GUI / VRM / overlay / keyring crate を、実機 probe 前に恒久 design contract として固定しない。** 以下は preferred candidate である。probe が通った時点で「採用」へ昇格し、失敗したら第7節の fallback / 再選定だけを動かす。要件は toolkit 名を固定していない。

### 7.1 Text GUI: Slint（provisional preferred）

候補: winit backend、通常ウィンドウのみ。Royalty-free Desktop。About から辿れる画面に `AboutSlint`（または同等 attribution）を置く。GPL にはしない。旧 `.slint` / `ene-stage-ui` はコピーしない。Qt backend は入れない。overlay / layer-shell / 透明ヒットテストは Slint にやらせない。

候補にする理由（probe で覆り得る）:

- 宣言的レイアウト、テーマ、複数画面、リスト/フォーム、キーボード導線を、imgui 風の見た目に落とさずに組める。
- Body を別 process にしたので、同一 process 合成はしない。
- Windows 11 と Linux を一つの UI 記述で賄える見込み。GTK のような OS 二重 UI にしない。
- IME は winit の composition イベント（Wayland `zwp_text_input_v3` / Windows TSF）に乗れる見込み。未確定入力は commit まで送らない。これは probe 項目である。

確定する不採用:

- **egui / eframe**: 即時モードのツール UI 向きで、会話タイムライン・セットアップウィザード・日英の製品画面としての自由度が足りない。
- **GTK / relm4**: KDE は強いが Windows 11 が第二級。
- **Tauri / webview**: CSS 級の自由度と IME は強い。秘密入力面・二言語 runtime・常駐重量が Stage 7 と 2 GiB gate に対して過大。

**iced** は MIT でアプリらしいが、レイアウト自由度は Slint より低い。**Slint の IME / Wayland / Windows 技術成立 probe が失敗したときの fallback 候補**であり、既定にも、probe 前の第二の確定採用にもしない。iced を使う判断が必要になった時点で、その IME / windowing も probe する。

### 7.2 VRM: `vrm-runtime`（provisional preferred）

renderer-agnostic な VRM 1.0 loader / runtime（expression・humanoid・LookAt・SpringBone・VRMA・MToon 評価）を使う案。0.1 でも手書き extras より小さい見込み。待機/発話の仕草に SpringBone は要るので、ランタイム選定の理由で SpringBone を落とさない。crate 名 `ene-vrm` は復活させない。

`ene-body` は loader/runtime から GPU 向け view を受けて wgpu に載せる。MToon の GPU 実装まで runtime crate は肩代わりしない。shader が未完の間は spec どおり unlit/PBR fallback を許す。0.1 の API 変動を恒久 contract にしない。lock へ入れるのは probe 後の採用決定のとき。

確定する不採用:

- **gltf + 手書き `VRMC_vrm` extras**: loader は小さくなるが SpringBone / expression / LookAt を自前で持つ。実装量が増える。`vrm-runtime` probe が失敗したあとの再選定であり、最初から自前 extras を正本にしない。
- **Bevy / bevy_vrm**: compile / idle CPU / 2 GiB 常駐に対して過大。
- **winit AlwaysOnTop on Wayland**: プロトコル上存在せず、GNOME/KDE で失敗する既知制約。

### 7.3 Overlay windowing と描画

描画 API として Body だけが `wgpu` を持つことは確定（Host / desktop に GPU device を置かない）。overlay の OS 手段は provisional:

- Windows: `winit` + layered/DWM 透明と非矩形 hit-test。
- KDE Wayland: `wayland-client` + `zwlr_layer_shell_v1` + ARGB + input region。

XWayland 成功を KDE Wayland 証拠にしない。desktop は 2D toolkit の通常ウィンドウだけを持ち、wgpu overlay を持たない。

### 7.4 Credential store adapter: `keyring`（provisional preferred）

OS 保護ストア抽象（DPAPI / libsecret / Keychain）は既存の確定契約である。`keyring` crate はそれを呼ぶ adapter 候補。Windows Credential Manager と Linux Secret Service / KWallet での put / get / delete、および OS 認可 UI が C4 の正本になることを probe する。失敗したら同じ抽象のまま別 adapter を選ぶ。テスト / dev の env pin は残してよい。

## 8. パフォーマンス

閾値の正本は [acceptance の Performance Gates](../../requirements/acceptance.md#性能基準-performance-gates)。本書は数値を下げず、分母と比較不能な％を同じ合格にしてはいけない、という測り方を固定する。この文書の時点では未測定であり、合格したとは書かない。技術成立 probe の成功を性能合格の代替にしない。

### 8.1 何が gate か

通常 avatar 表示中・推論なしの待機が基準。Body 非表示や renderer 停止は補助測定で、それで idle 合格を代替しない。Voice 等の未提供操作は対象外と明記する。headless CI の成功は実 GPU / compositor 性能の合格ではない。

- **idle CPU**: セットアップ後、推論なし 5 分。起動している全 ene process（`ene-core` + `ene-desktop` + `ene-body`。動かしているなら `ene-ctl` も）の CPU time を合計する。Body を除外しない。
- **resident memory**: 同じ区間の Host + 全 Client process 合計 ≤ 2 GiB。Body を除外しない。
- **avatar**: 同梱 `ene` の通常表示で平均 ≥ 30 FPS。他のデスクトップ操作を 1 秒以上 block しない。
- **操作受付**: cancel 等は入力から 1 秒以内に受付状態を出す。ローカルの「送信待ち」を Host 受付済みと偽らない。受付と完了は別。

### 8.2 分母

Windows と Linux で Task Manager / top の％は同じ意味ではない。生値を残し、合格判定の式を一つにする。

- **CPU 生値**: 各 PID の user+system time（Linux: `/proc/<pid>/stat` の utime+stime。Windows: `GetProcessTimes`）。5 分の wall と online logical CPU 数も記録する。
- **合格に使う％**: `100 * Σ cpu_seconds / (elapsed_wall * logical_cpus) ≤ 10`。マシン全体に対する使用率。
- **busy-wait 検出（追加。gate を弱めない）**: 同時に 1-core equivalent `Σ cpu_seconds / elapsed` を記録する。idle 中にどれか 1 process がほぼ 1 コアを埋めている（目安: その process の 1-core equivalent ≥ 0.5）なら、全体％が 10 未満でも不合格。16 コア機で 1 スレッド spin が 6％に見える穴を塞ぐ。
- **RSS**: Linux は各 PID の `VmRSS` 合計。Windows は Working Set 合計。ピークと区間平均を残す。GPU 専用 VRAM は診断として別記録し、RSS の代わりにしない。debug / sanitizer は対象外。release、exact SHA、build flags を添える。
- **FPS**: Body が実際に present した時刻（Windows: DXGI present。Wayland: frame callback）。要求した redraw 回数を FPS と数えない。warmup（初回 shader / pipeline）の後の連続区間。
- **desktop block**: overlay の透明領域が click-through であること、および描画 hitch が 1 秒以上ポインタ/キーボードを奪わないこと。合成器側の証拠（入力が下の窓へ届く）を残す。
- **操作受付**: first-party GUI が「受付」を描いた時刻。Body frame や provider 応答を待たない。

記録する環境: OS、nixpkgs revision または Windows build、KDE session、CPU/GPU/driver、解像度と scale、UI 言語、asset、描画 backend、warmup と測定区間。Linux 測定が 26.11 公式 acceptance でない場合は、その旨を記録する（第9節）。

### 8.3 3 process でも 2 GiB / 10％に入れる

分割は RSS を増やす（Rust runtime が3つ）。その代わり GPU / wgpu は `ene-body` だけ、Host は vsync wait に入らない、高負荷時に Body だけ落とせる。

常駐を膨らませない選択が Bevy 不採用と Tauri/webview 不採用である。同梱 `ene` は一度ロードする。Host は VRM バイトのマスターを Client に二重キャッシュし続けない（IPC §19 の transient cache。revision で捨てる）。`ene-ctl` を製品 idle に常駐させない。

### 8.4 idle CPU と 30 FPS

危ないのは、何も起きていないのに 60/120 Hz で SpringBone + present し続けること。

- Body の目標は平均 30 FPS（gate が 30。60 を追わない）。present 待ち（vsync / frame callback）。busy present しない。
- pose hint は状態変化のときだけ。関節角を IPC しない。SpringBone / LookAt / expression は body 局所、30 Hz。
- hide / fullscreen / 高負荷 pause では present を止める。GPU を回したまま透明描画しない。
- desktop の GUI はイベント駆動。Host I/O・provider wait・body IPC で event loop を塞がない。
- Host は描画 tick を持たない。health tick は数 Hz 以下。

SpringBone は待機の仕草に要るので idle でランタイムごと外さない。足りなければイテレーション回数やコライダ精度を先に落とす。MToon の重い GPU が未完なら unlit/PBR fallback を許す。見た目の品質であり、SpringBone を理由に 30 FPS を捨てない。初回 PSO は測定前 warmup。GUI の redraw と Body の present は同じループにしない。

透明 overlay の入力領域を実シルエットに近づけ、デスクトップ全体を hit-test しない。これが「他の操作を邪魔しない」の本体である。

### 8.5 操作の 1 秒と縮退順（AD-13）

cancel / 一時停止等は `ene-desktop` の経路。Body の frame 完了も GPU reset も待たない。「受付」は Host が command を intake した事実の投影。overlay の drag / resize は body 局所。毎ピクセル Host に投げない。離したあと一般設定候補として desktop が Host に出す。

守るもの（先）: 入力受付、cancel、設定/復旧、Host-only Task、chat 本文の送受信。

落とすもの（先）: Body の解像度 / MSAA / SpringBone 精度 → 15 FPS → present pause / hide。fullscreen 中は Body を休止（要件どおり）。Host を落とさない。Body を殺しても chat は生きる。

## 9. Probe と最終 acceptance の分離

NixOS 26.11 は acceptance の Support Matrix 上の Linux 対象である。この文書の時点では正式リリース前であり、公式 26.11 desktop が無いことを理由に Stage 7 全体を止めてはいけない。技術成立 probe と最終 acceptance を混ぜない。

この Cloud Agent 環境は Windows 11 desktop も KDE Wayland session も持たない。以下は **未実施** と明示する。合格したとは書かない。

### 9.1 今実施する技術成立 probe

目的: 第7節の provisional 候補を採用へ昇格できるか、または fallback 再選定が要るかを決める。性能ゲート合格の代替ではない。

Linux は **今使える KDE Wayland** でよい。実際の distro / nixpkgs revision / Plasma / compositor を記録する。これは NixOS 26.11 公式 acceptance の代用ではない。Windows は Windows 11 x86-64 が用意できたとき。

- text GUI toolkit（既定候補 Slint）: 日本語 IME composition（未確定を送らない、候補ウィンドウ位置）、HiDPI、focus、日英切り替え。
- Body 透明ウィンドウ、drag / resize、hide / restore。
- HiDPI / scaling、focus / pointer（Body 側）。
- VRM runtime 候補で VRM 1.0 を読み、透明描画、idle / speaking の expression、SpringBone が動くこと。
- Wayland: layer-shell + input region の click-through。XWayland 成功を KDE Wayland 証拠にしない。
- Windows: layered/DWM 透明と非矩形 hit-test。
- Body crash / hang / GPU init failure のあと chat / cancel / settings が生きる。
- OS 保護ストア adapter: put / get / delete と、提供されるなら認可 UI。
- 第8節の測り方そのもの（5分 idle の全 PID CPU time、logical CPU 分母、1-core equivalent、VmRSS / Working Set 合計、present 時刻、click-through）。release SHA と session/GPU を記録する。数値ゲートの合否は slice F。

**slice との関係:**

- **A1**（`ene-client`、control listener、`ConfirmationSession`、serving 中 approve / credential put の Host 側）は GUI / overlay probe を待たない。Stage 6 完了後の統合 base に積んでよい。
- **B** の production GUI は、text GUI toolkit の技術成立 probe の後。
- **D** の production overlay は、overlay + VRM runtime の技術成立 probe の後。
- **C** の管理画面は B の GUI 面の後。overlay は待たない。

### 9.2 後日の最終 acceptance（slice F）

対象は acceptance Support Matrix: Windows 11 x86-64 と **正式リリースされた NixOS 26.11** x86-64 KDE Wayland。日本語と英語。性能ゲートを含む。

- 今の KDE Wayland probe 成功 ≠ 26.11 acceptance。
- headless CI 成功 ≠ desktop 合格。
- XWayland 成功 ≠ KDE Wayland 合格。
- 26.11 が F 開始時点で未リリースなら、Linux 最終 acceptance は open のまま残す。未リリース OS 上の測定を 26.11 合格と書かない。その間も A1 以降の Host / GUI 作業は止めてはいけない。

## 10. 残した Design Freedom

- control / 投影 IPC の具体的なメッセージ識別子とフレーム長上限（契約は「小さい」「秘密を載せない」「確認完了は session 束縛」まで）。
- `ConfirmationSession` の nonce 長・期限秒・確認面の文言。OS user-presence API の具体（第5.1節の義務は Freedom ではない）。
- Body の camera / lighting、MToon をいつ完成させるか。
- 複数 Companion を同じ Client に出すときの renderer 数。
- Slint から iced への切替は第9.1節の IME / windowing probe が失敗したときに限る。別 toolkit を probe 無しで正本にしない。

プロセスを GUI と Body で分けること、Host に GPU を入れないこと、control を `ene-api` に載せないこと、同一 UID を Owner 確認にしないこと、credential 寿命を通常 DTO / log / Targeted Deletion に依存させないことは Freedom ではない。

## 11. Traceability

| 判断 | 根拠 |
|---|---|
| Host に GUI/wgpu を置かない | RT-01、SC-09、AD-01 |
| text と Body を別 process | IO-3 / IO-7、acceptance §2.4 / §6、AD-13、SC-09 |
| Body は Client ではない | RT-02、IPC §19、X-C |
| high-priv は Host 発行 session + Owner ジェスチャ。同一 UID や control socket 開封は十分ではない | 要件「信頼境界」、IPC §18、IB 第9節 |
| credential 生値の区間 C1–C5 と破棄。DTO / Debug / log / DB / Targeted Deletion に寿命を依存させない | 要件 Credential、S-1〜S-5、AD-14、IB K-C |
| 性能の分母と全 process 計上 | acceptance Performance Gates、#1636 §6 |
| Slint / `vrm-runtime` / `keyring` / layer-shell は provisional | 第7節。要件に toolkit 名は無い。probe 後に採用へ昇格 |
| 技術成立 probe と 26.11 最終 acceptance を分離 | acceptance Support Matrix。未リリース OS で Stage 7 全体を止めない |
