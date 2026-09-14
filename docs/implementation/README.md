# ene 実装ガイド

ステータス: **開発実装用ガイド**

このドキュメントは、ene を開発・実装するにあたって、**「どの順番で何を作り、並列で何を進められ、どこまでできたら完了とするか」** を分かりやすくまとめた開発ガイドです。

- **仕様・要件**: [`docs/requirements/`](../requirements/README.md) を参照してください。
- **アーキテクチャ・内部設計**: [`docs/design/`](../design/README.md) を参照してください。
- **開発用コマンド・Rust規約**: [`AGENTS.md`](../../AGENTS.md) を参照してください。

> [!IMPORTANT]
> 要件や設計とこのガイドの内容に矛盾が生じた場合は、常に要件や設計を優先します。実装中に設計上の矛盾や疑問が見つかった場合は、その場しのぎのコードで解決せず、一旦作業を止めて GitHub Issue や設計修正 PR で先に方針を合意してください。

---

## 1. 開発の進め方

### 1.1 最初から最後まで一貫して動く「縦断スライス」を優先する
モジュールやクレートを上から順番にバラバラに作り込むのではなく、**「ユーザーから見て最初から最後まで通しで動く最小限の流れ（縦断スライス）」** を最優先で組み立てます。

例えばテキスト会話の実装では、UI、データベース、AI 連携を別々に長期間開発することを避け、まずは簡易的でもよいので以下の最小ルートを一気通貫で動かします：

```text
画面入力 → ホスト受付 → パートナー判断 → 権限確認 → AI推論 → 画面返信 → 履歴保存
```

この動く土台ができてから、記憶機能や複雑なタスク処理、エラー復旧などを少しずつ追加していきます。将来必要そうだからという理由だけで、使われない共通クラスや管理マネージャーを先行して作ることは避けてください。

### 1.2 PR（プルリクエスト）の分割方針
実装は原則として小さく積み重ねる **Stacked PR（段階的なPR）** で進めます。1つの PR に巨大な変更を詰め込まず、**レビュアーが意図を無理なく理解でき、変更前後の挙動をテストできる粒度** に分割します。

- **依存関係のある変更**: 前の PR のブランチの上に重ねて積み、下から順にマージします。各 PR は単体でビルドおよびテストが通る状態を保ちます。
- **独立した変更**: 互いに依存しない機能は、同じ安定したブランチから並列して進めて構いません。
- **設計変更**: コードの実装 PR と混ぜず、先にドキュメント修正の PR を分けて出します。
- **PR の説明文**: 何を動かすための PR か、対応する設計・要件、意図的に含めていない範囲、実施したテスト内容を明記します。

### 1.3 後方互換性は一切考慮不要
ene の開発では、`.old/` に限らず、いかなる後方互換性も考慮する必要はありません。旧バージョンとの互換レイヤー、マイグレーション用のシム（shim）、過渡的な回避策などは一切不要です。過去の仕様やデータ構造に縛られず、要件と設計に最も合致する明快で堅牢な実装を優先してください。

---

## 2. 実装ステージと推奨順序

以下のステージ番号は依存関係に基づく推奨順序です。並列化条件を満たすものは同時に進めて構いません。

### Stage 0: リポジトリとビルド基盤の構築
新しい実装コードを置くための、最小限の土台を整えます。
- **主な作業**: ルートの Cargo ワークスペース設定、Rust ツールチェーン・フォーマッタ・Linter（clippy）の設定、最小限の CI、Linux/Windows ビルド環境の整備。
- **完了基準**: 空または最小構成のワークスペースでビルド、テスト、Lint がエラーなく実行できること。

### Stage 1: 最小限の共通規約と基盤の作成
各機能が共通して使う最小限の型や設定を定義します。
- **主な作業**: 基本型（`ene-primitive`）、設定管理（`ene-config`）、共通 API 定義（`ene-api`）、ホストおよびクライアントアプリの最小エントリポイント。
- **完了基準**: Stage 2 の最初の機能実装が、ここで定めた基本型と設定を前提としてスムーズに開始できること。

