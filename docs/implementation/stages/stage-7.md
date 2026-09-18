# Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 7 の**実装範囲、PR 分割、依存関係、検証 gate**を管理します。製品 behavior は [requirements](../../requirements/README.md)、受け入れ範囲は [acceptance](../../requirements/acceptance.md)、authority と状態の意味は [design](../../design/README.md) を優先します。計画の存在は着手・完了を意味しません。

Stage 7 に向けて過去の設計文書に置かれた application / crate / module の名称、分割例、UI・windowing・rendering の技術候補は、要件そのものではありません。A0 では、それらを維持することを目的にせず、現在の要件と上位設計から必要な責務・障害境界・依存方向を再導出します。既存案が最小で適切なら維持し、不必要な分割・名前・依存・process は削り、より単純な構成が適切なら対応する design を実装前に更新します。

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

## 2. 固定する契約と、再評価する実装構造

Stage 7 は Stage 0〜6 の domain authority と安全性契約を再設計しません。一方で、それを GUI へ接続するための**具体的な名前・crate 数・application 数・process 数・library 選択は A0 の結論が出るまで固定しません**。

### 固定するもの

- Host が durable domain state と最終 authority を持ち、Client の表示状態を master にしない。
- connection / incarnation / presence / presentation を同一視しない。
- credential raw value を通常の Host↔Client payload、DB、ログ、エラーへ流さない。
- high-privilege の最終確認を remote input、LLM、tool、plugin が代理しない。
- UI / renderer の障害で Host-only Task や text / management の利用を壊さない。
- usage / cost / cap、Targeted Deletion、Task recovery 等は既存 semantic owner の query / command / currentness を使う。
- GUI 専用の authoritative DB、permission registry、presence registry、accounting registry を作らない。
- acceptance と performance gate を実装都合で弱めない。

### A0 で再評価するもの

- 製品版 Client application の名称と数。
- library crate / adapter crate / module の境界と名称。新しい crate を作らない選択肢も含む。
- text GUI と avatar renderer を同一 process にするか分離するか、その寿命と supervision。
- UI toolkit、windowing、graphics、VRM load / animation、renderer-local IPC 等の依存ライブラリ。
- current CLI の transport / auth / request-correlation 実装を共有・移動・再構成するか。
- design の target workspace tree にある未実装 component 名が今も適切か。
- platform-specific adapter を共通化する範囲と、Windows / KDE Wayland で分ける範囲。

既存の source は動作済みの制約と回帰を理解するために読みますが、既存の名前やファイル境界を新構成へ保存すること自体は目的にしません。逆に、意味の変わらない既存実装を理由なく書き直すことも目的にしません。変更は、責務、依存、障害分離、検証可能性、保守性、build / runtime cost の改善で説明できる場合だけ行います。

## 3. PR 分割と各 slice の gate

各 ID は実装作業の参照名であり、新しい要件 ID ではありません。A0 で具体的な構成を確定した後は、その構成に合う名前で実装 PR を切ります。この文書の slice 名を Rust の crate / module / binary 名へ機械的に写しません。

### A0: Stage 7 実装アーキテクチャの再導出

**目的**: requirements と上位 design から Stage 7 に必要な責務を再導出し、既存の命名・分割・技術候補を含めて比較し直します。最終成果物は production code ではなく、選定根拠を反映した design docs と、技術的な不確実性を潰す小さな probe / 検証記録です。

#### A0-1: 必要な runtime / ownership boundary

最初に framework や crate 名を選ばず、次を満たす最小構成を導出します。

- fresh install から trusted first-party setup を開始できる。
- text conversation / management は avatar rendering 失敗から独立して使える。
- Host の domain authority を Client へ移さない。
- credential の raw value を protected boundary 外へ不要に露出しない。
- GUI event loop を Host I/O、provider wait、renderer I/O で block しない。
- Client が Targeted Deletion の local participant として、自身が実際に保持する一時本文を消去できる。
- process / component の再起動で古い command、presentation、conversation、Task effect を replay しない。

ここから必要な executable / process / library boundary を決めます。process を増やすことも減らすことも目的にしません。

#### A0-2: 命名と code ownership

採用する各 application / crate / module について、名前を先に決めず、以下を説明してから命名します。

1. 何を owner とするか、または owner を持たない adapter / composition なのか。
2. 寿命は何に結び付くか。
3. どの依存方向を許可するか。
4. 何が壊れても巻き込んではならないか。
5. public boundary を独立させる価値があるか。
6. 単一 module のままでは不足する理由があるか。

既存 design の未実装 crate 名は候補であり、名前を維持するためだけに crate を作りません。逆に、単なるファイル整理を理由に新しい crate を増やしません。A0 の結論が current crate decomposition と異なる場合は、production code より先に design を修正します。

#### A0-3: dependency / framework 選定

UI、window、graphics、VRM、animation、process-local communication など、Stage 7 で新たに必要になる外部依存は白紙から比較します。現在または過去の実装で使われていた library を既定値にしません。

最低限、各候補について次を比較します。

