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
        └── Model Context Protocol (MCP) ブリッジ (ene-connector)
              └── 外部 MCP サーバー (Node.js / Python / Go などの MCP プロセス)
```

---

## 2. IPC Protocol v4 仕様

プラグインは `stdin`/`stdout` 上で **IPC Protocol v4** を使用して通信します：

- **フレーミング**: すべてのパケットは 4 バイトのリトルエンディアン `u32` パケットサイズで始まり、UTF-8 JSON ペイロードが続きます。
- **ハンドシェイクネゴシエーション**: ホストは `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`、すなわち単一の固定値ではなく `VersionRange { min: 3, max: 4 }` を送信します。プラグインは `VersionRange::negotiate` でその範囲と自身がサポートする範囲の共通部分を取り、両者に共通する最大バージョンを `HandshakeAck { version, capabilities: PluginCapabilities }` として返します。
- **リクエスト相関**: 非同期リクエストおよびレスポンスはすべて必須の `request_id` (`Uuid`) を保持します。
- **ケーパビリティ**: プラグインはサポートする機能 (`tools`, `llm_providers`, `stt_providers`, `tts_providers`) を宣伝します。

### バージョニングポリシー（N-1 後方互換）

ツール・プロバイダプラグインは独立したプロセス外バイナリとして配布されます。`PLUGIN_IPC_PROTOCOL_VERSION` を上げても既存のプラグインバイナリは再コンパイルされないため、ホストは**1バージョン分の後方互換**を維持します。

- ホストは常に単一の固定バージョンではなく `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]`（`crates/ene-plugin-proto/src/ipc.rs` の `VersionRange::host_supported()`）をハンドシェイクで提示します。1つ前のプロトコルバージョンでビルドされたプラグインでも、そのバージョンでネゴシエートして接続できます。
- プラグインバイナリ側は範囲をサポートする必要はなく、自身がビルドされたバージョンを `VersionRange { min: N, max: N }` として申告してよいです。互換性を維持する責務はホスト側に集約されており、個々のプラグイン作者に強制されません。
- **プロトコルバージョンのバンプ**: `PLUGIN_IPC_PROTOCOL_VERSION` を上げる際は `PLUGIN_IPC_MIN_SUPPORTED_VERSION` も同じ数だけ繰り上げ、最も古いサポート対象バージョンのサポートを打ち切ります。
- **バンプが必要なケース**: 既存メッセージの意味変更、必須フィールドの追加、enum variant の削除・リネームの場合のみです。新しいフィールドは `#[serde(default)]` を使うことで、バージョンバンプなしに新旧のピア間で互換性を保てます。
- **機能ゲート**: ホストはネゴシエート済みバージョンを `ene-plugin-host` の `IpcPluginConnection` に保持し、`negotiated_version()` で参照できます。最小サポートバージョンより後に追加されたメッセージに依存する挙動はこれをもとにゲートすべきです。たとえば `supports_cancel_stream()` は v4 で追加された `PluginIpcRequest::CancelStream` をゲートしており、v3 のプラグインには理解できないメッセージを送らず、既存のタイムアウトベースのストリーム終了にフォールバックします。
- **ネゴシエーション失敗の診断**: プラグインが提示する範囲とホストのサポート範囲が重ならない場合、プラグイン側の `HandshakeAck` エラーおよびホスト側の `PluginHostError::HandshakeFailed` / `ProtocolMismatch` はいずれも双方の範囲を明記します（例: "host supports 3..=4, plugin supports 2..=2"）。これにより、単なる汎用的なハンドシェイク失敗ではなく、プラグインバイナリの再ビルドが必要であることが開発者に伝わります。

---

## 3. 組み込みプラグインカタログ

| プラグインバイナリ | ネームスペース | 説明 | ステートフル？ |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | システムアプリ起動・ウィンドウ制御 | いいえ |
| `ene-plugin-browser` | `browser.*` | ヘッドリス Chrome/CDP ブラウザ自動化 | はい (セッションストア) |
| `ene-plugin-fs` | `fs.*` | サンドボックス化ファイル操作 & Undo 履歴 | はい (DB IPC ソケット) |
| `ene-plugin-utility` | `utility.*` | 電卓、日時、TODO リスト管理 | はい (DB IPC ソケット) |
| `ene-plugin-web` | `web.*` | Web 検索および Markdown ページ抽出 | いいえ |
| `ene-plugin-anthropic` | Provider | Anthropic Claude プロバイダプラグイン | いいえ |

---

## 4. MCP (Model Context Protocol) 連携

`ene-connector` および `ene-plugin-host` は外部 MCP サーバーをシームレスに統合します：

1. **発見と起動**: ホストは `plugins.mcp_servers` 設定を読み込み、 `stdio` または HTTP/SSE 経由で対象の MCP サーバーバイナリを起動します。
2. **ツール変換**: MCP ツールは自動的に `ToolSpec` アイテムに変換され、 `CompositeToolRegistry` に登録されます。
3. **実行ルーティング**: LLM から生成されたツール呼び出しは MCP ブリッジを経由してルーティングされ、 `ene-runtime` に返却されます。

---

## 5. カスタムツールプラグインの開発

開発者は `ene-tool-sdk` の `#[derive(ToolAction)]` と `ene-plugin` のサーバーエントリポイントを使用して独自のツールプラグインを作成できます。以下は説明用のスケッチです — 実際にコンパイルが通る現行パターンは `plugins/tool/*` 配下の既存プラグイン (例: `plugins/tool/app/src/main.rs`) や `cargo doc -p ene-tool-macros --open` を参照してください：

```rust,ignore
use ene_tool_sdk::prelude::*;

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
