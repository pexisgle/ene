# MCP サーバー設定ガイド

Ene は、任意の Model Context Protocol (MCP) サーバー — `stdio` で起動するローカルプロセス、または HTTP で接続するリモートエンドポイント — に接続し、そのツールをキャラクターに公開できます。このガイドでは、Calendar / Mail・Chat / Notes / Map / RSS サービス向けに MCP サーバーを設定する手順を説明します。基盤となる仕組みは汎用で、すでに目的のサービス用の MCP サーバーをお持ちなら、以下の設定形式をそのまま利用できます。

> 例で挙げるサードパーティ製 MCP サーバーは npm / レジストリのパッケージ名です。これらはコミュニティまたはベンダーがメンテナンスするプロジェクトであり、Ene のコードではありません。パッケージ名・環境変数・認証フローは変わり得るため、各プロジェクトの README で最新のセットアップ手順を確認してください。

---

## 1. MCP サーバーの宣言方法

MCP サーバーは `settings.json` の `plugins.mcp_servers` に記述します（ファイルの場所と設定の優先順位については、[設定リファレンス](../../configuration.md) を参照）：

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "my-server",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@some/mcp-server"]
        },
        "env_passthrough": ["SOME_API_KEY"]
      }
    ]
  }
}
```

| フィールド | 型 | 説明 |
|---|---|---|
| `name` | string | ルーティングとツールの名前空間に**そのまま**使われます。ハイフンなど他の文字も利用できます。 |
| `enabled` | bool | **必須。** `false` にするとエントリを残したままサーバーをスキップします。 |
| `transport.type` | `"stdio"` \| `"http"` | **必須。** stdio は子プロセスを起動、http はリモートエンドポイントに接続します。 |
| `transport.command` / `transport.args` | string / string[] | stdio のみ：起動するプロセス（例：`npx` と `-y <package>`）。 |
| `transport.url` | string | http のみ：サーバーのエンドポイント。 |
| `transport.auth_header` | string | http のみ、任意：`Authorization` ヘッダーとして送信されます（例：`"Bearer <token>"`）。値が不正な場合は接続自体が失敗します（認証なしへのサイレントなダウングレードは発生しません）。 |
| `env_passthrough` | string[] | stdio のみ：ホストプロセスの環境変数のうち、サーバーの子プロセスへ転送する名前（API キーなど）。 |

stdio トランスポートの 2 つの挙動は、シークレットを配線する前に知っておくべき点です：

- **子プロセスの環境はクリアされます。** 自動で転送されるのは `PATH`・`HOME`・`TMPDIR`・`LANG`・`TZ`・`LD_LIBRARY_PATH`（Linux）と一部の Windows 必須変数のみ。それ以外 — 特に API キー — は、(a) Ene を起動した環境にエクスポートし、(b) `env_passthrough` でホワイトリストに登録する必要があります。
- **`env` マップはありません**（他の一部の MCP クライアントとは異なり）：Ene はサーバーごとにインラインの環境変数を定義できません。Ene の起動前に変数をエクスポートし、`env_passthrough` に列挙してください。

HTTP トランスポートは既定で HTTPS のみを許可し、SSRF 対策としてループバックアドレス（`127.0.0.0/8`、`::1`）は拒否されます。自分のマシン上で動くサーバー（Obsidian の組み込み MCP エンドポイントなど）に接続するには、`plugins` 内で `"mcp_allow_insecure_urls": true` を設定します。これでプレーンな `http://` とループバックが許可されますが、リンクローカルアドレスは引き続き拒否されます。セキュリティモデルの詳細は[プラグインと MCP](../../concepts/plugins-and-mcp.md) を参照してください。

`mcp_servers` は配列のため、エントリは `settings.json` で宣言します — `ENE_` 環境変数で配列要素を追加することはできません。スカラー値のプラグイン設定は起動時に上書きできます（例：`ENE_PLUGINS__MCP_ALLOW_INSECURE_URLS=true`）。

サーバーを追加・編集したら Ene を再起動します。CLI の `/tool list` でサーバーのツールが表示されれば正常です（各 MCP ツールはサーバー名の下に表示されます）。接続に失敗したサーバーはログに記録されてスキップされるため、ツールは一覧から黙って消えます。読み込みを疑うときはログ出力を確認してください — 文言はトランスポートごとに異なり、stdio の失敗は "MCP server failed to connect"、HTTP の失敗は "MCP HTTP connection failed" です。