- Milestone 1 の機能充足。特に transparent desktop body、drag / resize / hide / restore、focus、IME、HiDPI。
- Windows 11 と KDE Wayland での実装可能性。API の存在ではなく、対象 backend 上の実挙動。
- Rust integration と unsafe / native dependency の隔離。
- binary size、compile cost、idle CPU / memory、rendering overhead。
- crash / hang isolation を妨げないこと。
- maintenance 状態、release cadence、platform support、license / distribution 条件。
- testability と headless test / real-desktop test の分離。
- 必要な機能だけを使えるか。巨大な framework を convenience のためだけに導入しないこと。
- 独自実装を選ぶ場合、その保守コストが dependency 導入より本当に小さいか。

候補を最低数揃えること自体は目的にしません。明らかに不適格な候補を数合わせで比較せず、必要なら現在の公式資料を調査して、最も単純で要件を満たす構成を選びます。選定結果には、採用理由だけでなく有力な不採用案と不採用理由を残します。

#### A0-4: trusted Host-local setup / management

未ペアリングの fresh install から、Host が管理する第一者 UI で setup / high-privilege confirmation / credential registration を行う境界を具体化します。

- SameMachine、同一 UID、paired、Client の自己申告だけを trust の根拠にしない。
- arbitrary local process が trusted surface を偽装できない。
- remote Client、LLM、Computer Use、tool、plugin が final confirmation を代理できない。
- raw credential は Owner input から credential owner の protected store までの必要最小区間だけに存在する。
- setup wizard の page や boolean を domain authority にせず、各 owner の durable fact から setup readiness を導出する。
- credential 登録だけでは provider call を開始せず、assignment / consent 等の既存前提を満たして初めて送信可能にする。

既存の local control mechanism を再利用できるかは調査しますが、用途の異なる endpoint を名前の近さだけで万能 management channel に拡張しません。

#### A0-5: avatar / text failure boundary

acceptance が要求する「avatar が停止しても chat / settings が使える」を、実際の crash / hang / GPU initialization failure まで含めて満たす境界を決めます。

- text / management 側が renderer の起動完了や返答を同期的に待たない。
- renderer に conversation、Memory、Task body、credential、management authority を不要に渡さない。
- renderer へは表示に必要な projection だけを渡す。
- 頻繁な ephemeral update は bounded に扱い、古い update を捨てられるようにする。
- renderer restart を domain replay の契機にしない。
- avatar hide / renderer exit を Companion stop や Task cancel と同一視しない。
- process 分離を採用しない場合でも、acceptance が要求する故障分離をどう証明するか明示する。

#### A0-6: 実機 probe と測定前提

production 実装前に、採用候補で不確実な platform behavior を最小 probe で確認します。

対象:

- Windows 11 x86-64
- NixOS 26.11 x86-64 / KDE Wayland

最低限確認するもの:

- transparent window
- drag / resize
- hide / restore
- focus / pointer / keyboard
- 日本語 IME composition
- HiDPI / scaling
- VRM 1.0 load と transparent rendering
- idle / speaking の最小 animation
- renderer crash / hang / GPU initialization failure
- renderer 不在時の chat / management / Task cancel / recovery

Linux は実際に使用した session / backend を記録します。別 backend や別 session の成功を KDE Wayland の証拠にしません。library の API documentation だけで実機対応済みとは判定しません。

**A0 gate**:

- Stage 7 の application / process / crate / module boundary と命名が、要件から説明できる。
- 新規外部依存の選定理由と主要な不採用案が記録されている。
- trust / secret / renderer failure boundary が design に反映されている。
- 対象 OS で、production 実装を左右する platform uncertainty の probe 結果がある。
- current design と結論が異なる箇所は design が先に更新されている。
- 未決事項を「実装しながら決める」として依存 slice へ押し出していない。

### A1: Client 接続と event delivery の最小共有境界

A0 で確定した構成に従い、GUI が必要とする Host 接続、認証、request / response correlation、unsolicited fact、Client local erasure を実装します。

current CLI の実装は重要な回帰資料ですが、「CLI と GUI で共有 library を作る」ことを先に結論にしません。既存コードの抽出、内部 module の移動、adapter の分離、既存実装の保持のいずれが最小かを A0 の責務境界に照らして選びます。

- GUI event loop を接続待ち・provider 待ちで塞がない。
- connection / incarnation / presence を混同しない。
- 古い connection の入力・receipt・cursor・選択状態を継承しない。
- 再接続で未送信本文や mutation command を自動 replay しない。
- bounded queue と cancellation を持ち、遅い Client が Host-only Task を止めない。

**gate**: 実 socket / named pipe で既存 Client 回帰が維持され、GUI 側でも connection replacement、response correlation、slow consumer、待機中の deletion demand を検証できること。

### B: 初回セットアップからテキスト会話までの縦断 GUI

A0 で決めた first-party Client 構成で、avatar に依存せず最初の製品経路を通します。

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

**範囲**: A0 で選定した runtime / dependency / asset 境界を使い、同梱 `ene` を表示します。まず透明表示・移動・resize・非表示/再表示を通し、次に既存の応答状態に従う待機/発話中の表情・仕草を接続します。新しい感情推論や Voice pipeline は作りません。

