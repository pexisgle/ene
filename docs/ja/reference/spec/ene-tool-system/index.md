# ツールシステムコア概要 & アーキテクチャ

Ene は、コンパニオンが外部システムと協調するための柔軟かつ安全なツールシステムを備えています。セキュリティと安定性を保証するため、すべてのツール実行はホストプロセスから分離され、**別プロセスの Sandbox 環境**で実行されます。本ドキュメントでは、ツールシステムの全体像、物理クレートの責務、および 2レイヤー ABI の概要を定義します。

---

## 1. ツールシステムを構成するクレート群

ツールシステムは、機能と階層構造に応じて以下の6つの専用クレートに分割されています。

| クレート名 | フォルダ | 責務と依存関係 |
|---|---|---|
| `ene-plugin-proto` | `crates/ene-plugin-proto` | **ワイヤープロトコル層**。IPCメッセージ定義、ソケットトランスポート、`ToolProvider` トレイト、Sandbox構造体データ。 |
| `ene-plugin-host` | `crates/ene-plugin-host` | **オーケストレーション層**。ツールプロセスの起動・管理、環境変数によるIPCセキュリティトークンの伝播、MCP（Model Context Protocol）接続。 |
| `ene-tool-rag` | `crates/ene-tool-rag` | **ツール回収（RAG）層**。LLMの文脈に合わせた最適なツールの検索、多ベクトル類似度、HyDE、LLMリランク。 |
| `ene-tool-common`| `crates/ene-tool-common`| **ツールユーティリティ層**。ツールバイナリ用の共通コード、HTML-to-markdown変換など。 |
| `ene-tool-derive`| `crates/ene-tool-derive`| **コード生成マクロ**。`#[derive(ToolSpec)]` によるスキーマの自動生成。 |
| `ene-tool-db` | `crates/ene-tool-db` | **ツール用データベースプロキシ**。ソケット経由での安全なSQLite CRUDクライアント。 |

---

## 2. 2レイヤー ABI 設計 (Two-Layer ABI)

Ene のツールインターフェースは、セキュリティ保護と統合モデルのため、ワイヤー（Wire）層とホスト（Host）層の2層構造に明確に分離されています。

1.  **ワイヤー (Wire) レイヤー (`ene-plugin-proto`)**:
    -   ツールバイナリ自身、または外部のIPC接続先が実装すべき物理的な境界定義。
    -   `ToolProvider` トレイトがこれに相当し、`call_tool` によるJSON文字列の受け渡しのみを行います。
2.  **ホスト (Host) レイヤー (`ene-plugin-host`)**:
    -   `ene-runtime` にリンクされ、ツール全体を束ねてLLMに提示・実行する制御部分。
    -   `ToolRegistry` トレイトがこれに相当し、IPCツール、内蔵（Built-in）ツール、および外部の MCP サーバーを透過的にまとめて単一のレジストリとして振る舞わせます。

---

## 3. 名前衝突のフェイルクローズ処理 (Collision Hard-Error)

複数のツールやMCPサーバー間で同一の名前（例: `fs.read`）が登録されると、LLMによるツールの誤認が発生し、不正なコマンド実行（特権昇格やデータ破損）につながる恐れがあります。これを防ぐため、Ene は各レイヤーで衝突を検知した場合は**警告なしでシステムを即座にクラッシュ（フェイルクローズ）**させます。
*   `HostRegistry` への登録時に名前が重複した場合は、`ToolError::DuplicateName` でエラー。
*   `CompositeToolRegistry` での登録時は `ToolHostError::DuplicateToolName` でアクター起動失敗。
