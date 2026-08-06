# `ene-plugin-db` インターフェース

## 役割

**プラグインバイナリ**向けの型付き CRUD データベース API。ホストの
`memory.db` にホストサービス `db` パッセンジャー経由で接続します。
機能非依存: テーブル・行・値を知るだけで、ビジネス意味は知りません。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `client` | `DbClient`・`DbError` |
| `messages` | `DbRequest`・`DbResponse`・`DbWriteOp`・`DbBatchOpResult`・`DbErrorCode` |
| `types` | `DbSchema`・`DbTable`・`DbColumn`・`DbType`・`DbFilter`・`DbValue`・`DbIndex`・`DbOrderBy`・`DbOrderDirection`・`Row` |

## 主要 API

- `DbClient::connect_with_token(socket, token)` — ホストサービスへ接続。
  `connect` はテスト用。
- `declare_schema(schema)` — プラグインのテーブルスキーマを登録
  （プレフィックス + テーブル + インデックス）。
- `insert` / `update` / `delete` / `search` — プラグイン自身のテーブルへの
  型付き CRUD。
- `batch(ops)` — `DbWriteOp` のリストを 1 つの SQLite トランザクションで
  適用（全か無か）。

## 依存関係

- 依存: `ene-plugin-proto`（ワイヤ型）。
- 利用: `ene-store`（サーバー側）・状態保持プラグインバイナリ
  （`plugins/tool/counter`・`calendar`・`utility` など）。

## リファクタリングの注目点

- **プレフィックス分離がセキュリティモデル**です。サーバーはプラグインが
  宣言プレフィックス配下のテーブルにしか触れないようにします。チェックは
  サーバー側・トークン認証のままにしてください。
- ワイヤ互換ルール（クレートドキュメント参照）: DB チャネルはバージョン
  フィールドを持ちません。`Batch` は*追加的*拡張でした。リクエスト/レスポンス
  enum は追加のみで拡張してください（新しい任意フィールドには
  `#[serde(default)]`）。
- `DbValue` はプラグイン向けの値言語です。`ene-core` ドメイン型との変換は
  ここではなく `ene-store` で行います。