renderer に渡す情報は必要なアセット参照と表示指示に限定し、会話本文、Memory、API key、management authority を渡しません。avatar の非表示・終了を Companion 停止や Task cancel と同一視せず、renderer 再起動で古い会話/操作を replay しません。

**gate**: acceptance §2 を両実 desktop で通すこと。renderer の異常終了・hang・初期化失敗を注入しても、text 入力、Task cancel、設定/復旧が利用できることを確認します。アバターの矩形や透明部分がデスクトップ入力を不必要に奪わず、操作に応じて移動・resize できることを実測します。別の Client surface が生存しているだけでは GUI fallback の合格にしません。

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
A0 architecture / technology decision
        ├→ A1 → B → C1 ─┐
        │          ├→ C2 ─┼→ E → F
        │          └→ C3 ─┤
        └────────────→ D ──┘
```

A0 の調査中でも、既存 contract の回帰 test と acceptance / performance harness の準備は並行できます。ただし、新しい public boundary、crate、binary、framework を A0 の結論より先に既成事実として追加しません。

C1 / C2 / C3 は異なる owner と画面に分け、共通 Client boundary・DTO・schema が確定した範囲だけ並列化します。同じ transport loop、Host-local trust boundary、schema version、共有 view を別 branch で同時に再設計しません。共通 prerequisite は先に取り込み、stack の base / head と担当範囲は PR 本文で管理します。

## 5. acceptance と証拠の対応

| acceptance | 主担当 | Stage 7 で追加する first-party 証拠 |
|---|---|---|
| §1: セットアップと最初の会話 | A0 / B | 新規環境から GUI だけで同意・登録・割り当て・会話。登録のみでは送信ゼロ。日英切り替えと再起動後の履歴 |
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
| idle CPU | セットアップ後・推論なしの連続5分間で、全 ene process の平均 CPU 使用率が10%以下。Host、Client、renderer 等の実際に起動した全構成要素を計上し、process CPU time の生値と使用率の分母を記録する |
| resident memory | 同じ待機区間で Host と全 Client process の合計が2 GiB以下。OS ごとの resident 指標、集計方法、時系列と最大値を記録し、描画 process を分離した場合も除外しない |
| avatar | 同梱 `ene` の通常表示で平均30 FPS以上。他のデスクトップ操作を1秒以上 block しない。実際の frame 提示時刻と操作遅延を測り、要求した redraw 回数を FPS と数えない |
| 操作受付 | Milestone 1 に含まれる cancel 等は入力から1秒以内に受付状態を表示する。ローカルの「送信待ち」を Host 受付済みと偽らず、受付と完了を区別する |

A0 で各 OS の測定指標・CPU の正規化方法をレビューして固定し、比較不能な数値を同じ百分率として扱いません。release build の exact SHA、build flags、OS / nixpkgs revision、KDE/session、CPU/GPU/driver、解像度・scale、UI言語、asset と描画 backend、warm-up と測定区間を結果に添えます。NixOS の対象版が未確定の時点の probe は暫定値とし、最終検証環境を無断で置き換えません。

通常 avatar 表示中の idle を基準にし、非表示・renderer 停止時は補助測定として分けます。Voice 等で今回提供しない操作例は対象外の理由を明記し、未実装の必須機能を skip して合格にはしません。headless CI の成功は実 GPU / compositor / IME / desktop 性能の合格と別です。

## 7. docs と closeout の運用

| 場所 | 更新する内容 |
|---|---|
| この文書 | Stage 7 の範囲、slice、依存、検証対応。具体的な crate / binary 名や dependency 選定は A0 の結論を design に反映した後にのみ確定する |
| `docs/design/` の対応箇所 | A0 で再導出した application / crate / process / dependency boundary と、恒久的な trust / IPC / ownership / failure 契約。current design と異なるなら production code より先に更新する |
| `docs/requirements/acceptance.md` | 原則変更しない。実装都合で scenario・OS・性能 gate を削らず、要件変更が必要なら別の明示的な判断にする |
| Issues / PRs、実行可能 test / harness | 個別不足、比較した選択肢、選定根拠、実装範囲、exact tip、再現手順、テスト対応、実機の測定結果と証拠。結果は pass / fail / 未実施を区別する |
| `PROGRESS.md` | current / completed / blocker / next の短い index のみ。Stage 6 完了前に Stage 7 を current / completed とせず、Stage 7 内部の全 slice を転載しない |

最初の作業は A0 の再設計です。既存の target workspace tree や過去 UI 実装から crate 名・framework・process topology を写して production code を始めません。A0 の結論と必要な実機 probe が揃った後に A1 / B / D を開始します。

Stage 7 closeout では共通 quality gate に加え、§5 の全対象と§6 の実測、GUI の local erasure、text fallback、後続 Stage が利用する query / command の配線を確認します。Stage 6 を含む既存回帰を維持し、未解決の acceptance blocker がないことを確認してから、Stage 7 を completed index に1行追加し、Stage 8 の詳細計画へ進みます。
