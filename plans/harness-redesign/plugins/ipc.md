# プラグイン IPC

> 実現する要件: **P-1001**(アウトプロセスプラグイン)、**P-1004**(多言語プラグイン)、
> **P-1009**(副プロトコル分割)、P-509 の実行期限伝播、P-902 の Broker 接点。
> 参照: D-22。ホスト文脈の寿命は [composition.md](composition.md)(D-32)。

ツール・プロバイダ・MCP ブリッジが子プロセスで動く。ハーネス機能ツールは
ホスト内なのでこの IPC を通らない([../tools/registry.md §0](../tools/registry.md#0-2軸の分類d-10))。

## 0. 現行(v8)を書き直す理由(D-22)

現行の `ene-plugin-proto` は単一の巨大な enum に、ライフサイクル・ツール・
権限・プロバイダ6種・ケイパビリティが同居している。この形が3つの問題を生む。

1. **無関係な知識の強制**。ツールしか提供しないプラグインも、VAD や TTS の
   メッセージ変種を含む型を扱うことになる。
2. **バージョンの巻き添え**。新しいモダリティを1つ足すと共有プロトコル
   バージョンが上がり、N-1 互換ポリシーによって無関係な全プラグインが
   一斉に失効する。
3. **境界の型が無い**。`arguments: String`(中身は JSON)、
   `provider_config: Value`、`messages: Vec<Value>` のように、
   本来スキーマがあるはずの場所が素通しになっている。
   `request_id` が `#[serde(default)]` なため相関IDが省略可能になり、
   全ハンドラが空文字を許容する作りになっているのも同根である。

加えて、音声が `audio_base64: String` としてフレームに乗る。
base64 は 33% の膨張に加えて、フレーム全体をメモリに乗せることを強制する。

書き直しの方向は3点。**副プロトコルへの分割**(§1)、
**境界の型付け**(§4)、**バルク転送のフレーム外化**(§6)。

## 1. 副プロトコルへの分割(P-1009)

1本のプロトコルを4つに割り、**それぞれ独立にバージョンを持つ**。

| 副プロトコル | 責務 | 実装義務 |
|---|---|---|
| `core` | ハンドシェイク・ヘルス・終了・ログ | **必須**。全プラグインが実装する |
| `tool` | ツール一覧・呼び出し・進捗・キャンセル | 任意 |
| `provider` | LLM / 埋め込み / STT / TTS / VAD | 任意。さらにモダリティ単位で分かれる(§5) |
| `capability` | Broker 要求・承認問い合わせ | 任意 |

分割の効果:

- ツールプラグインは `core` + `tool` だけを実装する。
  provider のメッセージ型を知らなくてよい。
- TTS にフィールドを1つ足しても、上がるのは `provider.tts` の
  バージョンだけ。ツールプラグインは影響を受けない。
- 互換性の判定が「どの副プロトコルのどのバージョンを話せるか」の
  積になるので、失効の範囲が実際の変更範囲に一致する。

## 2. 輸送

- **ソケット**: Unix domain socket(Linux)。Windows では named pipe。
- **フレーム**: 長さプレフィックス(32bit BE)+ MessagePack ペイロード。
  1フレーム上限は `plugins.ipc.max_frame_bytes`。
  **この上限に当たるのは設計の失敗**である。大きいデータは §6 の経路に
  載せる(上限は暴走の歯止めであって、通常運用の枠ではない)。
- **FD 渡し**: `SCM_RIGHTS`(Linux)。Broker のファイルハンドル・
  ストリームのソケット対・音声の共有メモリ fd を渡す。
  受け取り側は必ず即座に所有する(複製はしない)。
  Windows は `DuplicateHandle` による同等の受け渡しを使う。
- **背圧**: ソケットごとの送信バッファ上限を超えたら
  `flow_control: pause` フレームでストリームを一時停止。

## 3. ハンドシェイク

接続直後に1往復する。副プロトコルごとの範囲を交換する。

```text
Host → Plugin: hello {
  host_name, host_version,
  protocols: {                        -- 副プロトコルごとに独立した範囲
    core:       { min, max },
    tool:       { min, max },
    provider:   { min, max },
    capability: { min, max },
  },
  capabilities: [CapabilityGrant],    -- このプラグインに与える権限(manifest 検証済み)
  clock_id: string,                   -- 期限計算用の単調時計共有
}

Plugin → Host: hello_ack {
  plugin_id, plugin_name, plugin_version,
  manifest_digest: sha256,
  protocols: {                        -- 実装する副プロトコルと選んだ版のみ
    core: u32,
    tool: u32?,                       -- 実装しないなら省く
    provider: { llm: u32?, tts: u32?, stt: u32?, vad: u32?, embed: u32? }?,
    capability: u32?,
  },
  ready_after: duration?,             -- 初期化猶予
}
```

- `manifest_digest` が manifest と不一致なら即切断(なりすまし防止)。
- **manifest が宣言していない副プロトコルを `hello_ack` が名乗ったら切断**。
  権限審査は manifest を見て行うので、実際に話す面が manifest を
  超えてはならない([manifest-and-profile.md](manifest-and-profile.md))。
- `core` の範囲が重ならなければ `hello_reject{reason}` で切断。
  他の副プロトコルは重ならなければ**その面だけ無効**にして接続を続ける。
  ツールは使えるが TTS は使えない、という部分的な状態を許す。
- 互換ポリシーは副プロトコルごとに N-1。フィールド追加は
  `#[serde(default)]` で吸収し、版上げは意味破壊時のみ。

## 4. メッセージ

全メッセージ共通: `{ id: u64, proto: string, kind: string, body: ... }`。
`proto` は副プロトコル名。`id` はリクエスト/応答の対応付けに使う。

**`id` は省略可能にしない**。現行の `request_id: #[serde(default)]` は、
全ハンドラに「空の相関ID」の処理を強いていた。応答を持つ種別では必須とし、
通知(応答を持たないもの)は別の型にして `id` フィールド自体を持たせない。

### 4.1 core

| kind | 方向 | 説明 |
|---|---|---|
| `ping` / `pong` | 双方向 | ヘルス。不達が続けば死亡判定 |
| `shutdown` | Host→Plugin | 猶予付き終了 |
| `drain` | Host→Plugin | 実行中を片付けてから終了(ファイバー unload / 再起動の前段) |
| `reconfigure` | Host→Plugin | `{ config }`。行の設定だけが変わったときの差分。応答必須 |
| `reconfigure_ack` | Plugin→Host | `{ status: applied \| need_rebuild \| error }`。`need_rebuild` / `error` / 無応答はホストがそのファイバーだけ rebuild する。`applied` ならプロセスも `uid` も維持 |
| `log` | Plugin→Host | 構造化ログ(tracing 形式のフィールド群) |

### 4.2 tool

| kind | 方向 | 説明 |
|---|---|---|
| `list` | Host→Plugin | ツール一覧要求 |
| `spec` | Plugin→Host | ツール定義(name, description, parameters schema, output schema, hints)。ホストはこれを巻き戻し可能な effect としてレジストリへ載せる([composition.md §3](composition.md#3-巻き戻し可能な-effect時間)) |
| `call` | Host→Plugin | `{ call_id, tool_name, args, deadline?, fds, signal_token }` |
| `progress` | Plugin→Host | `{ call_id, fraction?, note? }` |
| `result` | Plugin→Host | `{ call_id, status, value, fds? }` |
| `cancel` | Host→Plugin | `{ call_id }`。協調的キャンセル |

**引数と結果の型付け**: `args` は現行のような JSON 文字列ではなく、
MessagePack の構造化値として運ぶ。二重シリアライズを避けるためだけでなく、
文字列にすると中間層がスキーマ検証できないためである。検証は
`spec` の parameters schema に対してホスト側が呼び出し前に行う
([../tools/registry.md §3](../tools/registry.md))。

**正規出力**: プラグインは `spec` で宣言した output schema に従う値を返す。
表示用ブロックへの変換はホスト側のレンダラが行う。
これにより「正規値」がログとリプレイの安定した単位になる。

`side_effects` の宣言は `spec` に含める。空かどうかが表層への公開可否を
決める([../tools/registry.md §2.1](../tools/registry.md#21-層ごとの公開面p-522--d-2))ので、
**宣言は省略不可**とし、既定値で空にはしない。

### 4.3 provider

モダリティごとに独立した版を持つ(§1)。

| モダリティ | 主な kind |
|---|---|
| `llm` | `describe` / `generate` / `chunk` / `done` |
| `embed` | `encode` |
| `stt` | `transcribe` / `stream` |
| `tts` | `synthesize` |
| `vad` | 制御面のみ(§6) |

**メッセージ列の型付け**: `llm.generate` の `messages` は現行の
`Vec<Value>` ではなく、役割・内容ブロック・ツール呼び出しを持つ
定義済みの型にする。プロバイダごとの方言への変換はプラグイン側の責務で、
境界では共通表現に揃える。`provider_config` も同様に、
`Value` ではなくプロバイダ種別ごとの型を持つ。

補助LLM(P-524)もこの面を通る。呼び出し元の分類はホスト側の関心事で、
プロトコルには現れない。

### 4.4 capability

| kind | 方向 | 説明 |
|---|---|---|
| `request` | Plugin→Host | `{ method, params, capability_ref }` |
| `grant` | Host→Plugin | `{ grant_id, fds }`。FD で資源を委譲 |
| `release` | 双方向 | 委譲の解除 |
| `approval_query` | Plugin→Host | 「この操作は承認済みか」の問い合わせ(判断はホスト側) |

詳細は [broker.md](broker.md)。

## 5. 副プロトコルの版管理

- 版は副プロトコルごとに独立して上がる。共有の「プロトコル世代」は持たない。
- ホストは副プロトコルごとに `[min, max]` を広告し、N-1 を維持する。
- **プラグイン側の実装コストを増やさないこと**が分割の前提。
  プラグイン作者は使う面だけを実装し、ハンドシェイクで名乗る。
  authoring facade が、実装したトレイトから `hello_ack` の中身を自動で
  組み立てる。ホスト文脈への登録の逆は**ホスト**が積む(D-32)。
  作者は `deactivate` を書かない。

## 6. バルク転送(音声・大きな結果)

**フレームに載せない。** 現行の `audio_base64: String` は base64 の膨張と
全体バッファリングを同時に招いていた。

- **確立**: ホストが `stream.open{ stream_id, kind }` を送り、
  プラグインは socketpair を FD 渡しで返す。以降そのソケット対に
  長さプレフィックス付きのバイナリフレームを流す。
- **音声フレーム**: `{ seq, ts, encoding, sample_rate, channels, data }`。
  `data` は生バイト列で、base64 を通さない。
- **大きなツール結果**: 一定量を超える結果は、フレームではなく
  FD 渡しのファイル/共有メモリで受け渡し、フレームには参照だけを載せる。
  spill との接続はホスト側で行う
  ([../core/context-assembly.md](../core/context-assembly.md))。
- **VAD**: 入力 PCM をこのチャネルで送り、発話区間イベント
  (`speech_start` / `speech_end`)を同じチャネルで返す。
  フレーム単位の同期 RPC は持たない(低遅延の要求と衝突するため)。
- 背圧は §2 の flow_control に従う。音声は**破棄優先**
  (溜めると遅延になるため、閾値超えは最新を優先して落とす)。

## 7. キャンセル伝播と期限

- `tool.call` / プロバイダ要求の `deadline` はホストの単調時計基準
  (`clock_id` で共有)。プラグインはローカルで期限監視し、
  超過前に quiescence に至る義務がある。
- `tool.cancel` 受信後は、進行中作業を**協調的に**停止し、
  部分結果を `result{status: cancelled}` で返す。
- ホスト側は deadline 超過後も応答がない呼び出しを `timeout` で
  確定し([../core/agent-loop.md §8](../core/agent-loop.md#8-ガードとエラー回復))、
  プラグイン自体は殺さない(次回の呼び出しは正常に送る)。
  期限無視が続けばサーキットブレーカーが開く。
- ファイバーの unload では、進行中の `call` を `cancel` し、`drain` のあと
  プロセスの逆(kill)とホスト文脈の逆(登録・grant の回収)を走らせる。
  プロセスが先に死んでもホスト文脈の逆は省略しない
  ([composition.md §3](composition.md#3-巻き戻し可能な-effect時間))。

## 8. 障害モード

| 障害 | 挙動 |
|---|---|
| プラグインの突然死 | ping 不達・EOF で検知。進行中の呼び出しは全部 `plugin_dead` で確定。ホスト文脈の登録と grant はファイバー unload で回収し、監督が再起動または `failed`([composition.md §6](composition.md#6-ライフサイクル慣性)) |
| フレーム上限超過 | フレームを拒否し `result{status: error, error_class: frame_too_large}`。§6 を使うべき場面なので、ライフサイクル警告も出す |
| FD 渡しの失敗 | `capability.grant` をエラーで返し、プラグインは代替(再要求/諦め)を選ぶ |
| hello のタイムアウト | 期限内に `hello_ack` がなければ切断+実行時失敗(backoff → `failed`) |
| `reconfigure` の無応答 | `need_rebuild`。そのファイバーだけ rebuild |
| manifest 外の副プロトコルを名乗る | 切断。権限審査との不一致は設定ミスではなく攻撃の可能性がある |
| 一部の副プロトコルだけ非互換 | その面のみ無効化して接続継続(§3) |
| MessagePack のデコード不能 | そのフレームを破棄し `log` で警告。連続で切断(実装欠陥とみなす) |
| 背圧閾値超過 | ストリームは破棄優先(§6)、RPC は送信側をブロック |

## 9. 設定キー

数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `plugins.ipc.max_frame_bytes` | 1フレーム上限(暴走の歯止め) |
| `plugins.ipc.hello_timeout` | ハンドシェイク期限 |
| `plugins.ipc.reconfigure_timeout` | `reconfigure_ack` 待ち。超過は `need_rebuild` |
| `plugins.ipc.ping_interval` | ヘルス間隔 |
| `plugins.ipc.send_buffer_bytes` | 背圧閾値 |
| `plugins.ipc.bulk_threshold_bytes` | これを超える結果は §6 の経路へ |
| `plugins.shutdown_grace` | 終了猶予 |

---

- 次: [composition.md](composition.md)
