# `ene-store` インターフェース

## 役割

SQLite/SeaORM の**唯一の所有者**: スキーマ・マイグレーション・エンティティ・
ベクトル検索（`sqlite-vec`）・バックアップ・監査ログ・プラグイン向け DB IPC
サーバー。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `store` | `MemoryStore`（open・メモリ/感情/約束/セッション/ツール/ワークスペース操作）・会話ログ型 |
| `typed_memory` | `MemoryItem`・`Query`・`MemoryKind` など（`ene-core` から再エクスポート）・ストア側検索 DTO |
| `entities` | SeaORM エンティティ構造体（テーブルごとに 1 つ: `typed_memories`・`memory_embeddings`・`sessions`・`conversation_logs`・`commitments`・`schedules`・`audit_log` など） |
| `migrator` | バージョン付き SeaORM マイグレーション（`MigratorTrait`） |
| `port` | `impl MemoryPort for MemoryStore` と他の `ene-core` ポート |
| `db_server` / `host_service` | `db` パッセンジャー: `ene-plugin-db` クライアント向け IPC リクエスト処理 |
| `backup` | `OpenOptions`・`list_backups`・`restore_database` |
| `export` | `SessionExport`・`ExportedMessage`・`SESSION_EXPORT_FORMAT_VERSION`・`redact_secrets` |
| `audit` | `AuditEntry`・`NewAuditEntry`・`AuditDecision`・リダクションヘルパー |
| `affect`・`commitment`・`schedule`・`session` | 各領域のストア側ドメインモデル |
| `search`・`forgetting`・`config`・`error` | ハイブリッドスコアリングヘルパー・ライフサイクル遷移・`StoreConfig`・`EneMemoryError` |

## 主要な型

- `MemoryStore` — 具象ストア。`ene_core::MemoryPort` と他のポートトレイトを
  実装。
- `EneMemoryError` — `#[non_exhaustive]` のエラー enum。新しい内部バリアント
  は自動的に `PublicApiError` へ射影されます（[API v1](../architecture/api-v1.md)参照）。
- `StoreConfig` — `store.enabled` などのトグル。
- `SessionMeta`・`NewSessionMeta` — セッション一覧メタデータ
  （`PublicSessionMeta` にミラー）。

## 依存関係

- 依存: `ene-config`・`ene-core`・`ene-rag`（スコアリング核）・
  `ene-plugin-db`/`ene-plugin-proto`（DB IPC ワイヤ型）。
- 利用: `ene-runtime`・`ene-cli`・`ene-desktop`・`db` ホストサービス経由の
  プラグインバイナリ・`ene-mind` テスト（dev）。
- 明示的に**依存しない**: `ene-ai`・`ene-mind`・`ene-runtime`。

## リファクタリングの注目点

- **スキーマ変更 = マイグレーション。** エンティティは SeaORM。テーブル変更は
  `migrator` に新しいマイグレーションを追加し、古いものを編集しません。
- **DB IPC 契約**（`ene-plugin-db` 経由）は追加のみ。`DbRequest`/
  `DbResponse` のバリアント・フィールドは増えるだけにしてください
  （[ene-plugin-db](ene-plugin-db.md)参照）。
- ストアは埋め込みベクトルを入力として受け取ります。埋め込みは呼びません。
  `ene-ai` の import を `ene-store` に持ち込まないでください — この依存方向
  こそが設計の要点です。
- `memory.db` のファイル形式はバージョン管理され、バックアップ/復元・
  整合性チェックが運用者向けインターフェースの一部です
  （CLI `/store`・`ene store`）。
- リダクション（`redact_secrets`・`redact_arguments`）はストア境界で適用され、
  シークレットがログやエクスポートに到達しません。そこに置き続けてください。
