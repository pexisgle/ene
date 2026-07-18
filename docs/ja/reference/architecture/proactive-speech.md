# ADR: 能動発話（Proactive Companion Speech）

- **Status:** Accepted
- **Date:** 2026-07-18
- **Epic:** [#103](https://github.com/pexisgle/ene/issues/103) — 能動発話

## 背景と課題

Companion / AITuber 体験では、ユーザーが明示的に話しかけたときだけ応答すると「一緒にいる」感覚が薄れる。常時高性能モデルを回すのはコストが高く、うるさくなりやすい。Ene では **二段階** を採用する。軽量なローカルモデルが「今話すべきか」を判定し、話すべきときだけ通常の生成経路で発話する。

## 決定

能動発話を以下の契約で固定する。

### Crate 責務

| Crate | 責務 |
|---|---|
| `ene-mind` | `ProactiveContext` の構築、決定論的ゲート、判定プロンプト、`ProactiveDecision` の解析・正規化。OS API・スケジューラ・UI には依存しない。 |
| `ene-ai` | 判定モデルのルーティング: `llama_cpp`（プロセス内 llama-cpp-2 GGUF）、`cloud`（OpenAI 互換の model override）、`disabled`。発話生成は既存の chat `LlmProvider` を再利用。ローカル handle は明示的な async shutdown。 |
| `ene-runtime` | interval スケジューリング、ユーザー turn との `TurnGate` 統合、`TurnOrigin`、履歴・イベント、観測投入 API、diagnostics。 |
| `ene-desktop` | プライバシー配慮の OS 観測、設定 UI、能動発話の chat 表示。 |

### 依存ルール

- `ene-mind` は `ene-runtime` / `ene-tool-host` / OS 観測クレートに依存しない。
- `ene-ai` は判定（およびローカル embedding）に llama-cpp-2 を埋め込む。`llama-server` を起動せず、Candle グラフも持たない。llama-cpp 推論はプロセス全体のロックで直列化する。
- Desktop 観測は `ene-desktop`（または desktop 内 platform module）に閉じる。raw screenshot は `ene-mind` / `ene-store` に入らない。

### 判定から発話まで

```text
Timer / observation update
  -> runtime が enabled + cooldown + user turn gate を確認
  -> mind が ProactiveContext を構築（source flag を尊重）
  -> 軽量判定モデルが Decision JSON を返す
  -> should_speak かつ TurnGate が空なら通常の生成経路を起動
  -> TurnStarted + TextDelta / Performance / Terminal を TurnOrigin::Proactive 付きで emit
  -> assistant 応答のみを session history と conversation_logs へ追加
```

### Decision JSON 契約

判定モデルは JSON のみを返す（発話本文は生成しない）。正規化後のフィールド:

| フィールド | 型 | 備考 |
|---|---|---|
| `should_speak` | bool | 必須。欠落時は `false` |
| `confidence` | f64 | 有限かつ `[0.0, 1.0]` 内であること。範囲外は fail-closed（`should_speak = false`）。欠落は `0.0` |
| `reason` | string | 内部診断用。そのまま発話しない |
| `topic_hint` | string | 生成用の任意ヒント。欠落時は空 |
| `urgency` | string | `low` / `normal` / `high`。未知は `normal` |

未知フィールドは無視する。parse / timeout / provider 失敗は fail-closed: `should_speak = false` として扱い、生成を開始しない。

### Turn origin と履歴

- `TurnOrigin::{User, Proactive}` を turn-scoped event と `Terminal` に載せる。
- 能動発話は偽の user message を `ConversationSession` に追加しない。
- assistant 応答のみを history / `conversation_logs` に書き、以降の通常対話の文脈にする。
- `PostTurnInput` / memory writer は空の user message を記憶候補にしない。
- 能動発話の生成は `ai.tasks.proactive` でルーティング（設定時）。未設定なら `ai.tasks.chat`（下記 **モデルルーティング** 参照）。
- 内部 companion 指示は生成時に **system** メッセージとして注入。ユーザー履歴・embedding・memory writer には渡さない。
- `generation_timeout_seconds` が能動発話生成の壁時計上限（外側 timeout が優先）。
- ユーザー turn 開始または actor 終了時に進行中の decision タスクを abort する。
- 能動発話の生成は既定で `allow_tools = false`。

### 抑制ポリシー（設定化）

| ルール | 設定 / 挙動 |
|---|---|
| 機能オフ | `mind.proactive.enabled` の default は `false` |
| ユーザー / tool / permission busy | ユーザー turn・tool call・permission/input 待ち中は判定も生成もしない |
| 最低 idle | 最後のユーザー入力から `min_idle_seconds` 未満は抑制 |
| cooldown | **成功した**能動発話（`TerminalReason::Done`）後 `cooldown_seconds` の間は再発話しない |
| セッション上限 | 1 セッションあたり最大 `max_turns_per_session` 回 |
| source なし | 全入力ソースが無効（または利用不可）なら判定しない |
| 信頼度 | `confidence >= decision.min_confidence` のときのみ生成へ進む |
| 失敗 | 判定失敗・空生成・ローカルモデル初期化失敗でも actor を sticky Error にしない |

### 入力ソース

| ソース | 内容 | プライバシー |
|---|---|---|
| `conversation` | 直近の `HistoryEntry`（文字数上限で truncate） | セッション履歴のみ |
| `activity` | 任意の idle ヒント、**アプリ名のみ**（生ウィンドウタイトルなし）、直近フォーカス変化 | キーログなし。V1 ではタイトルは収集しない |
| `screen_summary` | デスクトップ要約プロバイダが利用可能なときの短命 **テキスト** 要約 | V1 デスクトップは有効でも要約器未同梱のため `unavailable`。raw screenshot は永続化・ログ・diagnostic に出さない |

各ソースは個別に有効 / 無効。無効時は desktop が収集せず、mind も判定プロンプトに含めない。

### モデルルーティング

モデルルーティングは設定の `ai.tasks` 配下（[設定](../configuration/settings.md#ai--プロバイダレジストリとタスクルーティング) 参照）:

| 役割 | 設定 |
|---|---|
| 生成 | `ai.tasks.proactive` があればそれ、なければ `ai.tasks.chat` |
| 分類器（感情） | `ai.tasks.classifier` があればそれ、なければ `ai.tasks.chat` |
| 判定 | `ai.tasks.proactive` が参照するプロバイダ（`proactive` が `null` のときは chat プロバイダ）: 非空 `model_path` 付き `local_gguf` → プロセス内 GGUF；それ以外は chat プロバイダ経由のクラウド判定 |

`local_gguf` は llama-cpp-2 で GGUF をプロセス内ロードする。`acceleration` は `auto` / `vulkan` / `cuda` / `cpu`。ローカル load 失敗時は判定バックエンドが disabled にフォールバック（fail-closed）— 観測コンテキストを黙ってクラウドへ送らない。

GGUF 重みはアプリに同梱しない。外部 `llama-server` も不要。パスはユーザー設定。

### Fail-closed まとめ

- 不正な設定 → 安全な default / clamp。明示的に有効化するまで機能はオフ。
- ローカルモデル欠如・load 失敗・timeout → typed error → 発話しない（cloud fallback は設定時のみ）。
- 不正な Decision JSON → `should_speak = false`。
- 判定中のユーザー `run()` → decision を破棄し、ユーザーターンを優先。
- shutdown で timer・判定・生成を止め、ローカル llama.cpp handle を解放。

## 影響

- デフォルト設定では既存の通常会話挙動が変わらない（能動発話オフ）。
- Desktop が OS 固有の観測を正規化した `ProactiveObservation` として runtime へ渡す。
- CLI / desktop の `EneEvent` 消費者は `TurnStarted` と `TurnOrigin` を壊さずに扱える必要がある（加算的な拡張）。
- デスクトップの設定変更は `UpdateProactiveSettings` で actor に即時反映される（再起動不要）。
- Guide にローカルモデル配置、Vulkan/RADV 要件、プライバシー注意を記載する。

## 関連

- Epic [#103](https://github.com/pexisgle/ene/issues/103)
- Sub-issues #162–#170
- [認知 Runtime ADR](cognitive-runtime.md)
- [API v1 ADR](api-v1.md)
