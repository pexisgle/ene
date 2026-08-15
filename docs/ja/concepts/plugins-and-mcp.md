# プラグインと MCP

Ene の能力はホストにコンパイルされていません。ツール（ファイルシステム・
web・カレンダー・…）と AI プロバイダー（OpenAI・Anthropic・ローカル GGUF
モデル・音声・…）は**別々のプラグインバイナリ**で、ホストが起動して IPC
で通信します。外部 MCP サーバーも同じ仕組みで接続されます。

## なぜプロセス外なのか

- **隔離** — プラグインがクラッシュ・ハング・暴走してもホストは落ちません。
  スーパーバイザーが再起動します。
- **サンドボックス** — プラグインはリソース要求を宣言し、ホストが受付制御
  を実施します。
- **ネイティブランタイムは 1 プラグインに 1 つ** — llama.cpp・whisper.cpp・
  ONNX Runtime はそれぞれ別バイナリなので、使わないランタイムのビルドコストを
  誰も払いません。

## 2 種類のプラグイン

### ツールプラグイン（`plugins/tool/*`）

ターン中にキャラクターが呼べる名前付きアクションを提供します:
`fs.read`・`web.search`・`utility.timer_start` など。各アクションは JSON
引数・モデル向けの概要/説明・検索キーワード・副作用・バックグラウンド実行の
可否を宣言します。

同梱ツールプラグイン:

| プラグイン | アクション（名前空間） |
|---|---|
| `app` | ウィンドウ/モニター一覧・スクリーンショット・タイピング・マウス/キーボード操作 |
| `browser` | Chrome 自動操作: 移動・クリック・入力・スクリーンショット・抽出 |
| `calc` | 式評価・単位/通貨/色変換 |
| `calendar` | カレンダーアカウント・イベント・空き時間検索（状態保持・承認ゲート付き） |
| `counter` | 状態保持カウンターのサンプル（参照実装） |
| `fs` | 読み書き・編集・削除・glob/grep 検索・パッチ・シェル・undo |
| `geo` | 位置・天気・タイムゾーン・太陽位置 |
| `git` | status・diff・log・branch・remote・blame |
| `homeassistant` | Home Assistant の状態と制御 |
| `random` | 乱数・UUID・選択・色 |
| `utility` | 通知・TODO リスト・時刻・システム情報・タイマー・質問 |
| `web` | URL 取得・Web 検索（Brave/Exa/Tavily/DuckDuckGo/arXiv） |

### プロバイダープラグイン（`plugins/provider/*`）

モデルと音声エンジンを提供します:

| プラグイン | kind | 提供内容 |
|---|---|---|
| `openai` | `openai` | LLM チャット（SSE ストリーミング・ビジョン・構造化出力）、埋め込み |
| `anthropic` | `anthropic` | Messages API による LLM チャット |
| `local-llm` | `local` | llama.cpp による GGUF チャット+埋め込み（`llm/chat@1`・`embed@1`・`gguf-runner@1`） |
| `llama-server` | `llama-server` | llama.cpp サイドカーサーバーによる GGUF チャット |
| `onnx` | `silero` | VAD（Silero ONNX） |
| `whisper` | `whisper` | STT（whisper.cpp） |
| `kokoro` | `kokoro` | ローカル TTS（Kokoro ONNX） |
| `edge-tts` | `edge-tts` | クラウド TTS（Microsoft Edge） |
| `elevenlabs` | `elevenlabs` | クラウド TTS（ElevenLabs REST、Broker 経由） |
| `openai-tts` | `openai_tts` | クラウド TTS（OpenAI） |
| `voicevox` | `voicevox` | VOICEVOX / Aivis Speech エンジンによる TTS（外部・managed サイドカーモード） |

## プラグインのライフサイクル

```text
発見 → 起動 → ハンドシェイク → capability 登録 → ヘルスプローブ
              ▲                                      │
              └────────── 再起動（サーキットブレーカー） ◀┘
```

1. **発見** — 組み込み・ユーザープラグインディレクトリからバイナリを探し、
   `plugins.list.<name>` / `tools.list` で絞り込みます。
2. **起動 + ハンドシェイク** — ホストがバイナリを起動し、stdio 上で IPC
   プロトコルバージョンをネゴシエーションします（長さプレフィックス付き
   フレーム、JSON ハンドシェイク、v6 以降は MessagePack。
   [プラグイン IPC プロトコル](../reference/plugin-ipc.md)参照）。