## 2. 前提条件

- `npx` で起動するサーバー（後述の大半）には **Node.js + npm**。
- `uvx` で起動するサーバー（CalDAV の亜種など）には **Python + uv**。
- 接続する各サービスのアカウントと API キー。
- OAuth ベースのサーバーは一度だけインタラクティブな認可が必要です。**Ene を設定する前に**、ターミナルでサーバーの `auth` 手順を手動で実行してください — Ene はサーバーを stdio パイプ経由で起動するため、ブラウザでの同意フローは Ene の外で完遂する方が確実です。

---

## 3. Calendar

### 3.1 Google Calendar — `@cocal/google-calendar-mcp`

Google Calendar API を利用する stdio サーバーです。

1. Google Cloud プロジェクトを作成し、**Calendar API** を有効化、OAuth 2.0 クライアント（種類：**デスクトップ アプリ**）を作成してクライアント JSON（`gcp-oauth.keys.json`）をダウンロードします。
2. 一度だけ認可を実行します：
   ```bash
   export GOOGLE_OAUTH_CREDENTIALS="/path/to/gcp-oauth.keys.json"
   npx @cocal/google-calendar-mcp auth
   ```
3. クレデンシャルファイルのパスを転送してサーバーを宣言します：

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "google-calendar",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@cocal/google-calendar-mcp"]
        },
        "env_passthrough": ["GOOGLE_OAUTH_CREDENTIALS"]
      }
    ]
  }
}
```

```bash
export GOOGLE_OAUTH_CREDENTIALS="/path/to/gcp-oauth.keys.json"
```

**認証：** クレデンシャルファイルによる OAuth 2.0。**トラブルシューティング：** `ENOENT ... gcp-oauth.keys.json` は変数が無いかパスが誤っていることを意味します — npx 実行時はファイルが自動検出されないため、環境変数の指定が必須です。Ene からトークンを更新できない場合は `auth` 手順をやり直してください。Google はホスト型 Calendar MCP エンドポイント（`https://calendarmcp.googleapis.com/mcp/v1`）も提供していますが、OAuth2 クライアントフローが必要で、Ene の HTTP トランスポート（静的な `Authorization` ヘッダー）では実行できないため、上記の stdio サーバーが対応ルートです。

### 3.2 CalDAV — `caldav-mcp`

CalDAV に対応したセルフホスト・プロバイダー製カレンダー（Nextcloud、ownCloud、Yandex Calendar、iCloud、FastMail など）向けです：

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "caldav",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "caldav-mcp"]
        },
        "env_passthrough": ["CALDAV_BASE_URL", "CALDAV_USERNAME", "CALDAV_PASSWORD"]
      }
    ]
  }
}
```

```bash
export CALDAV_BASE_URL="https://your-domain.com/remote.php/dav/calendars/yourname/"
export CALDAV_USERNAME="yourname"
export CALDAV_PASSWORD="your-password"
```

**認証：** ユーザー名・パスワードによる Basic 認証。**トラブルシューティング：** 多くのプロバイダーは**アプリ固有パスワード**（iCloud、Yandex）や完全なアカウント URL（Nextcloud：`/remote.php/dav/calendars/<user>/`）を要求します。Python 版の代替として `uvx mcp-caldav` もあります（`CALDAV_BASE_URL` ではなく `CALDAV_URL` を使用）。

---

## 4. Mail & Chat

### 4.1 Gmail — `@franciscpd/gmail-mcp-server`

1. Google Cloud Console で **Gmail API** を有効化し、OAuth 同意画面（スコープ：`https://mail.google.com/`）を設定、OAuth 2.0 クライアント（種類：**Web アプリケーション**）を作成します。
2. 承認コードをリフレッシュトークンに交換します（例：自分のクライアント ID / シークレットで OAuth 2.0 Playground を使用）。
3. サーバーを宣言します：

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "gmail",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@franciscpd/gmail-mcp-server"]
        },
        "env_passthrough": ["GMAIL_CLIENT_ID", "GMAIL_CLIENT_SECRET", "GMAIL_REFRESH_TOKEN"]
      }
    ]
  }
}
```

```bash
export GMAIL_CLIENT_ID="...apps.googleusercontent.com"
export GMAIL_CLIENT_SECRET="GOCSPX-..."
export GMAIL_REFRESH_TOKEN="1//0..."
```

**認証：** リフレッシュトークンによる OAuth 2.0（設定後のインタラクティブ操作は不要）。**トラブルシューティング：** 同意時に `Error 403: access_denied` が出る場合は、アカウントが同意画面のテストユーザーに登録されていません。Calendar と同様、Google のホスト型 Gmail MCP エンドポイント（`https://gmailmcp.googleapis.com/mcp/v1`）は OAuth2 クライアントフローが必要なため、Ene の HTTP トランスポートでは利用できません。

