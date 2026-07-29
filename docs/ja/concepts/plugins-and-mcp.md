# IPC プラグインシステムと MCP 連携

本ドキュメントでは、Ene のプロセス外 IPC プラグインアーキテクチャ、Protocol v4 ワイヤー仕様、Model Context Protocol (MCP) サーバー連携、および組み込みツールプラグインについて解説します。

---

## 1. プロセス外プラグインアーキテクチャ

プロセス分離、動作の安定性、セキュリティを確保するため、ツールプラグイン、カスタム LLM プロバイダ、MCP サーバーなどのすべての外部拡張機能は `PluginHostManager` (`ene-plugin-host`) によって管理される独立したサブプロセスとして動作します。

```text
Ene ホストアプリケーション (ene-runtime)
  │
  └── PluginHostManager (ene-plugin-host)
        │
        ├── IPC Protocol v4 (stdio 上の長さプレフィックス付き JSON)
        │     ├── ene-plugin-anthropic (Anthropic LLM プロバイダプラグイン)
        │     ├── ene-plugin-app       (GUI 起動ツール)
        │     ├── ene-plugin-browser   (CDP ブラウザ自動化ツール)
        │     ├── ene-plugin-fs        (サンドボックス化ファイルシステムツール)
        │     ├── ene-plugin-utility   (電卓 & TODO 管理ツール)
        │     └── ene-plugin-web       (Web 検索 & スクレイパーツール)
        │
        └── Model Context Protocol (MCP) ブリッジ (ene-plugin-host)
              └── 外部 MCP サーバー (Node.js / Python / Go などの MCP プロセス)
```

---

## 2. IPC Protocol v4 仕様

プラグインは `stdin`/`stdout` 上で **IPC Protocol v4** を使用して通信します：

