# `ene-connector`

> **クレート**: `ene-connector` | **役割**: 外部サービスコネクタの認証情報とアイデンティティの権威

`ene-connector` は外部サービスへ接続する際の*認証情報*側を担います: 安全な OAuth2 / API キーの記憶域 (シークレットは `Debug`/`Serialize` から秘匿され、ドロップ時に消記され、ログに出力されることもありません)、安定したコネクタ識別子 (`ConnectorId`)、OAuth 許可スコープ、表示用メタデータ (`ConnectorIdentity`)、そしてホストのメモリ内認証情報保管庫 `CredentialVault` (ストレージキーで索引されたエントリと有界な監査記録) を提供します。特定の外部サービスを実装せず、接続ライフサイクルも担いません — プロセス監督、再起動、ヘルスプローブは `ene-plugin-host` にあります。最初の具体的な利用クレートは `ene-plugin-host` のホストサービス `credential` 乗客サービスで、プラグインを認証し、IPC 経由で vault のエントリを解決します。プラグイン向けクライアント API はそのチャネルの上に構築されます。

---

## アーキテクチャ境界

- `ene-connector` は `CredentialStore` / `CredentialData` / `AccountCredentials`、`CredentialVault` / `VaultEntry` / `CredentialAuditLog` / `TokenRefresher`、`ConnectorId`、`PermissionScope`、`ConnectorIdentity`、`ConnectorError` を定義します。MCP 固有、ツール変換、プロセス監督のロジックは持ちません。
- 具体的な統合は利用側のクレート (`ene-plugin-host`) に実装され、このフレームワークを特定の外部プロトコルから切り離しています。
- **依存の向き**: `ene-connector` は意図的に `ene-config` や `ene-plugin-proto` に**依存しません** (#308)。認証情報の型を通じてその重量を露出させると、それらを参照するすべてのプラグインへ波及してしまいます。代わりに `ene-plugin-host` が connector と proto の両方を知るクレートです (#412)。

## 設計思想

- **なぜ認証情報を秘匿しドロップ時に消記するか**: `CredentialStore` は OAuth トークンと API キーを保持しますが、これらはプロセスメモリ、ログ、あるいは誤ったシリアライズに残留してはいけません。生の素材へは明示的で監査可能な `expose_for_persistence()` 経路を通じてのみ到達できます。
- **なぜ vault がサーバー側かつフェイルクローズなのか**: スコープ強制は vault の責務ではありません — credential 乗客サービスは要求された id をプラグインの登録済み `x-ene-credentials` 宣言 (`parse_credentials` / `resolve_scope` で解析) と照合し、解決されたストレージキーだけを vault に渡します。宣言のないプラグインは何も要求できず、存在しない・期限切れの資格情報は秘密を含まない表示用メタデータだけを載せた構造化エラーになります。
- **なぜ `Connector` ライフサイクル層を撤去したか (#416)**: 以前の版には `Connector` トレイトと `ConnectorRegistry` がありましたが、本番の MCP 経路に一度も配線されず、`ene-plugin-host` が既に提供する監督機構と二重化していました。これらは削除され、MCP ブリッジの SSRF URL 検証は `ene-plugin-host` の `mcp_registry` へ移設されました。
- **ポリシー型の行き先**: かつての `policy.rs` (リトライ / レート制限 / タイムアウト) はライフサイクル層とともに削除されました。クライアント側の認証情報ポリシーは #413 のもと `ene-plugin` に再導入されます。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-connector --open
```

`CredentialStore`、`CredentialVault`、`ConnectorId` から始めてください。

---

## 関連リンク
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [システムアーキテクチャ](../architecture.md)