### Stage 2: セットアップとテキスト会話の最小開通
初期設定から一対一のテキスト会話が実際に画面を通して動く状態を作ります。
1. ホストとクライアントの接続・最小ハンドシェイク
2. セットアップ画面（言語選択、API キー登録）
3. 権限・認証情報・推論の最小境界の整備
4. OpenAI Responses API を使った推論パイプライン
5. ユーザーからのメッセージ入力と、AI からのストリーミング応答表示
6. 会話履歴の安全なデータベース保存と、再起動後のタイムライン復元
7. 一連の流れを通す結合テスト（Integration Test）
- **完了基準**: 画面からメッセージを送信し、パートナーからの返答が表示され、アプリを再起動しても会話が残っていること。

### Stage 3: 経験の要約と記憶機能 (Experience Summary / Memory)
会話を通じてパートナーが知識やユーザーの好みを覚える仕組みを追加します。
1. 会話から要約すべき出来事（Experience candidate）を抽出
2. 要約（Experience Summary）の生成と、その根拠情報の保持
3. パートナー専用の記憶（Memory）の作成と更新
4. 会話による認識の訂正や、状況の変化への追従
5. 過去の記憶の検索と、次のプロンプトへの自然な組み込み
6. 管理画面での記憶一覧・根拠の確認機能
- **完了基準**: 会話を通じて新しい記憶が作られ、アプリ再起動後もその記憶を踏まえた自然な返答ができること。

### Stage 4: 作業用エージェント (Task Agent) とファイル操作タスク
指定したフォルダ（Workspace）を対象に、エージェントへファイル作業を任せる機能を作ります。
1. タスクのライフサイクル管理（開始、進行中、完了、失敗）
2. 対象フォルダ（Workspace）との安全な紐付け
3. 作業用エージェントへの処理委任
4. 権限チェック（フォルダ内のファイル一覧・読込・作成・編集のみを許可）
5. フォルダ外への不正アクセス（パストラバーサル）の確実な遮断
6. パートナーを通じた進捗報告・追加指示・作業キャンセル
- **cross-cutting prerequisite（Stage 4 内で先行）**: Task Agent の provider send を完成させる前に、Targeted Deletion 全体（Stage 6）ではなく、**送信 currentness に必要な最小 erasure-condition foundation** を導入します。これは保全・消去 owner の canonical current erasure-condition state とその durable store（restart 後も読める persistence）、送信 source 相関（`data_use`）に対する coverage 判定、AU14 推論試行 claim と同一トランザクションでの currentness compare、`DataUseHeld`、inference attempt の `data_use` durable 相関だけを持ちます。active condition set が空の場合も、canonical store を実際に照会した authoritative な空集合として送信を許可します（`NoDeletion` / `ErasureConditionRef::none()` / generation 0 のような placeholder や、producer の無い「削除なし」既定値を使いません）。
- **implementation slice 順（前提 PR 反映後）**:
  - A. minimal erasure-currentness foundation（preservation-owned current condition state、その persistence、AU14 coverage gate、`DataUseHeld`、`inference_attempt` の `data_use` 相関）
  - B. adopted-instruction History bounded read + prompt wiring（`load_message`、`TaskInstructionSource`、body resolution、correspondence validation、assembly、scrub）と、A を通した actual provider send
  - C. Cancel
  - D. autonomous Task Agent ↔ Action loop
  - E. Host 結合の Stage 4 E2E（レポート生成・パス脱出拒否・再起動読み戻し・キャンセル・遅延結果）
  - F. 会話・第一者管理経路からのタスク制御: `ProposeTaskCommand` と作業側のタスク作成 orchestration を `ene-companion` dialogue から開始し、実装済み `propose_steering` / `orchestrate_steering` へ会話の追加指示を配線する。会話の production 経路は、companion が provider 出力の最初の non-empty line に付す closed-world の `[task-control]` directive（exact field set）を `finish_turn` が解釈し、composition root の `DialogueTaskControlPort` が既存 Task owner 境界（作成・委任・steering・cancel・report）へ写す。task-control turn は provider の自然文を reply として残さず、typed owner outcome を scrub した後に presentation して同じ文字列を History に保存する。Workspace association は第一者管理経路の Owner 選択 premise からのみ成立させ、会話起点の書き込みは依拠 Owner message の currentness と同じ不分区間で commit する。Task 作成 + delegation の受理後は serving composition の launcher が既存 Task Agent runner を background 起動し、steering 後は new revision 用の新しい delegation を作成して同 launcher で起動する。`ManagementIntentKind::CancelTask` を `HostHandle::cancel_task` へ配線し、進捗・結果の報告を既存 Task/Action facts から組み立てる。最終 E2E で受け入れシナリオ 4.3（実行中の通常チャット）・4.4（キャンセル時点までの完了内容と残ファイルの報告）・4.5（完了報告の変更ファイル名・保存場所・やり残し）・4.7（既存ファイルを消さない）を、テスト側の直接操作ではなく会話入力だけを起点に確認する（4.6 のシェル・外部ネットワーク遮断とパス脱出は closed-world ツール語彙とワークスペース境界で構造的に保証済み）。
  - G. 作業担当が確定する terminal failure（`Failed`）producer。design が後続スライスへ委ねている Stage 4 item 1 の残り（`interface-boundaries.md` §Task progress / lifecycle）。
  - A と B を同じ implementation PR にまとめる必要はなく、レビューしやすい最小 stacked PR を優先します。ただし B を A より先に provider-send-enabled として merge しないでください。E の完了だけでは Stage 4 完了とせず、F の受け入れ E2E と G の producer で Stage 4 完了とします。
