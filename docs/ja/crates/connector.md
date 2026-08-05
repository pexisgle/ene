# `ene-connector`

> **クレート**: `ene-connector` | **役割**: 外部サービス連携のためのセキュアなコネクターフレームワーク

`ene-connector` はコネクターのライフサイクルと共通基盤を担います:
`Connector` トレイトと `ConnectorRegistry`（登録 / 疎通確認 / 接続 / 切断 /
アクション単位の権限付与 / 状態表示）、共通の転送ポリシー（タイムアウト、
バックオフ再試行、レートリミット、ページネーション）、Webhook 検証、
構造的シークレット除去、fail-closed なアクション単位の `PermissionGate`、
そして安全な OAuth2 / API キーの記憶域（シークレットは
`Debug`/`Serialize` から秘匿され、ドロップ時にゼロ化され、決して
ログに出力されません）。特定の外部サービスは実装せず、具体的な
コネクターは別の追従機能として、ランタイムの
`EneHandle::connectors()` を通じて登録されます。

---

## アーキテクチャ境界

- `ene-connector` は `CredentialStore` / `CredentialData` / `AccountCredentials`、`ConnectorId`、`PermissionScope`、`ConnectorIdentity`、`ConnectorError`、`Connector` トレイト、`ConnectorRegistry`、`PermissionGate`、ポリシーヘルパー、`WebhookValidator` を定義します。MCP 固有、ツール変換、プロセス監督のロジックは持ちません。
- 具体的な統合は利用側のクレート (`ene-plugin-host`) に実装され、このフレームワークを特定の外部プロトコルから切り離しています。
- **依存の向き**: `ene-connector` は意図的に `ene-config` や `ene-plugin-proto` に**依存しません**。認証情報の型を通じてその重量を露出させると、それらを参照するすべてのプラグインへ波及してしまいます。代わりに `ene-plugin-host` が connector と proto の両方を知るクレートです。

## 設計思想

- **なぜ認証情報を秘匿しドロップ時に消記するか**: `CredentialStore` は OAuth トークンと API キーを保持しますが、これらはプロセスメモリ、ログ、あるいは誤ったシリアライズに残留してはいけません。生の素材へは明示的で監査可能な `expose_for_persistence()` 経路を通じてのみ到達できます。
- **なぜライフサイクル層を再導入したか**: 以前の版には `Connector` トレイトと `ConnectorRegistry` がありましたが、本番経路に一度も配線されず削除されました。今回はランタイムハンドル、CLI、デスクトップの状態ページ、テストが消費する形でエンドツーエンドに配線され、パーミッション・監査統合も実装されています。
- **ポリシー型の場所**: `policy.rs`（リトライ / レート制限 / タイムアウト / ページネーション）は再びフレームワークの一部です。認証情報の保管庫 (vault) と OAuth フローは追従機能として計画されています。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-connector --open
```

`Connector`、`ConnectorRegistry`、`CredentialStore` から始めてください。

## 開発者ガイド

コネクターの書き方と秘密情報の取り扱い契約については
[コネクター](../concepts/connectors.md) を参照してください。

---

## 関連リンク
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [システムアーキテクチャ](../architecture.md)
