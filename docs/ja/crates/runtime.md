# `ene-runtime` — API リファレンス

> **クレート**: `ene-runtime` | **役割**: アクターベースのホストファサード & システムターンエンジン

`ene-runtime` は Ene を組込むアプリケーション (`ene-cli`, `ene-desktop`) のメインエントリポイントです。ターンの実行、プロンプト構築 (`ene-mind`)、記憶永続化 (`ene-store`)、プラグイン監視 (`ene-plugin-host`)、および DB IPC ソケットサーバーを統合管理します。

---

## 主要型とメソッド

### `EneHandle`
Ene を起動した際に返されるスレッドセーフなハンドル：

```rust
pub struct EneHandle { /* ... */ }

impl EneHandle {
    /// 指定された設定とキャラクターカードで Ene ランタイムを初期化・起動します。
    pub async fn open(config: EneConfig, card: CharacterCard) -> Result<Self, EneRuntimeError>;

    /// 会話ターンを開始します (単一飛行のシェル)。
    pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError>;

    /// 実行中の会話ターンをキャンセルします。
    pub fn cancel(&self, turn_id: TurnId) -> Result<(), CancelError>;

    /// リアルタイムチャットイベントストリーム (TokenStream, Performance, Terminal) を購読します。
    pub fn subscribe(&self) -> broadcast::Receiver<EneEvent>;

    /// ライフサイクルイベントストリーム (StatusChanged, PendingCandidateAvailable,
    /// ToolBackgroundCompleted) を購読します。
    pub fn subscribe_lifecycle(&self) -> broadcast::Receiver<LifecycleEvent>;

    /// 音声 (TTS PCM) ストリームの所有権を取得します。単一コンシューマ専用で、
    /// 2回目以降の呼び出しは `None` を返します。
    pub fn take_audio_stream(&self) -> Option<mpsc::Receiver<AudioChunk>>;

    /// 非同期診断・検査用ハンドルを取得します。
    pub fn diagnostics(&self) -> DiagnosticsHandle;

    /// ランタイムをシャットダウンし、バックグラウンド記憶書込をフラッシュします。
    pub async fn shutdown(self) -> Result<(), EneRuntimeError>;
}
```

### イベントバス: トラフィックの性質ごとに3系統のチャネルへ分離

`ene-runtime` のイベントは3系統の専用チャネルに分離されており、一方のバーストが他方の subscriber を lag させたり飢餓状態にしたりすることはありません。

- **チャットバス** (`EneEvent`、`subscribe` 経由) — 軽量・順序保証付き・ターンスコープのチャットイベントを流す `broadcast` チャネル：

  ```rust
  pub enum EneEvent {
      TurnStarted { turn_id: TurnId },
      TokenStream { chunk: String },
      Performance { cue: PerformanceCue },
      ToolCallStarted { tool_name: String },
      ToolCallFinished { tool_name: String },
      Terminal { turn_id: TurnId, status: TurnStatus },
  }
  ```

- **音声チャネル** (`AudioChunk`、`take_audio_stream` 経由) — 合成された TTS PCM を流す、bounded かつ単一コンシューマ専用の `mpsc` チャネル。重量級の PCM ペイロードがチャットsubscriberの `broadcast` バッファを膨張させないよう、チャットバスから分離しています。
- **ライフサイクルバス** (`LifecycleEvent`、`subscribe_lifecycle` 経由) — `StatusChanged` / `PendingCandidateAvailable` / `ToolBackgroundCompleted` といったターン非依存の通知を流す、小容量の `broadcast` チャネル。

---

## DB IPC サーバー (`DbServer`)

`ene-runtime` はローカル Unix ドメインソケット (UDS) サーバーを起動し、状態を保持するツールサブプロセス (`ene-plugin-fs`, `ene-plugin-utility`) が `ene-plugin-db` を介して `undo.db` / `todo.db` に対するスコープ付き CRUD 操作を実行できるようにします。

---

## 関連ドキュメント
- [システムアーキテクチャ](../architecture.md)
- [ターンとセッション](../concepts/turn-and-session.md)