### 4.2 Slack — `@modelcontextprotocol/server-slack`

1. ワークスペースに Slack アプリを作成し、Bot トークンスコープ（`channels:history`、`channels:read`、`chat:write`、`reactions:write`、`users:read`、`users.profile:read`）を追加してインストールします。
2. **Bot ユーザー OAuth トークン**（`xoxb-…`）とワークスペース / チーム ID を控えます。

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "slack",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-slack"]
        },
        "env_passthrough": ["SLACK_BOT_TOKEN", "SLACK_TEAM_ID"]
      }
    ]
  }
}
```

```bash
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_TEAM_ID="T01234567"
# 任意：アクセスを制限するチャンネル ID のカンマ区切りリスト
export SLACK_CHANNEL_IDS="C01234567,C76543210"
```

**認証：** Bot トークン。**トラブルシューティング：** `SLACK_TEAM_ID` がないとサーバーはワークスペースを解決できません。到達可能なチャンネルを制限するには `SLACK_CHANNEL_IDS` を設定します（未設定時は全公開チャンネル）。

---

## 5. Notes

### 5.1 Obsidian — Local REST API プラグインの組み込み MCP エンドポイント

Obsidian に **Local REST API** コミュニティプラグインをインストールし、設定画面から API キーをコピーします。このプラグインは MCP エンドポイントを HTTP で提供します — Ene の HTTP トランスポート（ループバック許可込み）との相性が良い構成です：

1. プラグイン設定で **HTTP サーバー**（プレーン `http://` モード）を有効化します。
2. API キーを `Authorization` ヘッダーとしてサーバーを宣言します：

```jsonc
{
  "plugins": {
    "mcp_allow_insecure_urls": true,
    "mcp_servers": [
      {
        "name": "obsidian",
        "enabled": true,
        "transport": {
          "type": "http",
          "url": "http://127.0.0.1:27123/mcp/",
          "auth_header": "Bearer YOUR_API_KEY"
        }
      }
    ]
  }
}
```

**認証：** `Authorization` ヘッダーとして送信される Bearer API キー — Ene の `auth_header` の転送方法と一致します。**トラブルシューティング：** HTTPS エンドポイント（`https://127.0.0.1:27124/mcp/`）は自己署名証明書を使うため Ene の HTTP クライアントが拒否します。そのため `mcp_allow_insecure_urls: true` を設定したプレーン HTTP エンドポイントを使用してください（ループバックはネットワークから隔離されたままです。この設定がリンクローカルを緩めることはありません）。ツールが表示されない場合は、プラグインの HTTP サーバーのトグルがオンで Obsidian が起動しているか確認してください。代わりに stdio ブリッジ（`npx -y @connorbritain/obsidian-mcp-server`、`OBSIDIAN_API_KEY` を `env_passthrough` に指定）を使えば HTTP の許可設定を完全に回避できます。

### 5.2 Notion — `@notionhq/notion-mcp-server`

1. Notion インテグレーションを作成し、トークン（`ntn_…`）をコピーします。
2. ページ / データベースをインテグレーションと共有します。

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "notion",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@notionhq/notion-mcp-server"]
        },
        "env_passthrough": ["NOTION_TOKEN"]
      }
    ]
  }
}
```

```bash
export NOTION_TOKEN="ntn_..."
```

**認証：** インテグレーショントークン。**トラブルシューティング：** Notion のホスト型 MCP（`https://mcp.notion.com/mcp`）は OAuth 専用で Bearer トークンを受け付けないため、Ene の HTTP トランスポートでは接続できません — トークンを使う stdio サーバーが対応ルートです。インテグレーションと共有されていないページはツールから見えません。

---

## 6. Map

### Google Maps — `@modelcontextprotocol/server-google-maps`