- **フレーミング**: すべてのパケットは 4 バイトのリトルエンディアン `u32` パケットサイズで始まり、UTF-8 JSON ペイロードが続きます。
- **ハンドシェイクネゴシエーション**: ホストは `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`、すなわち単一の固定値ではなく `VersionRange { min: 3, max: 4 }` を送信します。プラグインは `VersionRange::negotiate` でその範囲と自身がサポートする範囲の共通部分を取り、両者に共通する最大バージョンを `HandshakeAck { version, capabilities: PluginCapabilities }` として返します。
- **ハンドシェイクタイムアウト**: ホストは `HandshakeAck` の待ち時間に上限を設けています（`plugins.handshake_timeout_ms`、既定 10 秒）。ソケットを accept しながら応答しないプラグインは、残りのプラグインの起動をブロックする代わりに `PluginHostError::HandshakeFailed` でハンドシェイクに失敗します。プラグイン作者はハンドシェイクに即応答し、重い初期化（モデル読み込み等）はその後へ遅延させる必要があります——`ene-plugin` の `run_plugin_server` を参照してください。
- **リクエスト相関**: 非同期リクエストおよびレスポンスはすべて必須の `request_id` (`Uuid`) を保持します。
- **ケーパビリティ**: プラグインはサポートする機能 (`tools`, `llm_providers`, `stt_providers`, `tts_providers`) を宣伝し、各プロバイダ仕様はさらに `concurrency: ConcurrencyHint` を宣言します ([§3](#3-プロバイダの並行度-concurrencyhint) 参照)。

### バージョニングポリシー（N-1 後方互換）

ツール・プロバイダプラグインは独立したプロセス外バイナリとして配布されます。`PLUGIN_IPC_PROTOCOL_VERSION` を上げても既存のプラグインバイナリは再コンパイルされないため、ホストは**1バージョン分の後方互換**を維持します。

- ホストは常に単一の固定バージョンではなく `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]`（`crates/ene-plugin-proto/src/ipc.rs` の `VersionRange::host_supported()`）をハンドシェイクで提示します。1つ前のプロトコルバージョンでビルドされたプラグインでも、そのバージョンでネゴシエートして接続できます。
- プラグインバイナリ側は範囲をサポートする必要はなく、自身がビルドされたバージョンを `VersionRange { min: N, max: N }` として申告してよいです。互換性を維持する責務はホスト側に集約されており、個々のプラグイン作者に強制されません。
- **プロトコルバージョンのバンプ**: `PLUGIN_IPC_PROTOCOL_VERSION` を上げる際は `PLUGIN_IPC_MIN_SUPPORTED_VERSION` も同じ数だけ繰り上げ、最も古いサポート対象バージョンのサポートを打ち切ります。
- **バンプが必要なケース**: 既存メッセージの意味変更、必須フィールドの追加、enum variant の削除・リネームの場合のみです。新しいフィールドは `#[serde(default)]` を使うことで、バージョンバンプなしに新旧のピア間で互換性を保てます。
- **機能ゲート**: ホストはネゴシエート済みバージョンを `ene-plugin-host` の `IpcPluginConnection` に保持し、`negotiated_version()` で参照できます。最小サポートバージョンより後に追加されたメッセージに依存する挙動はこれをもとにゲートすべきです。たとえば `supports_cancel_stream()` は v4 で追加された `PluginIpcRequest::CancelStream` をゲートしており、v3 のプラグインには理解できないメッセージを送らず、既存のタイムアウトベースのストリーム終了にフォールバックします。
- **ネゴシエーション失敗の診断**: プラグインが提示する範囲とホストのサポート範囲が重ならない場合、プラグイン側の `HandshakeAck` エラーおよびホスト側の `PluginHostError::HandshakeFailed` / `ProtocolMismatch` はいずれも双方の範囲を明記します（例: "host supports 3..=4, plugin supports 2..=2"）。これにより、単なる汎用的なハンドシェイク失敗ではなく、プラグインバイナリの再ビルドが必要であることが開発者に伝わります。

---

## 3. プロバイダの並行度 (`ConcurrencyHint`)

プロセス境界は「ホスト」を不正なプロバイダプラグインから守ります——不正なプラグインがホストの tokio ブロッキングプールを枯渇させることはできません。しかし、それだけでは「プラグイン自身」は守られません。ホストが単一のプラグインバイナリに対して無制限に同時リクエストを送ることを妨げるものは何もなく、また「自分はローカルモデルなので一度に一件ずつ実行してほしい」とプラグインが表明する手段もありませんでした。`ConcurrencyHint` はこのギャップを埋めます。

`PluginCapabilities` の各エントリ——`LlmProviderSpec`、`TtsProviderSpec`、`SttProviderSpec`——は `concurrency: ConcurrencyHint` フィールドを持ちます。

```rust
pub struct ConcurrencyHint {
    /// Max jobs this provider can run at once.
    pub max_in_flight: u32,
    /// Extra jobs to queue before rejecting.
    pub queue_depth: u32,
}
```

### デフォルトは意図的に直列

`ConcurrencyHint::default()` は `max_in_flight: 1, queue_depth: 2` ——一度に1ジョブのみ、その後ろに浅いキューを1つ、です。これは見落としではなく、設計上の要となる決定です。並行性についてまったく考えていないプラグイン作者は、考えていない「からこそ」保守的で安全な挙動を得られます。より高い並行度を求めるプラグイン——典型的にはクラウド API へのステートレスな HTTP プロキシで、多数のリクエストを安全に同時処理できるもの——は `concurrency` を明示的に設定する必要があり、そう設定すること自体が、作者がこの問題を検討した証拠になります。組み込みの Anthropic プラグインはまさにこれを行っています。

```rust
fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
    vec![LlmProviderSpec {
        kind: "anthropic".to_string(),
        // ...
        concurrency: ConcurrencyHint { max_in_flight: 8, queue_depth: 16 },
    }]
}
```

このフィールドは、v3 以降にこのワイヤープロトコルへ追加された他のすべてのフィールドと同様に `#[serde(default)]` です——上記の[バージョニングポリシー](#バージョニングポリシーn-1-後方互換)を参照してください。`ConcurrencyHint` が存在する前にビルドされたプラグインバイナリは単にこのフィールドをワイヤー上で省略しますが、ホストはフィールドが欠けている場合、ハンドシェイクを失敗させたり無制限の並行度を既定にしたりするのではなく、安全な直列デフォルトとしてデシリアライズします。追加にあたってプロトコルバージョンのバンプは不要でした。

### ホスト側での強制

`ene-plugin-host` の `IpcLlmProvider` (`crates/ene-plugin-host/src/ipc_provider.rs`) は、宣言されたヒントを `ConcurrencyLimiter` で強制します: `max_in_flight` にサイズを合わせた `tokio::sync::Semaphore` と、パーミットを待てる呼び出し元を最大 `queue_depth` 件まで許容する仕組みです。両方の上限を超えたリクエストは、待ちキューを無制限に伸ばすのではなく `LlmProviderError::Busy` で即座に失敗します——`ene-infer` がローカル推論側で適用しているのと同じ「無限にキューイングするより早く失敗させる」規律を、プラグイン IPC 境界にも適用したものです。このリミッタは (プラグイン, プロバイダ種別) のペアごとに `IpcLlmProviderFactory` の中で一度だけ構築され、そのペアに対して以降作成されるすべてのプロバイダインスタンスで共有されます。呼び出しのたびに新しい `IpcLlmProvider` が作られるためです。ストリーミングリクエストの場合、取得したパーミットはストリームの生存期間中ずっと保持され、ストリームが自然に完了したか途中でキャンセルされたかにかかわらず、ストリームが drop された時点で自動的に解放されます。

### ローカル推論プラグイン作者向け: プロセス内でも同じ規律を

`ConcurrencyHint` が制限できるのは「ホスト」がプラグインに対して一度に送るリクエスト数だけです。自前でローカル推論を行うプラグイン (llama.cpp、whisper.cpp、ローカル TTS エンジンなど) は、それでも「自分自身のプロセス内」で並行性を正しく扱う必要があります——ホストは、そのプラグインのコードが到着したジョブをどう捌くかを見ることも制御することもできません。そのために、`ene-plugin` の `prelude` モジュールに依存してください。これは `ene-infer` のワーカースレッドフレームワーク (`EngineConfig`、`EngineError`、`EngineHandle`、`JobContext`、`LocalModel`、`StopReason`) を、通常のプラグイン作成用の型と並べて再エクスポートしています。

```rust
use ene_plugin::prelude::*;

struct MyLocalModel { /* ... */ }

impl LocalModel for MyLocalModel {
    type Request = MyRequest;
    type Response = MyResponse;
    type Error = MyModelError;

    fn engine_name(&self) -> &str { "my-local-model" }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        // Cooperatively check `ctx.should_stop()` at natural interruption points.
        // ...
    }
}

let handle = EngineHandle::spawn(|| Ok(MyLocalModel::load()?), EngineConfig::default());
```

`EngineHandle` は専用のワーカースレッド上でモデルを所有し、無制限に増え続けるのではなく境界のあるキューから即座に失敗する (`EngineError::Busy`) ジョブ実行を行い、パニックしたジョブからはモデルを再構築して復旧します——ホスト自身が依拠しているのと同じ保証を、`use` 一行で得られます。これがなければ、自前で書いた `spawn_blocking`/`block_in_place` による並行処理は、結局プロセス境界の向こう側にバグを移すだけになってしまいます。

---

## 4. 組み込みプラグインカタログ

| プラグインバイナリ | ネームスペース | 説明 | ステートフル？ |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | システムアプリ起動・ウィンドウ制御 | いいえ |
| `ene-plugin-browser` | `browser.*` | ヘッドリス Chrome/CDP ブラウザ自動化 | はい (セッションストア) |
| `ene-plugin-fs` | `fs.*` | サンドボックス化ファイル操作 & Undo 履歴 | はい (DB IPC ソケット) |
| `ene-plugin-utility` | `utility.*` | 電卓、日時、TODO リスト管理 | はい (DB IPC ソケット) |
| `ene-plugin-web` | `web.*` | Web 検索および Markdown ページ抽出 | いいえ |
| `ene-plugin-anthropic` | Provider | Anthropic Claude プロバイダプラグイン | いいえ |

上記 6 プラグインはすべてデフォルトの `plugins.list` に含まれており、
新規インストール時に自動的に起動します。

---

## 5. プラグインセキュリティモデル

### オプトイン型ディスカバリ

プラグインのディスカバリは**オプトイン方式**です。`plugins.list` に明示的に
`enable: true` で登録されたバイナリのみが起動します。プラグインディレクトリに
バイナリを配置するだけでは実行されず、ホストは設定への追加を促す警告を
ログに出力します。これにより「バイナリ配置 → 自動実行」の攻撃経路を遮断します。

```jsonc
// settings.json (抜粋)
{
  "plugins": {
    "list": {
      "fs": { "enable": true },
      "anthropic": { "enable": true, "env_passthrough": ["ANTHROPIC_API_KEY"] }
    }
  }
}
```

### 環境変数のハードニング (`env_clear`)

すべてのプラグインおよび MCP stdio サーバーは `env_clear()` 付きで起動されます。
継承された環境変数は消去され、明示的なホワイトリストのみが転送されます：

| 変数 | 用途 |
|---|---|
| `PATH` | システム実行ファイルの探索 |
| `HOME` | ユーザー設定ファイル |
| `TMPDIR` | 一時ファイル |
| `LANG` | ロケール依存出力 |
| `TZ` | タイムゾーン (設定時のみ) |
| `LD_LIBRARY_PATH` | 共有ライブラリ読み込み (Linux) |
| `SystemRoot`, `USERPROFILE`, `APPDATA`, `TEMP`, `PATHEXT` | Windows 必須変数 |
| `ENE_PLUGIN_SOCKET` | IPC チャネル (プラグインのみ) |

### プラグインごとの `env_passthrough`

追加のホスト環境変数 (API キーなど) が必要なプラグインは、`plugins.list`
エントリの `env_passthrough` で明示的に宣言します。セキュリティ上危険な名前
(`LD_PRELOAD`、`LD_AUDIT`、`DYLD_INSERT_LIBRARIES`、`ENE_PLUGIN_SOCKET` など)
は設定に関係なくブロックする組み込み拒否リストが適用されます。

MCP stdio サーバーも `plugins.mcp_servers` エントリに同じ `env_passthrough`
フィールドをサポートしています。

### バイナリチェックサム検証 (TOFU)

初回起動時にホストはプラグインバイナリの SHA-256 チェックサムを計算し、
`plugins.list.<name>.checksum` に記録します (Trust-on-First-Use)。
以降の起動ではバイナリを記録済みチェックサムと照合し、変更があれば
起動を拒否します。比較は大文字小文字を区別しません (16進エンコーディング)。

このチェックサムは監視下での再起動のたびにも再検証されます。ホストは
クラッシュまたは応答なしのプラグインを kill して再 spawn する前に、
ディスク上のバイナリのチェックサムを再計算し、起動時にピン留めした値と
比較します。ホストの稼働中にバイナリが変更された場合 (たとえば開発中に
`cargo build` がバイナリを置き換えた場合など)、再起動は中断され、その
プラグインはホスト自体を再起動するまで**永続的に無効化**されます。これは
意図的な動作です。稼働中のインスタンスは元のバイナリに対して検証済みのため、
ホストは別のバイナリを暗黙に exec することを拒否します。新しいバイナリを
反映するにはホストを再起動し、そのチェックサムを再度ピン留めしてください。

---

## 6. MCP (Model Context Protocol) 連携

`ene-plugin-host` は外部 MCP サーバーを統合します (かつての `ene-connector`
ブリッジ層は #416 で撤去されました — 接続ライフサイクルはすべてプラグインホスト内にあります)：

1. **発見と起動**: ホストは `plugins.mcp_servers` 設定を読み込み、 `stdio` または HTTP/SSE 経由で対象の MCP サーバーバイナリを起動します。
2. **ツール変換**: MCP ツールは自動的に `ToolSpec` アイテムに変換され、 `CompositeToolRegistry` に登録されます。
3. **実行ルーティング**: LLM から生成されたツール呼び出しは MCP ブリッジを経由してルーティングされ、 `ene-runtime` に返却されます。

サーバー名はルーティングとツール名前空間にそのまま使用され、文字種の検証は
行われないため、 `github-mcp` のようなハイフン入り名前も他の名前と同様に接続
できます (#417)。

### HTTP URL 検証 (SSRF 対策)

HTTP の MCP エンドポイント (`transport.type = "http"`) は、接続を試みる**前に**
`McpToolRegistry::connect_http` によって URL を検証されます。既定は拒否です：

- **HTTPS のみ。** `http://` URL は拒否されます。
- **ループバック拒否。** `127.0.0.0/8` と `::1` は拒否されます。
- **リンクローカルは常に拒否。** クラウドメタデータエンドポイント
  `169.254.169.254` を含む `169.254.0.0/16` と `fe80::/10` は、いかなる設定でも
  拒否されます。

拒否は tracing ログと返却されるエラー (`PluginHostError::McpHandshake`) の
両方に、サーバー名と理由を含めて報告されます。

ローカル開発向けに、 `plugins.mcp_allow_insecure_urls` (既定 `false`) で
プレーン `http://` URL とループバックエンドポイントを許可できます：

```jsonc
// settings.json (抜粋)
{
  "plugins": {
    "mcp_allow_insecure_urls": true,
    "mcp_servers": [
      { "name": "local-dev", "enabled": true,
        "transport": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } }
    ]
  }
}
```

このオプトインはリンクローカルのブロックを緩和しません。DNS リバインディング
(内部アドレスに解決されるホスト名) はスコープ外です：検査されるのは IP リテラルの
ホストのみです。これは、本番の接続経路で検証を一切行っていなかった以前の挙動より
弱くなることはありません。

---

## 7. カスタムツールプラグインの開発

開発者は `ene-plugin` の `#[derive(ToolAction)]`（`ene-tool-macros` 経由）とサーバーエントリポイントを使用して独自のツールプラグインを作成できます。以下は説明用のスケッチです — 実際にコンパイルが通る現行パターンは `plugins/tool/*` 配下の既存プラグイン (例: `plugins/tool/app/src/main.rs`) や `cargo doc -p ene-tool-macros --open` を参照してください：

```rust,ignore
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "custom", name = "greet",
       summary = "パーソナライズされた挨拶文を生成します。", category = "Custom",
       keywords_primary = "greet, hello")]
pub struct GreetAction {
    pub name: String,
}

impl GreetAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("こんにちは、{}さん！", self.name))
    }
}

#[tokio::main]
async fn main() {
    use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
    use std::sync::Arc;

    let provider = ActionSetProvider::new(vec![Box::new(GreetAction { name: String::new() })]);
    let dispatch = PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    );
    if let Err(e) = run_plugin_server(dispatch).await {
        tracing::error!("fatal error: {e}");
        std::process::exit(1);
    }
}
```
