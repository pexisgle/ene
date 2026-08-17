# capability seam 群と MCP

> 実現する要件: **P-602**(ビルトインツール)、**P-603**(MCP)、
> **P-604**(コネクタ)、**P-611**(コード実行)、**P-612**(コーディング)、
> **P-1003**(プロバイダ seam)。

## capability seam の3役割(dsh に倣う)

差し替え可能な能力は必ず3役割で設計する。

1. **Service Definition**: インターフェース宣言(Rust トレイト相当の契約)。
2. **Service Provider**: 実装(ローカル実装 / プラグイン / リモート)。
3. **Consumer**: 使用者。大半はモデル向けのツール。

プロバイダを入れ替えると、消費者を分岐させずに製品全体が切り替わる。
例: fs の provider をリモートサンドボックスに向けると、
fs 系ツール・コード実行・成果物保管がすべて一緒に移動する。

## 1. fs seam

| 役割 | 内容 |
|---|---|
| Definition | `open_read/open_write/list/stat/watch/move/remove`。パスは正規化済みで受ける |
| Provider(既定) | ローカル FS。Broker 経由でプラグインに委譲([../plugins/broker.md](../plugins/broker.md)) |
| Consumer | `fs.read` / `fs.write` / `fs.search`(ripgrep 相当) / `fs.list` ツール、成果物保管、spill |

- ルート概念: ツールは必ず**当該 spawn の job ディレクトリ**を基準に動く
  (`<data>/workspaces/<soul_id>/jobs/<job_id>/`、
  [../tasks/jobs-and-schedules.md](../tasks/jobs-and-schedules.md))。
  soul 根や他 job への横断はスコープ外。絶対パス指定はスコープ検査後に解決。
- `fs.search` は大規模リポジトリに耐える実装(rg 呼び出し)を既定とする。

## 2. exec seam(シェル/プロセス実行)

| 役割 | 内容 |
|---|---|
| Definition | `run(command, cwd, env, timeout) → { stdout, stderr, exit_code }` と持続 PTY の2面 |
| Provider(既定) | サンドボックス内ローカル実行([../security/sandbox.md](../security/sandbox.md)) |
| Consumer | `exec.run` / `exec.pty` ツール、サイドカー起動 |

- コーディング(P-612)と日常スクリプト(P-611)の両方がこの seam を使う。
- `cwd` の既定は当該 spawn の job ディレクトリ(fs と同じ根)。
- `exec.run` の既定: `read_only: false`, `side_effects: ["exec"]`。
  承認ポリシーの対象。
- 持続 PTY は `exec.pty` で扱い、セッション跨ぎのシェルを1つだけ許可
  (複数 PTY は後継)。

## 3. web seam

| 役割 | 内容 |
|---|---|
| Definition | `search(query) → 結果列`、`fetch(url) → 正規化コンテンツ` |
| Provider(既定) | ホスト側 HTTP クライアント(Broker 代行)+ HTML→Markdown 変換 |
| Consumer | `web.search` / `web.fetch` ツール |

- SSRF 防護は Broker 層で強制(私的アドレス帯の既定拒否)。
- `fetch` は巨大ページを spill 候補として扱う(本文上限+要約)。

## 4. browser seam(ブラウザ操作)

| 役割 | 内容 |
|---|---|
| Definition | `navigate/click/type/screenshot/snapshot`(DOM 要約) |
| Provider(既定) | ヘッドレスブラウザのプラグイン(アウトプロセス) |
| Consumer | `browser.*` ツール群 |

- ログイン済みセッションは使わない(毎回クリーン)。資格情報を要する
  操作は connector seam に委ねる。
- `screenshot` の出力は画像ブロック(spill 対象)。

## 5. MCP seam(P-603)

MCP サーバーは**供給元**として接続する([registry.md](registry.md) 前提参照)。

- **接続**: stdio / Streamable HTTP をホスト側 MCP クライアント
  (プラグインプロセス)が扱う。接続設定は manifest/profile の行。
- **tools**: MCP ツールをレジストリに登録。`execute` は MCP 呼び出しへの
  ブリッジ。承認・ガード・spill はネイティブと**同一パイプライン**。
  ツール名は `mcp:<server>.<tool>` の名前空間。
- **resources → Context Source**: MCP resource を
  [context-assembly](../core/context-assembly.md) の `mcp.resources` Source に
  変換。購読型 resource の変更は途中システムメッセージになる。
- **prompts → skill 素材**: MCP prompt を skill 素材として取り込める
  ([../tasks/skills.md](../tasks/skills.md))。
- 障害: MCP サーバー死亡は `mcp_unavailable` として、その供給元の
  ツールだけを一時的に無効化。他は影響しない。

## 6. connector seam(外部サービス)(P-604)

メール・カレンダー・ファイル同期などの外部サービス接続。

- **構造**: 各 connector は「資格情報(ボールト) + API クライアント +
  ツール群」のパッケージ。ツールは `calendar.*` / `mail.*` のように
  名前空間付きでレジストリに登録。
- **credential**: ボールトから Broker の `cred.inject` 経由で注入
  ([../plugins/broker.md §2](../plugins/broker.md))。プラグインに平文を渡さない。
- **v1.0 の最小**: カレンダー(予定の読み書き)+ メール(読み・下書き作成、
  **送信は承認必須**)+ 汎用 HTTP コネクタ。
- **後継**: ファイル同期・SNS 系。形式(credential 注入・ツール登録・
  承認)は v1.0 で固定されるので、追加は provider 増設のみ。
- **Webhook/受信**: コネクタの受信イベント(カレンダー変更等)は
  `inject` 入力として inbox に入り、能動発話やリマインドの材料になる。

## 7. code seam(コード実行)(P-611)

- **v1.0(最小)**: exec seam 上の隔離ディレクトリでスクリプト
  (Python 等)を実行。成果物は fs seam に書く。
- **後継**: 専用のコード実行ランタイム(依存のキャッシュ・複数言語)。
  seam は同じなので差し替えのみ。
- 実行の既定: `read_only: false`, `side_effects: ["exec", "fs.write"]`。

## 8. プロバイダ seam(P-1003)

LLM/埋め込み/STT/TTS/VAD はすべてプロバイダ seam。

| seam | 面 |
|---|---|
| `llm` | chat/generate。ツールスキーマ渡し・ストリーミング・内面チャネル対応 |
| `embed` | テキスト→ベクトル |
| `stt` | 音声→テキスト(部分結果の逐次) |
| `tts` | テキスト→音声(ストリーミング、感情/スタイル指定) |
| `vad` | 音声フレーム→発話区間 |

- クラウド/ローカルを問わない同一経路(P-909)。プロバイダはプラグイン
  として実装され、[ipc.md §3.3](../plugins/ipc.md) のメッセージを使う。
- タスク別モデル選択(`ai.tasks.<task>`)は
  [manifest-and-profile.md §3](../plugins/manifest-and-profile.md#3-プロバイダの選択p-909)。

## 9. vision seam(後継・形式のみ)

カメラ/画面/スクリーンショットの認知(P-112)。v1.0 では**接点のみ**
定義: `vision.capture(kind) → 画像ブロック` を Context Source と
 ツール引数の両方から参照できる形にする。実装は後継。

---

- 前: [registry.md](registry.md) / 次: [../companion/README.md](../companion/README.md)