3. **capability 登録** — プラグインはツールとプロバイダー仕様を宣言します。
   `requires`/`provides` の capability 文字列は検証され、ハード要件が
   満たされないプラグインは無効化されます。
4. **ヘルスと監視** — ホストはプラグイン自身を通してプロバイダー到達性を
   プローブ（最小チャット ping）し、生存を監視し、連続失敗時はバックオフ
   付きで再起動します（サーキットブレーカー）。

## Capability 仲介

プラグインは他のプラグインへ提供する capability
（`provides = "gguf-runner@1"`）と、自分が必要とする capability
（`requires = "gguf-runner@1"`）を宣言できます。ホストが呼び出しを仲介します。
呼び出し側の宣言済み `requires` がリクエストを許可し、ホストが capability
レジストリからプロバイダーを解決し、プロバイダーの IPC 接続へ転送します。

## IPC 上のホストサービス

ホストは共有ソケット上に多重化された**パッセンジャーサービス**を公開します:

- **`db`** — 状態保持プラグインは `ene-plugin-db` を通してホストの
  `memory.db` に対して型付き CRUD を実行します（プラグインごとにテーブルを
  プレフィックス分離、トークン認証）。
- **`capability`** — 上記の capability 仲介チャネル。

## MCP サーバー

Model Context Protocol サーバーは、ツールを公開する外部プロセスまたは
HTTP エンドポイントです。`tools.mcp_servers` に設定します:

```json
{
  "tools": {
    "mcp_servers": [
      {
        "name": "my-server",
        "enabled": true,
        "transport": { "type": "stdio", "command": "npx", "args": ["-y", "some-mcp-server"] },
        "env_passthrough": ["MY_API_KEY"]
      }
    ]
  }
}
```

トランスポートは `stdio`（子プロセス）と `http`（ストリーミング HTTP）。
セキュリティ上、子プロセスは `env_passthrough` に列挙した環境変数**以外**
を一切継承しません。[MCP サーバーガイド](../guides/tools/mcp-servers.md)参照。

## 権限と安全性

- ツールアクションは副作用を宣言し、ホストは破壊的操作の前に承認を求めます
  （`PermissionRequired` イベント）。一度だけ・セッション中・永続のいずれかで
  許可でき、付与済み権限は一覧表示・失効できます（`/permissions`・デスクトップ
  の権限ページ）。
- ツール引数と結果はログやイベントストリームに到達する前にリダクションされます。
- ファイルシステムツールのサンドボックスはパスを制限し、シェル実行は別の
  権限ゲート付きアクションです。
- プラグイン設定値はホスト境界でリダクションされます。

## プラグインの作成

- [ツールを書くガイド](../guides/tools/write-a-tool.md) — 新規ツール
  プラグインの手順。
- [ツール SDK リファレンス](../reference/tools/sdk.md) — `ToolAction` と
  プロバイダートレイト。
- [derive マクロリファレンス](../reference/tools/derive-macro.md) —
  属性リファレンス。
- [プラグイン IPC プロトコル](../reference/plugin-ipc.md) — ワイヤ形式。

## サンドボックス・Broker・承認

プラグインは OS に直接触れません。ホストが OS サンドボックス(Linux は
Landlock + seccomp + rlimits、Windows は Job Object)を適用し、すべての操作を
Broker チャネル(`file`・`network`・`process`・`credential`・`artifact`・
`platform`)で仲介し、二層の承認モデル(署名 manifest → 全体/プラグイン別
ポリシー)で要求をゲートします。実行可能 Artifact のダウンロードは署名付き
Catalog と CAS 検証からのみ可能です。詳細は
[サンドボックス・Broker・承認](sandbox-and-approvals.md)を参照してください
(設定 UI・監査ログ・SSRF ガードを含みます)。

## FAQ

**別言語でプラグインを書けますか？** プロトコル自体は stdio 上のフレーム化
JSON/MessagePack ですが、リポジトリ内のプラグインは `ene-plugin` 上に
構築された Rust バイナリです。テンプレートと SDK は Rust 前提です。

**使わないツールはどう無効化しますか？** `tools.list.<name>.enable = false`
（プロバイダープラグインは `plugins.list.<name>.enable = false`）。

**ターン中にプラグインがクラッシュしたら？** ターンはエラーイベントで
終了し、スーパーバイザーがプラグインを再起動します。状態データはプラグイン
プロセスではなく `db` パッセンジャー経由で `memory.db` に永続化されます。
