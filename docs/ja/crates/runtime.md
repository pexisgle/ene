# `ene-runtime`

> **クレート**: `ene-runtime` | **役割**: アクターベースのホストファサード & ターンエンジン

`ene-runtime` は Ene を組み込むアプリケーション (`ene-cli`, `ene-desktop`) のメインエントリポイントです。スレッドセーフなファサードである `EneHandle` を所有し、ターンの実行、プロンプト構築 (`ene-mind`)、記憶永続化 (`ene-store`)、プラグイン監視 (`ene-plugin-host`)、およびツール DB IPC ソケットサーバーを統合管理します。

---

## アーキテクチャ境界

- `EneHandle` の公開メソッドはすべて、単一スレッドのアクター (`handle::actor::TurnActor`) への非ブロッキングなチャネル送信または oneshot 非同期リクエストです。共有可変状態に直接触れることはありません。
- 読み取り専用のセッション/候補クエリと画面画像のビジョン要約は、アクターのメールボックスを完全にバイパスして `ene-store` やビジョンモデルに直接アクセスします。ターン実行コマンドとアクターのスループットを奪い合いません。
- イベントバスは単一チャネルではなく、トラフィックの性質ごとに3系統の専用チャネルへ分離されています: `broadcast` によるチャットバス (`EneEvent`)、bounded かつ単一コンシューマの `mpsc` による音声チャネル (`AudioChunk`)、小容量の `broadcast` によるライフサイクルバス (`LifecycleEvent`)。一方のチャネルのバーストが他方の subscriber を lag させたり飢餓状態にしたりすることはありません。
- 安定版パブリック API v1 契約は `public_api` にすべて集約されています (`PublicApiError`, `PublicChatEvent`, `PublicLifecycleEvent`, `PublicSessionMeta`, `PublicExportedMessage`, `API_VERSION`)。`Public*` 型のフィールドに `ene-store` / `ene-mind` / `ene-plugin-proto` の型が現れることはありません。内部エラー enum は `From` 実装を介して `PublicApiError` の安定したカテゴリへ射影されるため、内部エラーバリアントの追加が契約を破壊することはありません。
- アクターの死亡は一様に `PublicApiError::ActorDead` として報告されます (#408)。`EneHandle` のアクター制御メソッド (権限、undo、ユーザー入力、機能更新) と読み取り専用の diagnostics / vision ハンドルはすべて、専用の「アクター死亡」型ではなく `PublicApiError` を返します。消費者が分岐するエラー型は `RunError` / `CancelError` / `PublicApiError` の3系統のみです。`RunError::Busy` と `CancelError::TurnMismatch` は呼び出し側がこれらに基づいて動作するため保持されています。`EneRuntimeError` はブートストラップと、チャネル断絶以外のアクター側失敗 (例: `EneRuntimeError::Busy` タスクアドミッション) も表面化する diagnostics メソッドのためだけに残されています。
- `message_builder` と `streaming` は意図的に `#[doc(hidden)]` になっています — API v1 契約の一部ではなく、CLI のデバッグコマンドと統合テストのためだけに公開されています。

## 設計思想

- **なぜアクターモデルか**: ターン実行には、共有状態 (アクティブターン、Undo スタック、パーミッション許可) に対する厳密に直列化されたミューテーションが必要ですが、それを生のロックとして非同期かつ複数コンシューマ向けの API に露出させたくありません。単一スレッドのアクターメールボックスはこの直列化を自然に実現し、`EneHandle` を安価に clone 可能に保ちます。
- **なぜパニック分離が重要か**: `ene-desktop` は GUI・アクター・LLM ストリーミング・オーディオを単一プロセス内でホストします。ディスパッチされるすべてのコマンドとバックグラウンドタスクは `catch_unwind` ベースの分離を経由するため、コマンド内のパニックはプロセス全体を巻き込む代わりに診断イベントとして表面化します。この仕組みはワークスペースが release プロファイルで `panic = "abort"` を設定していないことに依存しています — 詳細な仕組みとそのビルド設定がなぜ重要かは `docs/architecture.md` §4 を参照してください。
- **なぜイベントバスを3系統に分離したか**: 単一の混合 `broadcast` チャネルでは、重量級の `AudioChunk` PCM ペイロードがすべてのチャットsubscriberのバッファを膨張させ、チャット流量とは無関係な理由で subscriber を lag させていました。トラフィックの性質ごとに分離することでこの結合を取り除いています。
- **なぜ読み取り専用クエリがアクターをバイパスするか**: セッションの一覧表示・エクスポート・検索やビジョン要約はターン実行クリティカルな状態に触れないため、`Run`/`Cancel` と同じメールボックスを経由させると回避可能なヘッドオブラインブロッキングが発生します。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-runtime --open
```

`EneHandle` から始め、イベントバスについては `handle::EneEvent` と `handle::LifecycleEvent` を参照してください。

---

## 関連ドキュメント
- [システムアーキテクチャ](../architecture.md)
- [ターンとセッション](../concepts/turn-and-session.md)
