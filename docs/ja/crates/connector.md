# `ene-connector`

> **クレート**: `ene-connector` | **役割**: 外部サービスコネクタの認証情報とアイデンティティの権威

`ene-connector` は外部サービスへ接続する際の*認証情報*側を担います: 安全な OAuth2 / API キーの記憶域 (シークレットは `Debug`/`Serialize` から秘匿され、ドロップ時に消記され、ログに出力されることもありません)、安定したコネクタ識別子 (`ConnectorId`)、OAuth 許可スコープ、および表示用メタデータ (`ConnectorIdentity`)。特定の外部サービスを実装せず、接続ライフサイクルも担いません — プロセス監督、再起動、ヘルスプローブは `ene-plugin-host` にあります。これらの型はまだ利用クレートがありません: MCP ブリッジ (SSRF URL 検証を含む) は既に `ene-plugin-host` の `mcp_registry` に実装されており、本クレートを利用する認証情報側は #412 のもと `ene-plugin-host` に再導入され、それを支える認証情報の保管庫 (vault) は #415 でここに実装されます。

---

## アーキテクチャ境界

- `ene-connector` は `CredentialStore` / `CredentialData` / `AccountCredentials`、`ConnectorId`、`PermissionScope`、`ConnectorIdentity`、`ConnectorError` を定義します。MCP 固有、ツール変換、プロセス監督のロジックは持ちません。
- 具体的な統合は利用側のクレート (`ene-plugin-host`) に実装され、このフレームワークを特定の外部プロトコルから切り離しています。
- **依存の向き**: `ene-connector` は意図的に `ene-config` や `ene-plugin-proto` に**依存しません** (#308)。認証情報の型を通じてその重量を露出させると、それらを参照するすべてのプラグインへ波及してしまいます。代わりに `ene-plugin-host` が connector と proto の両方を知るクレートです (#412)。

## 設計思想

- **なぜ認証情報を秘匿しドロップ時に消記するか**: `CredentialStore` は OAuth トークンと API キーを保持しますが、これらはプロセスメモリ、ログ、あるいは誤ったシリアライズに残留してはいけません。生の素材へは明示的で監査可能な `expose_for_persistence()` 経路を通じてのみ到達できます。
- **なぜ `Connector` ライフサイクル層を撤去したか (#416)**: 以前の版には `Connector` トレイトと `ConnectorRegistry` がありましたが、本番の MCP 経路に一度も配線されず、`ene-plugin-host` が既に提供する監督機構と二重化していました。これらは削除され、MCP ブリッジの SSRF URL 検証は `ene-plugin-host` の `mcp_registry` へ移設されました。
- **ポリシー型の行き先**: かつての `policy.rs` (リトライ / レート制限 / タイムアウト) はライフサイクル層とともに削除されました。クライアント側の認証情報ポリシーは #413 のもと `ene-plugin` に再導入され、認証情報の保管庫 (vault) と OAuth フローは #415 でここに実装されます。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-connector --open
```

`CredentialStore` と `ConnectorId` から始めてください。

---

## 関連リンク
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [システムアーキテクチャ](../architecture.md)
