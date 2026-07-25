# `ene-connector` — API リファレンス

> **クレート**: `ene-connector` | **役割**: 外部サービス連携および MCP ブリッジのための共有コネクタフレームワーク

`ene-connector` は、外部サービス（カレンダー、MCP、GitHub など）へのコネクタを構築し、Model Context Protocol (MCP) サーバーを Ene にブリッジするための再利用可能な共通フレームワークを提供します。

---

## 主要コンポーネント

### 1. フレームワーク抽象化
- **`Connector` トレイト**: すべての外部サービスコネクタが実装する統一されたライフサイクルトレイト。
- **`ConnectorRegistry`**: アクティブなコネクタインスタンスを管理するスレッドセーフなレジストリ。
- **`CredentialStore`**: ドロップ時にメモリ消記（Zeroize）を行うセキュアな OAuth2 トークンおよび API キー記憶域。
- **ポリシー**: 再利用可能な `RetryPolicy`、`RateLimiter`、`TimeoutPolicy`、および `PaginationCursor`。

### 2. MCP (Model Context Protocol) ブリッジ
- `stdio` または HTTP SSE 上で動作する外部 MCP サーバーに接続。
- MCP ツールスキーマを Ene の `ToolSpec` 定義に変換。
- LLM からのツール呼び出しを対象の MCP サーバーへ転送し結果を返却。

---

## 関連リンク
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [システムアーキテクチャ](../architecture.md)
