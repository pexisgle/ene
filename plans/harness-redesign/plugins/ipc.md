# プラグイン IPC

> 実現する要件: **P-1001**(全アウトプロセス)、**P-1004**(多言語プラグイン)、
> P-509 の実行期限伝播、P-902 の Broker 接点。

ツール・プロバイダ・MCP ブリッジの**すべて**が子プロセスで動く。
この IPC は軽量プラグインが常用に耐える速度を出すこと、
任意の言語で実装できることを目標に設計する。

## 1. 輸送

- **ソケット**: Unix domain socket(Linux)。Windows では named pipe
  (FD 渡し相当はサポートせず、ストリーム転送で代替)。
- **フレーム**: 長さプレフィックス(32bit BE) + MessagePack ペイロード。
  1フレーム上限は既定 64 MiB(`plugins.ipc.max_frame_bytes`)。
- **FD 渡し**: `SCM_RIGHTS`(Linux)。 Broker のファイルハンドル・
  ストリームのソケット対・音声の共有メモリ fd を渡す。
  受け取り側は必ず即座に所有(複製はしない)。
- **背圧**: ソケットごとの送信バッファ上限(既定 8 MiB)を超えたら
  ストリームは `flow_control: pause` フレームで一時停止。

## 2. ネゴシエーション

接続直後に1往復する。

```text
Host → Plugin: hello {
  host_name, host_version,
  protocol: { min: u32, max: u32 },        -- 現行世代: 1
  features: [string],                       -- 双方が使える機能の提案
  capabilities: [CapabilityGrant],          -- このプラグインに与える権限(manifest 検証済み)
  clock_id: string,                         -- 期限計算用の単調時計共有
}
Plugin → Host: hello_ack {
  plugin_id, plugin_name, plugin_version, manifest_digest: sha256,
  protocol_version: u32,                    -- [min, max] から選択
  features: [string],                       -- 採用する機能
  ready_after_ms: u32?                      -- 初期化猶予
}
```

- `manifest_digest` が manifest と不一致なら即切断(なりすまし防止)。
- 非互換バージョン(範囲外)は `hello_reject{reason}` で切断。
- 現行世代の protocol_version は `1`。フィールド追加は
  後方互換(serde default 相当: 未知フィールド無視)で済ませ、
  世代上げは意味破壊時のみ。

## 3. メッセージ種別

全メッセージ共通: `{ id: u64, kind: string, body: ... }`。
`id` はリクエスト/応答の対応付けに使う(通知は id なし)。

### 3.1 制御

| kind | 方向 | 説明 |
|---|---|---|
| `ping` / `pong` | 双方向 | ヘルス。既定間隔 15 秒、3回不達で死亡判定 |
| `shutdown` | Host→Plugin | 猶予 `plugins.shutdown_grace_ms`(既定 5000ms)で終了 |
| `drain` | Host→Plugin | 実行中を片付けてから終了(再起動前) |
| `log` | Plugin→Host | 構造化ログ(tracing 形式のフィールド群) |

### 3.2 ツール実行

| kind | 方向 | 説明 |
|---|---|---|
| `tool.list` | Host→Plugin | ツール一覧要求。応答は `tool.spec` の配列 |
| `tool.spec` | Plugin→Host | ツール定義(name, description, parameters JSON Schema, output schema, hints: timeout_ms/is_concurrency_safe/read_only) |
| `tool.call` | Host→Plugin | `{ call_id, tool_name, args, deadline_ms?, fds: [fd], signal_token }` |
| `tool.progress` | Plugin→Host | `{ call_id, fraction?, note? }`。UI の「今何をしているか」表示用 |
| `tool.result` | Plugin→Host | `{ call_id, status, blocks または canonical value, fds: [fd]? }` |
| `tool.cancel` | Host→Plugin | `{ call_id }`。協調的キャンセル。プラグインは速やかに quiescence に至る義務 |

**正規出力**: プラグインは `tool.spec` で宣言した output schema に
従う値を返す。表示用ブロックへの変換はホスト側が output schema の
レンダラで行う([../tools/registry.md](../tools/registry.md) の
`ToolOutputDefinition` に相当)。これにより「正規値」がログと
リプレイの安定した単位になる。

### 3.3 プロバイダ(LLM/TTS/ASR/埋め込み)

