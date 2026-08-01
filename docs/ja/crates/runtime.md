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
- **なぜ副作用のないツール呼び出しだけを並列実行するのか**: LLM が 1 つの応答で複数のツール呼び出しを出力したとき、それらを厳密に 1 つずつ実行するとレイテンシが N 倍になり（各呼び出しは `tools.timeout_ms` が上限）、`TextDelta` を停滞させます。しかし、権限/ユーザー入力のプロンプト、Undo スタック、同一リソースへの書き込み、`ToolCallStart`/`ToolCallResult` / `ToolResultSummary` の順序といった正しさの不変条件の多くは、決定論的な実行順序に依存します。その解決策が 2 段階ループです。`ToolSpec` で `side_effects: ReadOnly` を宣言している（かつバックグラウンド非対応の）ツールは `plugins.parallel_tool_calls_max` を上限に並列ディスパッチされ、生の結果だけを捕捉します。その後、すべてを元の `tool_calls` の順序で最終化し、イベントの発行・プロンプトの解決・Undo スタックの更新を逐次行います。並列分類されたツールが予期せず `PermissionRequired`/`UserInputRequired` を返した場合は、逐次解決へフォールバックします。分類はフェイルクローズドです。`ReadOnly` の副作用を宣言していないツール（Tool RAG を必要とする `system.search_tools` を含む）は決して並列化されず、`parallel_tool_calls_max: 0` は従来の完全逐次動作を復元します。
- **なぜコマンドメールボックスを unbounded のままにするか**: Stage 8 はアクターの5つのバックグラウンド `JoinSet` を有界化しましたが、それらを供給するコマンドチャネルは意図的に unbounded のままです (#404)。このチャネルは外部コンシューマ、アクター自身のバックグラウンドタスク (`PluginHostReconfigured` をここ経由でフィードバックする)、そして最後のハンドル破棄ガード (同期的な `Drop` から `Shutdown` を送る) の三者で共有されています。`try_send` による背圧を持つ有界チャネルでは、シャットダウンコマンドや内部の再構成が暗黙にドロップされかねず、それは緩やかな劣化ではなく正しさのバグです。本当に高コストな処理 (ツール呼び出し、検索、deferred ポーラー、GGUF ロード) は、`JoinSet` へのアドミッションという肝心な場所で有界化され、`EneRuntimeError::Busy` で即座に失敗します。外部からフラッド可能な唯一のコマンド — `UpdateProactiveObservation` — はその供給源 (画面キャプチャの間隔) でレート制限されているため、現実的なフラッド経路はメールボックスではなく上流で有界化されています。

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
