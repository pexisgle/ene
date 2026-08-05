# コネクターフレームワーク

コネクターフレームワーク（`ene-connector`）は、外部サービス連携に共通する
基盤を提供し、各コネクターはサービス固有の実装だけを行います。認証情報の
管理、パーミッション確認、再試行、レートリミット、タイムアウト、
ページネーション、Webhook 検証、機密情報のフィルタリング、監査ログ、
状態表示を共通化します。

## フレームワークの構成

- `crates/ene-connector` がフレームワーク本体です:
  - `Connector` トレイト — 識別情報、宣言されたアクション、転送ポリシー、
    ライフサイクル操作（`check_connectivity` / `connect` / `disconnect` と、
    レジストリが管理する状態スナップショット）。
  - `ConnectorRegistry` — 共通 API: 登録 / 登録解除 / 一覧 / 状態 /
    疎通確認 / 接続 / 切断 / 権限付与 / 権限取り消し / 権限状態表示。
    ライフサイクル操作はすべてコネクターのタイムアウトでラップされ、
    状態の読み取りは I/O なしのキャッシュスナップショットです。
  - `PermissionGate` — ツール権限センターと共通の fail-closed な
    アクション単位パーミッションモデル。ターン単位の一度きり承認と、
    会話単位の `(action, target-prefix)` パターン。
  - `policy` — タイムアウト、指数バックオフ再試行（ジッター付き）、
    トークンバケット方式のレートリミット、カーソル方式ページネーション。
  - `webhook` — HMAC-SHA256 署名検証とリプレイ防止ウィンドウ。
  - `redaction` — イベント・監査・エラー境界で適用される構造的
    シークレット除去。
  - `CredentialStore` — OAuth2 / API キー保存。秘密情報は
    `Debug`/`Serialize` から除去され、ドロップ時にゼロ化されます。
- `ene-runtime` が配線を担当します: `EneHandle::connectors()` が
  `ConnectorHandle`（登録・ライフサイクル・問い合わせ）を公開し、
  コネクター操作はアクターメールボックスを経由するため、パーミッション
  プロンプトは既存の権限センターで解決され、監査行は既存のツール権限
  監査トレイル（`connector.<id>.<op>` ツール名）に記録されます。
  状態変更は `LifecycleEvent::ConnectorChanged` でブロードキャストされます。
- CLI: `/connector list|status|check|connect|disconnect|grant|revoke|permissions`。
- Desktop: **コネクター**設定ページで、キャッシュされた状態・ヘルス・
  アカウント・権限付与を表示し、疎通確認ボタンを提供します。

具体的なコネクター（Discord、Slack、GitHub など）は別の追従機能です。
フレームワーク自体は組み込みコネクターを持ちません。

## コネクターの書き方

1. `Connector` を実装します:

   ```rust
   struct MyConnector {
       identity: ConnectorIdentity,
   }

   #[async_trait]
   impl Connector for MyConnector {
       fn identity(&self) -> &ConnectorIdentity { &self.identity }

       fn actions(&self) -> &'static [ConnectorAction] {
           &[ConnectorAction::side_effecting("send_message", "Send a message")]
       }

       fn policy(&self) -> ConnectorPolicy {
           ConnectorPolicy::default()
               .with_timeout(Duration::from_secs(10))
               .with_retry(RetryPolicy::new(4, Duration::from_secs(1), Duration::from_secs(8)))
               .with_rate_limit(RateLimitPolicy::new(10, Duration::from_secs(1)))
       }

       async fn check_connectivity(&self) -> Result<HealthStatus, ConnectorError> { /* … */ }
       async fn connect(&self, credential: &AccountCredentials)
           -> Result<Vec<AuthenticatedAccount>, ConnectorError> { /* … */ }
       async fn disconnect(&self, account: &AuthenticatedAccount)
           -> Result<(), ConnectorError> { /* … */ }
   }
   ```

