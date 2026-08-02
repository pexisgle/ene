# プラグインシステム関連クレート

> **クレート**: `ene-plugin-proto` (ワイヤープロトコル) | `ene-plugin` (開発用ファサード) | `ene-plugin-host` (プロセススーパーバイザ)

これらのクレート群は、Ene の統一されたプロセス外 IPC プラグイン基盤を形成します。ツール、カスタム LLM/TTS/STT プロバイダ、MCP サーバーはすべて、プロセス内コードではなく独立したサブプロセスとして動作します。

---

## アーキテクチャ境界

- `ene-plugin-proto` はワイヤープロトコルのみを扱います — ビジネスロジック、データベースアクセス、AI プロバイダへの依存を持ってはいけません。ツール IPC のワイヤーメッセージと、より豊富なプラグインプロトコル (ハンドシェイク、機能宣言、ストリーミング LLM メッセージ) の両方、およびクロスプラットフォームのトランスポート層 (UDS / named pipe フレーミング) を定義します。
- `ene-plugin` はプラグインバイナリが利用する開発用ファサードです。`ene-runtime`、`ene-mind`、`ene-store` に依存しません。ホスト側では使用されません。
- `ene-plugin-host` はホスト側専用です: プロセスの発見/起動、ハンドシェイクネゴシエーション、ツール/LLM プロバイダレジストリへの機能ルーティング、ヘルスプローブ、シャットダウンを担当します。プラグインが提供する LLM プロバイダは IPC アダプタ経由で `ene_ai::LlmProvider` にブリッジされ、プラグイン提供のツールと MCP のツールは単一のツールレジストリインターフェースの背後に集約されます。
- `plugins/tool/*` と `plugins/provider/*` のバイナリは軽量に保ちます — 依存先は `ene-plugin` であり、任意のクロスクレートビジネスロジックには依存しません。

## 設計思想