1. Google Maps Platform の API キーを作成し、利用する API（Geocoding、Places、Routes、Distance Matrix など）を有効化して請求先を設定します。
2. 可能であればキーをそれらの API に限定します。

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "google-maps",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-google-maps"]
        },
        "env_passthrough": ["GOOGLE_MAPS_API_KEY"]
      }
    ]
  }
}
```

```bash
export GOOGLE_MAPS_API_KEY="AIza..."
```

**認証：** API キー。**トラブルシューティング：** ツール結果が `REQUEST_DENIED` になるのは、通常キーがプロジェクトで有効化されていない API に限定されているためです。このパッケージはリファレンス MCP サーバー集に由来し、上流はアーカイブされていますが、npm には公開されたままです。活発にメンテナンスされる代替もあります（例：HTTP streamable の `mcp-server-google-maps`。`GOOGLE_MAPS_API_KEY` を使用）。ただし独自の `X-Api-Key` ヘッダーを要求するものは、Ene の HTTP トランスポート（`Authorization` ヘッダーのみ送信）では認証できません — stdio サーバーを選ぶか、ローカル専用利用でサーバー側の認証トークンを省略してください。

---

## 7. RSS

### RSS — `rss-mcp`

RSSHub ベースのリーダーです（RSS・Atom に加え、ネイティブフィードのないサービス向けの RSSHub ルートにも対応）。API キーは不要です。

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "rss",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "rss-mcp"]
        },
        "env_passthrough": ["PRIORITY_RSSHUB_INSTANCE"]
      }
    ]
  }
}
```

```bash
# 任意：プライベート / 安定した RSSHub インスタンスを優先する場合
export PRIORITY_RSSHUB_INSTANCE="https://my-rsshub.example.com"
```

**認証：** なし。**トラブルシューティング：** `PRIORITY_RSSHUB_INSTANCE` がない場合、サーバーは公開 RSSHub インスタンスを選び、レート制限に当たることがあります。自前のインスタンスを指定するのが対策です。このサーバーはツール呼び出しごとにフィード URL を受け取るため、設定変更なしでキャラクターが任意のフィードを購読できます。

---

## 8. よくあるトラブルシューティング

| 症状 | 原因 / 対処 |
|---|---|
| `/tool list` に MCP ツールが無い | サーバーの接続に失敗してスキップされました。ログを探してください — stdio の失敗は "MCP server failed to connect"、HTTP の失敗は "MCP HTTP connection failed" です。stdio サーバーの場合は、宣言した `command` / `args` を手動でターミナル実行してサーバー自身のエラーを確認できます。 |
| `command not found: npx` | 子プロセスの環境にはホスト側の小さな許可リスト（`PATH` を含む）だけが転送されます — npx がインストールされ、見つかる必要があります。初回の遅いダウンロードは一度手動で実行（`npx -y <package>`）してウォームアップしておくと、Ene の起動時にパッケージ取得が含まれません。 |
| サーバーは起動するがリクエストを拒否（"missing API key"） | キーが転送されていません：ホスト環境にエクスポート**かつ** `env_passthrough` に列挙してください。Ene にはインラインの `env` マップがありません。 |
| OAuth サーバーがブラウザを要求する | 先にターミナルでサーバーの `auth` 手順を完了してください。Ene は stdio 経由でサーバーを起動するため、同意画面を操作できません。 |
| ローカルサーバーへの HTTP 接続が拒否される | ループバックとプレーン `http://` は既定で拒否されます（SSRF 対策）。ローカル開発用に `plugins.mcp_allow_insecure_urls: true` を設定してください。 |
| "auth header contains invalid characters" | `auth_header` は有効な HTTP ヘッダー値（例：`Bearer <token>`）である必要があります。認証なしへの暗黙のダウングレードではなく、接続が失敗します。 |
| サーバーが `Authorization` 以外のヘッダー（例：`X-Api-Key`）を要求する | Ene の HTTP トランスポートは `Authorization` ヘッダーのみ送信できます — サーバーの stdio 版を使うか、ローカル専用利用ではサーバー側の独自ヘッダーなしで実行してください。 |
| ツールは表示されるが呼び出しが失敗する | リモートサービスがクレデンシャルを拒否しています（限定されたキー、テストユーザー未登録、Notion のページ未共有など）。サービスのコンソール / API ドキュメントを確認してください。 |
