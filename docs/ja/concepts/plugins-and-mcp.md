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
- **ハンドシェイクネゴシエーション**: ホストが `PluginIpcRequest::Handshake { version_range: VersionRange { min: 4, max: 4 } }` を送信します。プラグインはバージョンをネゴシエートし、 `HandshakeAck { version: 4, capabilities: PluginCapabilities }` で応答します。
- **リクエスト相関**: 非同期リクエストおよびレスポンスはすべて必須の `request_id` (`Uuid`) を保持します。
- **ケーパビリティ**: プラグインはサポートする機能 (`tools`, `llm_providers`, `stt_providers`, `tts_providers`) を宣伝します。

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

開発者は `ene-plugin` と `#[derive(ToolAction)]` を使用して独自のツールプラグインを迅速に作成できます：

```rust
use ene_plugin::prelude::*;

#[derive(Debug, Deserialize, ToolAction)]
#[tool_action(name = "custom.greet", description = "パーソナライズされた挨拶文を生成します。")]
pub struct GreetAction {
    pub name: String,
}

impl GreetAction {
    pub async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("こんにちは、{}さん！", self.name))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ActionSetProvider::new().register::<GreetAction>();
    run_plugin_server(Box::new(ToolPluginAdapter(provider))).await?;
    Ok(())
}
```