- **なぜ動的ロードやプロセス内トレイトオブジェクトではなくプロセス外プラグインか**: プロセス分離により、クラッシュしたり誤動作したりするツール/プロバイダがホストを巻き込むことはなく、各プラグインを個別にサンドボックス化・再起動・バージョン不一致に対応できます。コストは IPC フレーミングとハンドシェイクプロトコルですが、`ene-plugin-proto` がこれを一箇所に集約するため、プラグインごとに再実装する必要はありません。
- **なぜ固定プロトコルバージョンではなくバージョン付きハンドシェイク (`VersionRange` ネゴシエーション) か**: これにより、古い/新しい `ene-plugin-proto` に対してビルドされたホストとプラグインバイナリが、あらゆる不一致でハードに失敗する代わりに、共通のプロトコルバージョンに合意できます。
- **なぜ `ene-plugin-host` にサーキットブレーカーがあるか**: 繰り返し失敗するプラグインプロセス (例: 設定ミスのプロバイダ) は、そうしなければ呼び出しのたびにリトライされてしまいます。ブレーカーは連続失敗が閾値に達すると即座に失敗させ (fail-fast)、壊れたプロセスを叩き続ける代わりにクールダウンしてから再試行します。
- **なぜ再起動予算が健全なラウンドトリップで回復するか**: 監視機構はクラッシュまたは応答なしのプラグインを指数バックオフで再起動しますが、再起動回数に生涯上限を設けると、数日かけて数回クラッシュしただけで長時間稼働するコンパニオンのプラグインが永続的に無効化されてしまいます。そのため予算は、成功したツール呼び出しか健全なヘルスプローブ ping という、あらゆる健全なラウンドトリップでゼロにリセットされ、生涯のクラッシュ回数ではなく*最近の*不安定性を測ります。ヘルスプローブによるリセットこそが provider 専用プラグインをカバーする仕組みです — これらはツールを公開しないため、ツール呼び出しによるリセット経路を持ちません (#433)。ping は正常だが実際の処理は失敗するプラグインは、レジストリごとのサーキットブレーカーによって引き続き封じ込められます。
- **なぜ制御ブロードキャストを並列化し、権限承認をルーティングするのか**: `CompositeToolRegistry` の制御系メソッド (`set_call_context`、`allow_pattern`、`revoke_pattern`、およびフォールバックの `approve_permission`) は独立したプラグイン接続へ扇出するため、`join_all` で並列実行されます — 最悪ケースのレイテンシは全プラグインの合計ではなく、最も遅い 1 つのプラグインに抑えられます。権限承認はさらに踏み込みます: 要求は 1 件のツール呼び出しから発生するため、`approve_permission_for` はブロードキャストではなく所有元のサブレジストリへ 1 往復で直接承認を届けます (#434)。これにより、ユーザーの「許可」が無関係なプラグインの長いツール呼び出しの完了待ちで遅延することを防ぎます。
- **なぜ単一の逐次ヘルスループではなく、プラグインごとに 1 つの監督タスクを置くのか**: 監視対象の各プラグインは、それぞれ独立したタスクによって監視されます。そのため、あるプラグインの再起動バックオフ (指数関数的に増加し上限 30 秒) や遅い再接続が、他のプラグインのヘルス監視を止めることは決してありません。全プラグインを順番に ping する単一ループでは、1 つの不調なプラグインが全プラグインのプローブ — つまり検知と再起動 — を遅らせてしまいます (#432)。

## プロバイダ機能 derive

静的プロバイダ機能宣言 (`LlmProviderSpec` / `TtsProviderSpec` / `SttProviderSpec`
の構築) は、`ene-plugin-macros` が単一の `#[provider(...)]` 属性から生成し、
`ene_plugin::prelude` 経由で再エクスポートされます。derive はプラグイン構造体
自体に付与します:

```rust
use ene_plugin::prelude::*;

#[derive(LlmPlugin)]
#[provider(
    kind = "anthropic",
    models = "claude-sonnet-4-20250514, claude-haiku-4-20250514",
    streaming,
    vision,
    concurrency = 8,
    queue_depth = 16,
    context_window = 200_000,
)]
pub(crate) struct AnthropicPlugin;

impl ConfigurablePlugin for AnthropicPlugin {
    // config_schema / set_config は手書きのまま（API キー処理は provider 固有）。
}

#[async_trait]
impl LlmPlugin for AnthropicPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }

    async fn create_chat_stream(/* ... */) -> Result<PluginStream, PluginError> {
        // ストリーミング処理は手書き
    }
}
```

derive がプラグイン構造体に生成するもの: 内在メソッドの `llm_spec()` /
`tts_spec()` / `stt_spec()` コンストラクタと、トレイト別の kind 定数
(`LLM_PROVIDER_KIND` / `TTS_PROVIDER_KIND` / `STT_PROVIDER_KIND`。例:
非同期ハンドラ内の `if kind != Self::LLM_PROVIDER_KIND` ガード用)。
`*_capabilities()` トレイトメソッドは作者自身が書く 1 行
`vec![Self::<trait>_spec()]` です — derive は `impl LlmPlugin` 自体を生成でき
ません。なぜなら非同期ハンドラは同じトレイト impl 内に置く必要があり、
Rust は同一型に対する同一トレイトの 2 つ目の `impl` ブロックを拒否するからです
(E0119)。したがって非同期ハンドラと `ConfigurablePlugin` (`config_schema` を
含む) は常に手書きです。

対応する属性キー: `kind` (必須)、`models`、`voices`、`formats` (カンマ区切り
リスト)、`streaming`、`vision` (フラグ)、`concurrency` / `queue_depth`
(省略時は直列の `ConcurrencyHint` に既定化)、`context_window`。複合プロバイダ
(`#[derive(LlmPlugin, TtsPlugin)]`) は 1 つの `#[provider(...)]` 属性 — つまり
1 つの `kind` — を複数トレイトで共有します。`EmbedPlugin` には生成すべき静的
宣言が存在しない (`embed_batch` がトレイト全体) ため、derive はありません。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-plugin-proto --open
cargo doc -p ene-plugin --open
cargo doc -p ene-plugin-host --open
```

開発用には `ene_plugin::run_plugin_server` / `PluginDispatch`、ホスト側の監視には `ene_plugin_host::PluginHostManager` / `CompositeToolRegistry` から始めてください。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [ツール SDK リファレンス](tool-sdk.md)