2. ランタイムに登録します:

   ```rust
   handle.connectors().register(Arc::new(MyConnector::new()))?;
   ```

3. HTTP 呼び出しの内部でポリシーヘルパーを使います: `retry_with_backoff`
   は一時的な失敗（`Transport` / `Io` / `RateLimited`）のみ再試行します。
   `RateLimiter::acquire` はバーストを制限し、`collect_pages` は
   `PaginationPolicy::max_pages` までカーソルページを収集します。
   一連の処理全体を操作タイムアウトでラップし、サービスが固まっても
   操作境界を超えて走り続けないようにします。

4. ユーザーに表示されるアクションはすべて `actions()` で宣言します —
   未宣言のアクションは `grant` で拒否され、権限状態表示にも載りません。

5. カスタムアクション（フレームワーク自身がゲートする `connect` /
   `disconnect` 以外）の実施は実装側の責務です: 登録後に
   `registry.gate(id)` でコネクターのゲートを取得し、サービスに触れる
   前に各アクション内で `gate.check(action, target, description)` を
   呼び出してください。これによりアクション単位の権限付与・取り消しが
   カスタムアクションにも適用されます。

## 秘密情報の取り扱い契約

- 秘密情報は `AccountCredentials` / `CredentialStore`
  （`SecretString`）経由でのみ扱います: `Debug`/`Serialize` から除去され、
  ドロップ時にゼロ化されます。CLI は `ENE_CONNECTOR_<ID>_API_KEY`
  （ID の `[A-Za-z0-9]` 以外は `_` に置換）から API キーを読み取り、
  値をエコー・ログ出力しません。
- 生の秘密情報を状態メッセージ・イベント・エラー文字列・説明文に
  **決して**入れないでください。エラーは固定文字列と識別子から構築します。
  `redaction::scrub_secrets` はレジストリのイベント・監査境界に
  防御的追加として適用されますが、上記の契約の代わりにはなりません。
- 監査行は設計上引数を持たず（`{}`）、ストア側でも引数 JSON を
  第二層として除去します。

## パーミッションと監査

- `connect` / `disconnect` はコネクターの `PermissionGate` によって
  デフォルト拒否でゲートされます。拒否時は
  `ConnectorError::PermissionRequired { request_id, action, target, description }`
  を返し、ランタイムはツールと同じ権限センターでプロンプト
  （一度きり / セッション許可 / 拒否）を表示します。一度きりの承認は
  ターン境界で失効し、セッションパターンは会話境界で失効します。
  セッション許可は `/permissions` にも表示され、中央から取り消せます。
- 明示的な `grant` / `revoke` はアクション単位のパターンを記録・削除し、
  `/connector permissions <id>` で表示できます。
- すべてのコネクター操作は、パーミッション決定・アクション・ターゲット・
  結果を含む監査行（`connector.<id>.<op>`）を 1 件記録します。

## CLI リファレンス

| コマンド | 効果 |
|---|---|
| `/connector list` | 登録済みコネクターとキャッシュされた状態 |
| `/connector status <id>` | ヘルス・接続状態・アカウント |
| `/connector check <id>` | 疎通確認（読み取り専用） |
| `/connector connect <id>` | 認証（`ENE_CONNECTOR_<ID>_API_KEY` を読み取る） |
| `/connector disconnect <id> [account]` | アカウントのセッションを切断 |
| `/connector grant <id> <action> <target>` | アクション単位の権限を記録 |
| `/connector revoke <id> <action> <target>` | アクション単位の権限を削除 |
| `/connector permissions <id>` | 権限付与の一覧 |

## 状態表示

`list` / `status` はキャッシュされたスナップショットを読み取り、
ネットワークに触れません。`check` だけがサービスをプローブします。
状態はチェック・接続・切断のたびにレジストリが更新し、失敗は
`ConnectionState::Error`（除去済みの詳細付き）で表示されます。