- **完了基準**: 指定フォルダ内のファイルを読んで新しいレポートを生成するタスクが正常に完了し、不正なファイルアクセスが確実に拒否されること。加えて、会話経由の受け入れシナリオ 4 を F の E2E で確認し、G の `Failed` producer を実装した上で Stage 4 完了とします。

### Stage 5: クライアントのライフサイクルとホストでの作業継続

Client を閉じても Host-only Task は継続し、再接続先で未伝達の進捗・結果を提示します。Host restart で失われた実行は自動で再開せず、保存済み facts の確認と明示 resume によって同じ Task の新 revision・新 delegation として続けます。

**設計の入口**:

| 決定事項 | 所有する文書 |
|---|---|
| 製品としての継続・提示・明示再開 | [requirements §Task](../requirements/requirements.md#切断中の報告と中断した作業の再開) |
| Host-only / Client-dependent の境界 | [Presence Transition §7–8](../design/critical-areas/client-presence-transition.md#7-host-side-taskが継続できる理由と範囲) |
| connection phase、incarnation、wire retry、read / resume / ACK | [IPC §9–13・18](../design/concrete/host-client-ipc.md#93-connection-authenticationreconnect-authentication) |
| Task / Delegation / 未伝達の identity | [CI §5.2–5.3・5.6](../design/concrete/correspondence-identity.md#52-会話ラウンド未伝達メッセージ) |
| resume command / outcome / source、報告の責務 | [IB H-A.1・H-G・X-H](../design/concrete/interface-boundaries.md#h-a1-中断-task-の明示-resume) |
| 保存項目、source of truth、startup、lifecycle 表 | [PR §4.6・6.4–6.5](../design/concrete/persistence-recovery.md#46-未伝達は発生元の-fact-と不可分に登録する) |
| commit 順序、launch 登録、connection/presence、ACK race | [CCT §7.4・10.4–10.5](../design/concrete/concurrency-control.md#74-resume-は旧-revision-を閉じて新しい委任を受理する) |
| E2E の期待結果 | [acceptance S5-01〜24](../requirements/acceptance.md#5-クライアントとホストの動作継続) |

**実装スライス順と完了条件**（各行は必要に応じて小さな PR に分けます。前の gate が揃う前に後の実行経路を enable しません）:

| 順 | 実装範囲と責務 | 完了条件 |
|---|---|---|
| A | `ene-core::conn/serve` と `ene-ctl::client` の connection phase、current install、terminal supersede、close admission。Client boot incarnation、descriptor と pairing identity の分離。OS transport と単一 Host lock | #1384/#1385/#1387/#1389 の回帰を接続入口で再現して通す。Linux socket と Windows named pipe が同じ認証/currentness を使う。startup 前の writer 排他、二つの Host、旧 socket の handshake 再入場拒否を検証する |
| B | `ene-presence` と `ene-store::presence` の AU7、hint、startup 復旧。Round・receipt の invalidation と Host-owned runner の切断独立性 | auth/close と presence commit の race、通常切断と restart の違い、Stopped/NoActive/InTransition の復旧表を満たす。Client を落としても受理済み Host-only Task は続き、Client-dependent admission は止まる |
| C | `ene-companion` の未伝達 source/status、`ene-store` の親 fact と同時登録。Task/Action/History の bounded report query。startup sealed-result reconciliation と read-only open の分離 | 各 producer の commit/crash で通知漏れ・重複登録なし。Unknown を再提示対象として読み、report query は durable mutation をしない。古い adoption 禁止記述との整合を #1561 の決定で固定する |
| D | `ene-presentation`、`ene-api`、Host/Client の未伝達購読・新 Round/receipt・提示 ACK。再接続時の Task 一覧/選択を既存 owner query に接続 | 切断中の completion が新 Client に自動提示される。ACK 喪失、51 件以上の backlog、新着競合、buffer 満杯、stale receipt を通す。provider 不調でも report と管理操作を使える |
| E | `ene-task` の ResumeTaskCommand / TaskResumeOutcome、AU17、採用指示 source の History/第一者 activity 解決。既存 TaskExecutionRegistry の launch 予約と runner を接続 | 同じ Task r+1・新 delegation を不可分に受理し、旧 delegation を起動しない。Unknown/terminal/running/stale/欠如/枯渇を区別する。commit 後・spawn 前の crash で自動再実行ゼロ。会話と第一者管理の両方が同じ owner gate を通る |
| F | 両 OS の Host 結合・第一者 Client E2E と Stage 4 回帰 | S5-01〜24 と各 OS の実 transport subset、workspace test / clippy / fmt / docs を完了する。implementation Issue は実装と回帰確認後にだけ close する |

**main 調査との対応**（基準 `b1cfe7f4`）: `conn.rs` の paired socket count / implicit supersede、`serve/handshake.rs` の再 challenge、Client connect ごとの incarnation、`serve.rs` の submit 時 attach と通常切断、`task_run.rs` の durable-start probe と一時 registry、`task_control.rs` の一時 Task 選択・canonical report・sealed reconciliation、`ene-store::companion` の History-only 未伝達・sticky Unknown が主な変更対象です。既存の AU3/4/5/14/15/16、Task lifecycle、seal、Task-wide Unknown barrier を再利用します。`main` の Windows transport は Unsupported なので A/F の完了から除外しません。

open Issue は #1384/#1385（Stage 5 defer）、#1387/#1389（接続 identity の前提）を本 Stage で扱います。#1390 の unknown wire variant は protocol evolution の別実装課題、#1508 の learning correction interface と #1530 の ScrubbedText 構築境界はそれぞれの owner の別課題として open を維持します。いずれも既存契約を迂回する根拠にはしません。設計間の不一致と選定理由は [#1561](https://github.com/pexisgle/ene/issues/1561) に記録します。

Stage 5 に専用 recovery manager/session/workflow、Task recovery generation、永続 current-delegation / needs-resume / launch queue、報告用の Task snapshot を追加しません。導出できない伝達状態と既存 source/hint の相関だけを所有先へ足します。Remote network、Voice、Observation、Computer Use、Schedule、全域 Restore、Targeted Deletion の本体は既存の後続 Stage に残し、必要な境界の unsupported/hold を維持します。新 schema は旧 DB の backfill や互換層を設計せず、現在の canonical schema と契約を直接実装します。

- **完了基準**: 切断・再接続・Host restart・明示 resume・遅延結果・提示失敗を通じて、事実を失わず、無断の新規 work と二重実行を起こさないこと。スライスの履歴は PR/Issue で管理し、`PROGRESS.md` は現在の milestone index に留めます。

### Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全
プライバシー保護のためのデータ完全消去と、API 利用量・費用の透明性を確保します。
1. 指定したキーワードに関連する会話履歴、要約、記憶、検索データの完全消去
2. 消去処理中に新しい記憶が誤って復活保存されないための排他制御
3. 完全に消去されたことの検証と監査ログ
4. OpenAI のトークン消費量（入力・出力・キャッシュ）と発生費用の記録・表示
5. API キーなどの認証情報がログやプロンプトに絶対に漏洩しないことのテスト
- **Stage 4 erasure-currentness foundation との関係**: Stage 6 は Stage 4 で導入した canonical current-condition store と AU14 claim 内の data-use 照合を置き換えず、同じ foundation を利用・拡張して、ユーザー向け Targeted Deletion の operation producer / deletion sweep / participant の列挙・調整 / actual erasure / delayed-arrival collection / remainder verification / finalizing / 全域完了 / 監査を完成させます。別の deletion gate / currentness registry / source correlation を新設しません。Stage 4 が Stage 6 に依存してはなりません。
- **完了基準**: 指定データの完全消去が派生データを含めて完全に完了すること、および機密漏洩テスト・費用表示が正しく動作すること。

### Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証
すべての機能を結びつけ、最初のマイルストーンとしての完成度を高めます。
1. 会話、記憶、タスク、費用を一覧できるテキスト管理画面
2. VRM 1.0 による透明デスクトップアバターの表示（移動、サイズ変更、待機/発話アニメーション）
3. アバターが停止してもチャットや管理画面が動くフォールバック
4. [受け入れ条件 (acceptance.md)](../requirements/acceptance.md) に定めた全テストシナリオの実施
5. Windows / Linux 両環境での性能テスト（CPU・メモリ・FPS）のクリア
- **完了基準**: 両 OS 上で全シナリオがパスし、性能基準をクリアしていること（**最初の開発マイルストーン完了**）。

---

## 3. 並列で進められる作業

- **Stage 3（記憶）と Stage 4（タスク作業）**:
  Stage 2 完了後、共有 API やデータモデルが固まっていれば、別々のブランチで並列に進められます。
- **デスクトップアバターの画面実装**:
  ホスト側のバックエンド開発（Stage 3〜6）と並行して、UI クライアント側の描画機能を先行して進められます。
- **テスト基盤の整備**:
  モック用の AI プロバイダーや擬似テスト環境の構築は、各機能開発と並行して進めて構いません。

---

## 4. 最初のマイルストーン完了後のロードマップ

最初のマイルストーンが完了した後は、以下の機能を順次追加していきます：

| ステージ | 機能内容 | 前提となる機能 |
|---|---|---|
| Stage 8 | 定期スケジュール実行 | タスク管理・権限・利用量管理 |
| Stage 9 | バックアップ・復元・ホスト自動起動 | データ保全・管理画面 |
| Stage 10 | 音声会話機能 (Voice) | クライアントUI・権限管理 |
| Stage 11 | 画面認識・見守り機能 (Observation) | タスク管理・クライアントUI |
| Stage 12 | グループ会話・パートナー同士の自発的な交流 | 会話基盤・記憶機能 |
| Stage 13 | 全体共有記憶・スキル学習・感情と関係性の発達 | 記憶・学習基盤 |
| Stage 14 | リモートクライアント接続（別PCからの利用） | 端末認証・バックアップ復元 |
| Stage 15 | キャラクター編集・パッケージ配布 | キャラクター管理・設定UI |
| Stage 16 | 複数 AI プロバイダー対応・自動フォールバック | 推論基盤・費用制限 |

---

## 5. 品質確認と完了チェックリスト

### 各 PR で確認すること
- 新規コードに対するユニットテストおよび結合テストが通っているか
- 通信エラー、二重実行、アプリ強制終了などの異常系テストが含まれているか
- API キーなどの機密情報がログや例外メッセージに漏れていないか
- データベース保存を伴う場合、アプリ再起動後も正しく復元できるか

### ステージ完了時に確認すること
- [受け入れ条件 (acceptance.md)](../requirements/acceptance.md) の該当シナリオがすべて正常に通ること
- `cargo test --workspace` および `cargo clippy --workspace --all-targets -- -D warnings` がエラーなく通ること
- 後続のステージが安全に依存できる公開インターフェースが整っていること
- Windows と Linux の両方で同一の手順が再現できること

