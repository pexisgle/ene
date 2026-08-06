# `ene-connector` インターフェース

## 役割

外部サービス連携（カレンダー・GitHub・Discord など）のための安全なコネクタ
フレームワーク: コネクタライフサイクル・資格情報・権限ゲート・トランスポート
ポリシー・webhook 検証。意図的に `ene-config` / `ene-plugin-proto` 非依存で、
プラグインバイナリが設定/プロトコルのスタックなしで資格情報型を使えます。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `connector` | `Connector` トレイト・`ConnectorAction`・`ConnectorStatus`・`ConnectorSummary`・`HealthStatus`・`ConnectionState`・`AuthenticatedAccount`・`AccountAuthKind`・`PermissionGrant`・`actions` |
| `registry` | `ConnectorRegistry`・`ConnectorEvent(Kind)`・`AccountRef` |
| `credential` | `CredentialStore`・`CredentialData`・`AccountCredentials`（Debug/Serialize からリダクションされ、drop 時にゼロ化） |
| `identity` | `ConnectorId`（`namespace.name`）・`CredentialId`・`PermissionScope`・`ConnectorIdentity` |
| `gate` | `PermissionGate`（フェイルクローズ・アクション別） |
| `policy` | `ConnectorPolicy`・`RetryPolicy`・`RateLimitPolicy`・`PaginationPolicy`・`Page`・`RateLimiter`・バックオフ/リトライ/ページネーションヘルパー |
| `webhook` | `WebhookValidator`（HMAC + リプレイウィンドウ） |
| `redaction` | `redact_json`・`scrub_secrets` |
| `declaration` | `CredentialDeclaration`・`CredentialKind`・`parse_credentials`・`resolve_scope`・`ScopeDecision`・拒否/劣化資格情報型 |
| `error` | `ConnectorError` |

## 依存関係

- 依存: 内部なし。
- 利用: `ene-plugin-host`（資格情報 ID/解決）・`ene-runtime`
  （`ConnectorHandle`）・`ene-cli`・`ene-desktop`・コネクタ消費プラグイン
  （`plugins/tool/calendar` など）。

## リファクタリングの注目点

- 非依存ルールは意図的です（クレートドキュメント参照）。ここに
  `ene-config` や `ene-plugin-proto` を足すと、資格情報型を見る全プラグインへ
  その重みが伝播します。
- シークレット規律はインターフェースの一部です。資格情報値は生データを持つ
  `Debug`/`Serialize` を実装せず、唯一の出口は監査付き
  `expose_for_persistence` です。
- `PermissionGate` の意味論（デフォルト拒否・ターン単位付与・会話単位
  パターン）は破壊的外部操作の安全契約です。監査ログと合わせて変更して
  ください。
