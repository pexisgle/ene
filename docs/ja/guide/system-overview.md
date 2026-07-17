# システム概要

ene はローカル AI キャラクター基盤です。LLM との会話、任意のツール、長期記憶、Desktop では VRM アバターを扱います。

## 主な部品

| 部品 | 役割 |
|------|------|
| **CLI / Desktop** | ホストアプリ。設定とキャラクターカードを読み、ランタイムを開き、UI を出す。 |
| **`ene-runtime`** | ホストファサード。準備済みハンドル（`EneHandle::open`）、同時に 1 ターン。 |
| **`ene-mind`** | プロンプト内容、記憶の想起、感情更新、アバター用キューの決定。 |
| **`ene-ai`** | クラウドまたはローカルの LLM / 埋め込み。 |
| **`ene-store`** | 記憶などの SQLite 永続化。 |
| **Tools** | IPC 越しの別プロセス（filesystem、web、browser など）。 |
| **`ene-vrm`** | アバター描画。mind / runtime に依存しない。 |

## 会話の 1 ターン

```text
メッセージ入力
  → ランタイムがターン開始（または Busy）
  → mind が文脈を想起しプロンプトを組む
  → LLM がトークンをストリーム
  → 途中でツールが動くこともある
  → mind が記憶・感情を書き、Performance キューを出す
  → ターン終了（Terminal イベント）
```

ホストは小さなチャットイベントバスを購読します。診断は別チャネルです。

## アプリの位置づけ

- **CLI** — REPL とスラッシュコマンド。ツールや記憶のデバッグ向き。
- **Desktop** — 同じランタイムに加え、Performance イベントで VRM を再生。

## さらに深く

- 概念: [ターン](concepts/turns.md)、[セッション](concepts/sessions.md)、[記憶](concepts/memory.md)、[感情](concepts/emotions.md)
- 設計契約: [アーキテクチャ概要](../reference/architecture/overview.md)、[API v2](../reference/architecture/api-v2.md)
- 公開 API: [API インデックス](../reference/api/index.md)