| kind | 方向 | 説明 |
|---|---|---|
| `provider.describe` | Host→Plugin | 対応タスク・モデル一覧・窓・料金等のメタ情報 |
| `llm.generate` | Host→Plugin | `{ request_id, model, messages, tools?, sampling }` |
| `llm.chunk` | Plugin→Host | `{ request_id, delta: text|tool_call|inner, finish? }` |
| `llm.done` | Plugin→Host | `{ request_id, usage, finish_reason }` |
| `tts.synthesize` | Host→Plugin | `{ request_id, text, voice, style? }`。音声は別チャネル(§4) |
| `asr.transcribe` / `asr.stream` | Host→Plugin | 音声認識。`asr.stream` は partial を逐次返す |
| `embed.encode` | Host→Plugin | `{ request_id, texts }` → ベクトル群 |

VAD はフレーム単位 RPC(`vad.frame`)にしない。低遅延が要る音声は
§4 の専用チャネルに載せる。制御面は `stream.open{ kind: vad }` だけ。

### 3.4 Broker と capability

プラグインからホストへの要求は Broker RPC として流れる
(詳細は [broker.md](broker.md)):

| kind | 方向 | 説明 |
|---|---|---|
| `broker.request` | Plugin→Host | `{ method, params, capability_ref }` |
| `broker.grant` | Host→Plugin | `{ grant_id, fds }`。FD で資源を委譲 |
| `broker.release` | 双方向 | 委譲の解除 |
| `approval.query` | Plugin→Host | 「この操作は承認済みか」の問い合わせ(plane 判断はホスト側) |

## 4. ストリーミング(音声など高頻度データ)

- RPC フレームに載せると遅延が出るため、**専用チャネル**を使う。
- 確立: ホストが `stream.open{ stream_id, kind }` を送り、
  プラグインは socketpair を FD 渡しで返す。以降そのソケット対に
  長さプレフィックス付きバイナリフレームを流す。
- 音声フレーム形式: `{ seq, ts, encoding: pcm16|opus, sample_rate, channels, data }`。
- **VAD**: 入力 PCM をこのチャネルで送り、発話区間イベント
  (`speech_start` / `speech_end`)を同じチャネルで返す。
  `vad.frame` 同期 RPC は持たない(5ms IPC 予算と衝突するため)。
- 背圧は §1 の flow_control に従う。音声は**破棄優先**
  (溜めると遅延になるため、閾値超えは最新を優先して落とす)。

## 5. キャンセル伝播と期限

- `tool.call` / プロバイダ要求の `deadline_ms` はホストの単調時計基準
  (`clock_id` で共有)。プラグインはローカルで期限監視し、
  超過前に quiescence に至る義務がある。
- `tool.cancel` 受信後は、進行中作業を**協調的に**停止し、
  部分結果を `tool.result{status: cancelled}` で返す。
- ホスト側は deadline 超過後も応答がない呼び出しを `timeout` で
  確定し([../core/agent-loop.md](../core/agent-loop.md) §8)、
  プラグイン自体は殺さない(次回の呼び出しは正常に送る)。
  連続3回の期限無視でサーキットブレーカー。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| プラグインの突然死 | ping 不達・EOF で検知。進行中の呼び出しは全部 `plugin_dead` で確定。監督が再起動([../platform/process-model.md](../platform/process-model.md)) |
| フレーム上限超過 | フレームを拒否し `tool.result{status: error, error_class: frame_too_large}`。プラグインは即座に切断されない(仕様違反の記録のみ) |
| FD 渡しの失敗 | `broker.grant` をエラーで返し、プラグインは代替(再要求/諦め)を選ぶ |
| hello のタイムアウト | 接続後 2 秒以内に hello_ack がなければ切断+起動失敗扱い |
| MessagePack のデコード不能 | そのフレームを破棄し、`log` で警告。連続5回で切断(実装欠陥とみなす) |
| 背圧閾値超過 | ストリームは破棄優先(§4)、RPC は送信側をブロック |

## 7. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `plugins.ipc.max_frame_bytes` | `67108864` (64 MiB) | 1フレーム上限 |
| `plugins.ipc.hello_timeout_ms` | `2000` | ネゴシエーション期限 |
| `plugins.ipc.ping_interval_ms` | `15000` | ヘルス間隔 |
| `plugins.ipc.send_buffer_bytes` | `8388608` (8 MiB) | 背圧閾値 |
| `plugins.shutdown_grace_ms` | `5000` | 終了猶予 |

---

- 次: [manifest-and-profile.md](manifest-and-profile.md)

