# IPC プラグインシステムと MCP 連携

本ドキュメントでは、Ene のプロセス外 IPC プラグインアーキテクチャ、Protocol v6 ワイヤー仕様、Model Context Protocol (MCP) サーバー連携、および組み込みツールプラグインについて解説します。

---

## 1. プロセス外プラグインアーキテクチャ

プロセス分離、動作の安定性、セキュリティを確保するため、ツールプラグイン、カスタム LLM プロバイダ、MCP サーバーなどのすべての外部拡張機能は `PluginHostManager` (`ene-plugin-host`) によって管理される独立したサブプロセスとして動作します。

```text
Ene ホストアプリケーション (ene-runtime)
  │
  └── PluginHostManager (ene-plugin-host)
        │
        ├── IPC Protocol v6 (stdio 上の長さプレフィックス付きフレーム)
        │     ├── ene-plugin-anthropic (Anthropic LLM プロバイダプラグイン)
        │     ├── ene-plugin-openai    (OpenAI 互換プロバイダプラグイン)
        │     ├── ene-plugin-openai-tts (OpenAI Speech API TTS プロバイダプラグイン)
        │     ├── ene-plugin-llama-cpp (ローカル GGUF プロバイダプラグイン)
        │     ├── ene-plugin-kokoro     (Kokoro-TTS ONNX ローカル TTS プロバイダプラグイン)
        │     ├── ene-plugin-voicevox  (VOICEVOX / Aivis Speech TTS プロバイダプラグイン)
        │     ├── ene-plugin-edge-tts  (Microsoft Edge Neural Voice TTS プロバイダプラグイン)
        │     ├── ene-plugin-elevenlabs (ElevenLabs TTS プロバイダプラグイン)
        │     ├── ene-plugin-app       (GUI 起動ツール)
        │     ├── ene-plugin-browser   (CDP ブラウザ自動化ツール)
        │     ├── ene-plugin-calc      (計算ツール)
        │     ├── ene-plugin-calendar  (カレンダーツール)
        │     ├── ene-plugin-counter   (サンプル状態付きツール)
        │     ├── ene-plugin-fs        (サンドボックス化ファイルシステムツール)
        │     ├── ene-plugin-random    (ランダム生成ツール)
        │     ├── ene-plugin-geo       (地理情報ツール)
        │     ├── ene-plugin-git       (読み取り専用 Git ツール)
        │     ├── ene-plugin-utility   (TODO・質問・タイマー・通知ツール)
        │     └── ene-plugin-web       (Web 検索 & スクレイパーツール)
        │
        └── Model Context Protocol (MCP) ブリッジ (ene-plugin-host)
              └── 外部 MCP サーバー (Node.js / Python / Go などの MCP プロセス)
```

---

## 2. IPC Protocol v6 仕様

プラグインは `stdin`/`stdout` 上で **IPC Protocol v6** を使用して通信します：

- **フレーミング**: すべてのパケットは 4 バイトのリトルエンディアン `u32` パケットサイズで始まり、ネゴシエーションされた `WireFormat` のペイロードが続きます。ハンドシェイクのやり取り（リクエストと ack）は常に UTF-8 JSON を使用し、両者がプロトコル v6 をネゴシエーションした場合、以降のすべてのフレームは MessagePack（`rmp-serde`、マップエンコード）になります。v5 以下でネゴシエーションしたピアは接続全体で従来どおりの JSON フレーミングのままであり、N-1 プラグインは v6 以前のワイヤーとバイト互換です。
- **ハンドシェイクネゴシエーション**: ホストは `PluginIpcRequest::Handshake { version: VersionRange::host_supported() }`、すなわち単一の固定値ではなく `VersionRange { min: 5, max: 6 }` を送信します。プラグインは `VersionRange::negotiate` でその範囲と自身がサポートする範囲の共通部分を取り、両者に共通する最大バージョンを `HandshakeAck { version, capabilities: PluginCapabilities }` として返します。
- **ハンドシェイクタイムアウト**: ホストは `HandshakeAck` の待ち時間に上限を設けています（`plugins.handshake_timeout_ms`、既定 10 秒）。ソケットを accept しながら応答しないプラグインは、残りのプラグインの起動をブロックする代わりに `PluginHostError::HandshakeFailed` でハンドシェイクに失敗します。プラグイン作者はハンドシェイクに即応答し、重い初期化（モデル読み込み等）はその後へ遅延させる必要があります——`ene-plugin` の `run_plugin_server` を参照してください。
- **リクエスト相関**: 非同期リクエストおよびレスポンスはすべて必須の `request_id` (`Uuid`) を保持します。
- **ケーパビリティ**: プラグインはサポートする機能 (`tools`, `llm_providers`, `stt_providers`, `tts_providers`) を宣伝し、各プロバイダ仕様はさらに `concurrency: ConcurrencyHint` を宣言します ([§3](#3-プロバイダの並行度-concurrencyhint) 参照)。プラグイン間の能力共有は `provides` / `requires` で宣言します ([§4](#4-能力宣言-provides--requires) 参照)。

### バージョニングポリシー（N-1 後方互換）

ツール・プロバイダプラグインは独立したプロセス外バイナリとして配布されます。`PLUGIN_IPC_PROTOCOL_VERSION` を上げても既存のプラグインバイナリは再コンパイルされないため、ホストは**1バージョン分の後方互換**を維持します。

- ホストは常に単一の固定バージョンではなく `[PLUGIN_IPC_MIN_SUPPORTED_VERSION, PLUGIN_IPC_PROTOCOL_VERSION]`（`crates/ene-plugin-proto/src/ipc.rs` の `VersionRange::host_supported()`）をハンドシェイクで提示します。1つ前のプロトコルバージョンでビルドされたプラグインでも、そのバージョンでネゴシエートして接続できます。
- プラグインバイナリ側は範囲をサポートする必要はなく、自身がビルドされたバージョンを `VersionRange { min: N, max: N }` として申告してよいです。互換性を維持する責務はホスト側に集約されており、個々のプラグイン作者に強制されません。
- **プロトコルバージョンのバンプ**: `PLUGIN_IPC_PROTOCOL_VERSION` を上げる際は `PLUGIN_IPC_MIN_SUPPORTED_VERSION` も同じ数だけ繰り上げ、最も古いサポート対象バージョンのサポートを打ち切ります。
- **バンプが必要なケース**: 既存メッセージの意味変更、必須フィールドの追加、enum variant の削除・リネームの場合のみです。新しいフィールドは `#[serde(default)]` を使うことで、バージョンバンプなしに新旧のピア間で互換性を保てます。
- **機能ゲート**: ホストはネゴシエート済みバージョンを `ene-plugin-host` の `IpcPluginConnection` に保持し、`negotiated_version()` で参照できます。最小サポートバージョンより後に追加されたメッセージに依存する挙動はこれをもとにゲートすべきです。たとえば `supports_set_config()` は v5 で追加された `PluginIpcRequest::SetConfig` をゲートしています。現在の N-1 ウィンドウ（v5 以上）ではすべてのピアがこの variant を認識するため、ライブプッシュは常に適用され、このチェックは最小バージョンより後に導入される機能向けのバージョン相対パターンとして残されています。動的設定メッセージ（`ListConfigOptions`、`ValidateConfig`、`MigrateConfig`）はプロトコル ≥ v5 **かつ** 対応する `PluginCapabilities` フラグ（`supports_list_config_options` など。当該 variant を知らない古い v5 バイナリでは serde デフォルトの `false`）が必要です。
- **ネゴシエーション失敗の診断**: プラグインが提示する範囲とホストのサポート範囲が重ならない場合、プラグイン側の `HandshakeAck` エラーおよびホスト側の `PluginHostError::HandshakeFailed` / `ProtocolMismatch` はいずれも双方の範囲を明記します（例: "host supports 5..=6, plugin supports 3..=3"）。これにより、単なる汎用的なハンドシェイク失敗ではなく、プラグインバイナリの再ビルドが必要であることが開発者に伝わります。

---

## 3. プロバイダの並行度 (`ConcurrencyHint`)

プロセス境界は「ホスト」を不正なプロバイダプラグインから守ります——不正なプラグインがホストの tokio ブロッキングプールを枯渇させることはできません。しかし、それだけでは「プラグイン自身」は守られません。ホストが単一のプラグインバイナリに対して無制限に同時リクエストを送ることを妨げるものは何もなく、また「自分はローカルモデルなので一度に一件ずつ実行してほしい」とプラグインが表明する手段もありませんでした。`ConcurrencyHint` はこのギャップを埋めます。

`PluginCapabilities` の各エントリ——`LlmProviderSpec`、`TtsProviderSpec`、`SttProviderSpec`——は `concurrency: ConcurrencyHint` フィールドを持ちます。

```rust
pub struct ConcurrencyHint {
    /// Max jobs this provider can run at once.
    pub max_in_flight: u32,
    /// Extra jobs to queue before rejecting.
    pub queue_depth: u32,
}
```

### デフォルトは意図的に直列

`ConcurrencyHint::default()` は `max_in_flight: 1, queue_depth: 2` ——一度に1ジョブのみ、その後ろに浅いキューを1つ、です。これは見落としではなく、設計上の要となる決定です。並行性についてまったく考えていないプラグイン作者は、考えていない「からこそ」保守的で安全な挙動を得られます。より高い並行度を求めるプラグイン——典型的にはクラウド API へのステートレスな HTTP プロキシで、多数のリクエストを安全に同時処理できるもの——は `concurrency` を明示的に設定する必要があり、そう設定すること自体が、作者がこの問題を検討した証拠になります。組み込みの Anthropic プラグインはまさにこれを行っています。

```rust
fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
    vec![LlmProviderSpec {
        kind: "anthropic".to_string(),
        // ...
        concurrency: ConcurrencyHint { max_in_flight: 8, queue_depth: 16 },
    }]
}
```

このフィールドは、v3 以降にこのワイヤープロトコルへ追加された他のすべてのフィールドと同様に `#[serde(default)]` です——上記の[バージョニングポリシー](#バージョニングポリシーn-1-後方互換)を参照してください。`ConcurrencyHint` が存在する前にビルドされたプラグインバイナリは単にこのフィールドをワイヤー上で省略しますが、ホストはフィールドが欠けている場合、ハンドシェイクを失敗させたり無制限の並行度を既定にしたりするのではなく、安全な直列デフォルトとしてデシリアライズします。追加にあたってプロトコルバージョンのバンプは不要でした。

### ホスト側での強制

`ene-plugin-host` の `IpcLlmProvider` (`crates/ene-plugin-host/src/ipc_provider.rs`) は、宣言されたヒントを `ConcurrencyLimiter` で強制します: `max_in_flight` にサイズを合わせた `tokio::sync::Semaphore` と、パーミットを待てる呼び出し元を最大 `queue_depth` 件まで許容する仕組みです。両方の上限を超えたリクエストは、待ちキューを無制限に伸ばすのではなく `LlmProviderError::Busy` で即座に失敗します——`ene-infer` がローカル推論側で適用しているのと同じ「無限にキューイングするより早く失敗させる」規律を、プラグイン IPC 境界にも適用したものです。このリミッタは (プラグイン, プロバイダ種別) のペアごとに `IpcLlmProviderFactory` の中で一度だけ構築され、そのペアに対して以降作成されるすべてのプロバイダインスタンスで共有されます。呼び出しのたびに新しい `IpcLlmProvider` が作られるためです。ストリーミングリクエストの場合、取得したパーミットはストリームの生存期間中ずっと保持され、ストリームが自然に完了したか途中でキャンセルされたかにかかわらず、ストリームが drop された時点で自動的に解放されます。

TTS プロバイダプラグインも同じ規律に従います: `ene-plugin-host` の
`IpcTtsProvider` / `IpcTtsProviderFactory` (`ipc_tts.rs` / `tts_factory.rs`) は
`ene_ai::TtsProvider` / `TtsProviderFactory` を実装し、プラグインの
`tts_providers` ケーパビリティを `TtsProviderSpec.kind`（例：`"voicevox"`。
`ai.tts.provider` で選択）をキーとしてグローバルな
`AudioProviderRegistry` に登録します。合成呼び出しは 1 回の
`SynthesizeSpeech` IPC ラウンドトリップで音声ファイル全体（WAV）を返し、
ホスト側で PCM にデコードして `TtsChunk` に分割し、`TtsProvider::synthesize_stream`
の契約を保ちます。さらに `voicevox` プラグインはマネージドモード
（`auto_start: true`）でローカルの VOICEVOX 互換エンジンバイナリの起動と
監視を行います。

### クロスプロバイダ入場制御（`ResourceClass`）

`ConcurrencyHint` は 1 つのプラグインのプロバイダに対するリクエスト数を
制限しますが、**別々の**プラグインが同じ物理デバイスで競合することには
何も言いません。すべての `LlmProviderSpec` は `resource_class: ResourceClass`
の申告（ワイヤ上では `"Cpu"` / `{"Gpu":{"device":0}}` / `"Network"`。
`#[serde(default)]` で `Cpu`）を持ち、ホストは**同じクラスを申告する
すべてのプラグインで 1 つの入場予算を共有**します。クラス permit を取得
できるまでリクエストは送出されないため、デバイス 0 にオフロードする 2 つの
ローカルモデルは別々のプラグインプロセスに由来しても同時実行されません。
permit はリクエストの間ホスト側で保持され、リクエスト終了・キャンセル・
配信プラグインのクラッシュのいずれでも drop で解放されます——ホストは
クラッシュしたプロセスに何かの解放を頼ることはありません。`Gpu` クラスは
既定でゲートされ（デバイスごとに 1 ジョブ、最大 8 待機、超過は `Busy`）、
`Cpu` / `Network` は `plugins.resource_classes`（`docs/configuration.md`
参照）に列挙した場合のみゲートされるため、クラウドプロバイダーの宣言済み
プラグイン別並行度は保たれます。

`#[provider(...)]` derive では
`resource_class = ::ene_plugin::ResourceClass::Gpu { device: 0 }` と宣言
できます（省略時は `Cpu`）。組み込みのローカル GGUF プラグインは 1 つの
バイナリで CPU モデルと GPU モデルの両方を提供できるため、`acceleration`
設定から動的に申告します。`EmbedPlugin` にはまだワイヤ上の申告面が無い
ため、埋め込みリクエストはホスト側ではゲートされません — GPU 埋め込みを
実行するプロバイダーは、プロセスローカルのバックストップとして自前の
プロセス内入場制御を維持します。

`kokoro` プラグイン（`plugins/provider/kokoro`）は、ローカルの
Kokoro-82M ONNX モデルを自プロセス内で直接実行します（`ene-voice` の ONNX
エンジン経由）。API キー・外部エンジン・ローカルサーバーは不要です。モデル
は最初の利用時に遅延ロードされ、24 kHz モノラル WAV を出力します。解決済み
設定（モデル/ボイスパス・ボイス・速度・言語）が変わるとエンジンを再構築
します。

`edge-tts` プラグイン（`plugins/provider/edge-tts`）は、同じ `TtsPlugin`
契約を Microsoft の無料・キー不要な Edge 読み上げ WebSocket エンドポイント
に対して実装します。ブラウザ拡張機能のハンドシェイク（`TrustedClientToken`
と `Sec-MS-GEC` のクエリパラメータ・ヘッダー）を模倣し、48 kbps モノラルの
MP3 フレームを受信してプロセス内でデコード（`nanomp3`）し、WAV 音声を返します
—— API キーもエンジンもローカルサーバーも不要です。接続が失われた合成呼び出し
は指数バックオフで再試行します。

`elevenlabs` プラグイン（`plugins/provider/elevenlabs`）は、同じ `TtsPlugin`
契約を ElevenLabs API に対して 2 つのトランスポートで実装します: REST の
`POST /text-to-speech/{voice_id}/stream` エンドポイント（既定）と、低遅延
ストリーミング用の双方向 `stream-input` WebSocket です。音声は raw 16-bit PCM
（`pcm_16000` / `pcm_24000` / `pcm_44100`）で要求し、WAV として返します。
API キーは `plugins.list.elevenlabs.config.api_key` または
`ELEVENLABS_API_KEY` 環境変数から取得し、`xi-api-key` リクエストヘッダーで
送信します—— URL には決して含めません。

### ローカル推論プラグイン作者向け: プロセス内でも同じ規律を

`ConcurrencyHint` が制限できるのは「ホスト」がプラグインに対して一度に送るリクエスト数だけです。自前でローカル推論を行うプラグイン (llama.cpp、whisper.cpp、ローカル TTS エンジンなど) は、それでも「自分自身のプロセス内」で並行性を正しく扱う必要があります——ホストは、そのプラグインのコードが到着したジョブをどう捌くかを見ることも制御することもできません。そのために、`ene-plugin` の `prelude` モジュールに依存してください。これは `ene-infer` のワーカースレッドフレームワーク (`EngineConfig`、`EngineError`、`EngineHandle`、`JobContext`、`LocalModel`、`StopReason`) を、通常のプラグイン作成用の型と並べて再エクスポートしています。

```rust
use ene_plugin::prelude::*;

struct MyLocalModel { /* ... */ }

impl LocalModel for MyLocalModel {
    type Request = MyRequest;
    type Response = MyResponse;
    type Error = MyModelError;

    fn engine_name(&self) -> &str { "my-local-model" }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        // Cooperatively check `ctx.should_stop()` at natural interruption points.
        // ...
    }
}

let handle = EngineHandle::spawn(|| Ok(MyLocalModel::load()?), EngineConfig::default());
```

`EngineHandle` は専用のワーカースレッド上でモデルを所有し、無制限に増え続けるのではなく境界のあるキューから即座に失敗する (`EngineError::Busy`) ジョブ実行を行い、パニックしたジョブからはモデルを再構築して復旧します——ホスト自身が依拠しているのと同じ保証を、`use` 一行で得られます。これがなければ、自前で書いた `spawn_blocking`/`block_in_place` による並行処理は、結局プロセス境界の向こう側にバグを移すだけになってしまいます。

### ResourceClass: プロセス内入場制御のキーがワイヤ型になる

同一の物理リソース（同じ GPU デバイス、共有の CPU クラス、ネットワーク接続クラス）を奪い合うローカルエンジンを直列化するプロセス内入場予算は、`ResourceClass` enum——`Gpu { device: u32 }` / `Cpu` / `Network`——をキーにしています。この enum は `ConcurrencyHint` と同じく `ene-plugin-proto` の `capabilities.rs` に定義され、プロセス内エンジンは `ene_ai::ResourceClass` として利用します。ホスト側リソース入場制御の後続作業では、これをプラグインの能力仕様に配線し、プラグインが自身の使用する物理リソースを申告できるようにします——境界の両側で同じ型を使い、二重定義を避けるためです。

---

## 4. 能力宣言 (`provides` / `requires`)

プラグイン同士で*能力*を共有できます。重いランタイム（推論エンジン、音声合成など）を所有するプラグインはそれを `provides` で宣言し、他のプラグインは `requires` でその能力が必要であることを宣言します。ホストは起動時にすべての宣言を索引化し、各 `requires` を提供プラグインに解決し、ハード要求が未充足のプラグインは登録を拒否します——この宣言機構がプラグイン間の能力仲介（下記 `gguf-runner` 契約）の基盤です。

### 4.1 宣言の形式

`PluginCapabilities` に 2 つのハンドシェイクフィールドが追加されています。どちらも `#[serde(default)]` で、古いプラグインバイナリは省略（＝何も宣言しない）と扱われます。プロトコルバージョンの引き上げは不要でした:

```json
provides: ["llm/chat@1", "embed@1", "gguf-runner@1"]
requires: ["gguf-runner@^1", "g2p/ja@^1?"]
```

- `provides` の各エントリは能力**参照**: `name@major`。
- `requires` の各エントリは能力**要求**: `name@[^]major[?]`。

`^` は互換性の意図表明（`^1` = 「任意の 1.x」）です。現在のワイヤ上では参照バージョンは major のみのため `^1` と `1` は同じ集合にマッチしますが、将来 minor が導入される前に消費者が意図を表明するための構文です。末尾の `?` は**ソフト要求**を表し、提供者がいなくてもプラグインは起動して内蔵実装にフォールバックできます。`?` がない場合は**ハード要求**で、提供者がいないプラグインはホストによって無効化されます。

### 4.2 能力の命名規則とバージョン方針

名前は小文字の `[a-z0-9-]` セグメントを `/` で区切った形です（例: `llm/chat`、`g2p/ja`、`gguf-runner`）。バージョンは major 単独の整数で、ワイヤ上に minor/patch は意図的に持ちません。方針:

- 能力バージョンは **semver 風（semver-ish）** であって semver そのものではありません: 互換性のある追加（新メソッド、任意フィールド）は同一 major 内に収め、既存コンシューマを壊す変更は major を上げます。能力の ABI はワイヤ ABI と同程度の慎重さで扱ってください——第三者による宣言はリリースより長生きする約束です。
- major は慣行として `1` から始めます。`0` はパースできますが公開契約ではありません。pre-1.0 の流動的な能力を消費者が当てにしてはいけません。
- 能力の*追加*は互換ですが、*意味の変更*（実装だけでなく契約の変更）は major バンプが必要です。
- 不正なエントリ（文字セット違反、`@` 欠落、非数値 major、先頭ゼロなど）は 1 件ずつホストの警告付きで破棄されます——1 つのタイプミスでプラグインのハンドシェイク全体が失敗することはありません。

### 4.3 ホスト側の解決

ホストはすべてのプラグインのハンドシェイク宣言から能力レジストリを構築してから、ツールやプロバイダを登録します:

- **ハード要求が未充足** → そのプラグインは一切登録されません（ツールもプロバイダも監視もなし）。`RequirementsUnmet` ヘルス診断が未充足要求の一覧付きで発行されます。復旧はホスト再起動、または提供者を含む再設定です。
- **ソフト要求が未充足** → プラグインは通常どおり起動し、警告のみ記録されます。フォールバックはプラグイン側の責務です。
- **決定的な勝者**: 複数のプラグインが同じ能力を提供する場合、解決はプラグイン名の辞書順最小を選びます（`plugins.list` はマップのため設定順はタイブレーカーにできません）。明示的な提供者優先指定は将来の課題です。
- **推移的**: 未充足で無効化されたプラグインは提供者として数えないため、その能力を要求する消費者も無効化されます（ゲートは fixpoint まで評価されます）。
- **自己解決は許可**: プラグインの `requires` は自身の `provides` で充足できます。仲介経由で自身の能力を*呼べる*かどうかは別の ACL 判断です。

### 4.4 `gguf-runner@1` 能力契約

`gguf-runner@1` は**任意の GGUF モデルをロードして推論する**能力です——サードパーティの GGUF モデルプロバイダが自前の llama.cpp を同梱する代わりに借りるランタイムです（N 個の同梱ランタイムは N 個の GPU コンテキストと N 倍の VRAM を意味します）。GGUF モデルを提供したいプラグインは次のように宣言します:

```json
requires: ["gguf-runner@^1"]
```

runner API は設計上**非ストリーミング**です。トークンストリーミングが必要な消費者は、モデルプロバイダから直接 `llm/chat@1` を要求する設計にしてください。仲介（プラグインがホスト経由で `gguf-runner` を呼ぶ仕組み）は runner 実装とともに実装されますが、能力レベルの契約はここで固定します:

| メソッド | リクエスト | レスポンス |
|---|---|---|
| `generate` | `{ model, prompt, json_schema? }` | `{ text }` |
| `embed` | `{ model, texts: [string] }` | `{ embeddings: [[number]] }` |
| `unload` | `{ model }` | `{ ok: true }` |

`model` は提供プラグインに設定されたモデルプロファイルを識別します。`json_schema`（指定時）は `generate` を構造化出力に制約します。`unload` はロード済みモデルの常駐メモリ（VRAM）を解放し、将来のリソース常駐管理のフックです。これらのメソッド名とペイロード形状が第三者実装の対象となる契約で、ワイヤエンコーディングは仲介層とともに定義されます。

`gguf-runner@1` を提供する組み込みプロバイダは `ene-plugin-llama-cpp`
(`plugins/provider/local-llm`) で、`llm/chat@1` と `embed@1` も宣言し、
プロバイダ IPC 越しにチャットストリーミング・JSON スキーマ補完・GGUF
埋め込みを提供します。プラグインクレートの CPU 契約テスト（固定 GGUF
fixture 使用）で検証されます。

---

## 5. 組み込みプラグインカタログ

| プラグインバイナリ | ネームスペース | 説明 | ステートフル？ |
|---|---|---|---|
| `ene-plugin-app` | `app.*` | システムアプリ起動・ウィンドウ制御 | いいえ |
| `ene-plugin-browser` | `browser.*` | ヘッドリス Chrome/CDP ブラウザ自動化 | はい (セッションストア) |
| `ene-plugin-calc` | `calc.*` | 数式評価・単位/通貨/色変換 | いいえ |
| `ene-plugin-calendar` | `calendar.*` | カレンダー単位のパーミッション・書き込み確認・空き時間検索付きローカルカレンダー | はい (ホストサービス `db`) |
| `ene-plugin-counter` | `counter.*` | サンプル状態付きツール: DB バックアップのカウンターと権限ゲート付きリセット | はい (ホストサービス `db`) |
| `ene-plugin-fs` | `fs.*` | サンドボックス化ファイル操作 & Undo 履歴 | はい (ホストサービス `db`) |
| `ene-plugin-random` | `random.*` | 乱数・UUID v4・リスト選択・hex 色生成 | いいえ |
| `ene-plugin-geo` | `geo.*` | IP ベース位置情報・現在の天気・太陽時 UTC オフセット・日の出日の入り | いいえ |
| `ene-plugin-git` | `git.*` | 読み取り専用 Git 検査: ステータス・差分・履歴・ブランチ・リモート・責務追跡 | いいえ |
| `ene-plugin-homeassistant` | `homeassistant.*` | Home Assistant スマートホーム連携: エンティティ状態取得・スイッチ/照明/プラグ制御・空調温度設定 | いいえ |
| `ene-plugin-utility` | `utility.*` | 質問プロンプト、TODO リスト管理、日時/システム情報、カウントダウンタイマー & デスクトップ通知（Linux・D-Bus のみ） | はい (ホストサービス `db`) |
| `ene-plugin-web` | `web.*` | Web 検索および Markdown ページ抽出 | いいえ |
| `ene-plugin-anthropic` | Provider | Anthropic Claude プロバイダプラグイン | いいえ |
| `ene-plugin-openai` | Provider | OpenAI 互換プロバイダプラグイン（チャット・ストリーミング・埋め込み） | いいえ |
| `ene-plugin-openai-tts` | Provider | OpenAI Speech API TTS プロバイダプラグイン（tts-1 / tts-1-hd）— WAV（24 kHz PCM）音声 | いいえ |
| `ene-plugin-edge-tts` | Provider | Microsoft Edge Neural Voice TTS プロバイダプラグイン — 無料・キー不要の WebSocket（24 kHz MP3 を WAV にデコード） | いいえ |
| `ene-plugin-elevenlabs` | Provider | ElevenLabs TTS プロバイダプラグイン（REST + WebSocket ストリーミング）— WAV（16-bit PCM）音声 | いいえ |
| `ene-plugin-llama-cpp` | Provider | ローカル GGUF (llama.cpp) プロバイダプラグイン — チャットストリーミング・補完・GGUF 埋め込み | いいえ |
| `ene-plugin-kokoro` | Provider | ローカル Kokoro-82M ONNX TTS プロバイダプラグイン — プロセス内 ONNX 推論による 24 kHz WAV | いいえ |
| `ene-plugin-voicevox` | Provider | VOICEVOX / Aivis Speech TTS プロバイダプラグイン — 2 段階 `audio_query` → `synthesis` フローによる WAV 音声 | いいえ |

上記 19 プラグインはすべてデフォルトの `plugins.list` に含まれており、
新規インストール時に自動的に起動します。

### ファイルツールリファレンス (`filesystem.*`)

ファイルシステムプラグインは read / write / edit / delete、glob・正規表現
検索、unified diff パッチ適用、シェル実行を提供します。検索系アクションは
以下のとおりです。

**`filesystem.grep`** — 正規表現でファイル内容を検索します。オプションパラメータ:

| パラメータ | 型 | デフォルト | 説明 |
|---|---|---|---|
| `pattern` | string | — (必須) | 検索する正規表現 |
| `path` | string | cwd | 検索対象のベースディレクトリまたはファイル |
| `include` | string | なし | ファイル名 glob フィルタ — 1 回の呼び出しにつき 1 パターンのみ (例: `*.rs`; `{a,b}` のブレース展開は非対応) |
| `case_insensitive` | boolean | `false` | 大文字小文字を区別しない |
| `line_numbers` | boolean | `true` | 各マッチに 1 始まりの行番号を付ける |
| `context_lines` | integer | `0` | 各マッチの周囲に出力する非マッチ行数 |
| `count` | boolean | `false` | ファイルごとと合計のマッチ数のみ出力する |

パターンがキャプチャグループを含む場合、マッチしたグループ値が各行の下に
`Captures: 1="…", 2="…"` として出力されます（参加しなかったグループは
`(none)` と表示）。`count` を指定しない限り、1 回の呼び出しあたり最大
100 マッチに制限されます。

**`filesystem.regex.test`** — 正規表現が文字列にマッチするかを判定し、
`true` / `false` を返します。`text`（テスト対象の文字列）と `pattern`
（正規表現）を受け取ります。マッチは文字列内のどこでも判定され、
`filesystem.grep` と同じセマンティクスです。無効なパターンはエラーとして
報告されます。エージェントが「この文字列はこのパターンにマッチするか」を
ファイルシステムに触れずに判定するのに便利です。

---

## 6. ツール DB スキーマの宣言と進化

ステートフルなツールプラグイン (`ene-plugin-fs`、`ene-plugin-utility`、
`ene-plugin-calendar`、`ene-plugin-counter`) は、共有**ホストサービス**ソケット
(`ene-host-service.sock` / named pipe) を
介してデータをホストの `memory.db` に永続化します。最初のフレームで
事前共有トークン付きの乗客サービスを開き、現状実装されているのは `db`
のみです (`ene-store` の `host_service` + `db_server`)。予約 ID
(`assets` / `capability` / `credential`) は実装まで拒否されます。
全プラグインがこの単一ソケットを共有するため、ネームスペースの隔離は
プラグインごとの認証トークンのみに依存します (プラグインごとのソケット
パス層は廃止されました)。
プラグインが直接 DDL を発行することはありません。`DeclareSchema`
リクエストでテーブル・列・インデックスを宣言し、ホストが物理テーブルを
作成・所有します。すべてのテーブル名はプラグインのプレフィックス
(`fs_`、`utility_`) で始まる必要があり、すべてのインデックス名にもその
プレフィックスを含める必要があります (SQLite のインデックス名は
データベース全体で一つのネームスペースを共有するため、プレフィックスの
ないインデックスは、将来コアのマイグレーションが必要とする名前を
先取りできてしまいます)。後続のリクエストはすべて宣言に対して検証されます。

### フィンガープリントベースの変更検知

`DeclareSchema` のたびに、ホストは宣言をハッシュ化 (BLAKE3) し、内部テーブル
`__tool_schemas` に保存された `fingerprint` と比較します。

| 変更内容 | 挙動 |
|---|---|
| 変更なし | 保存済み行は変更されず、既存テーブルが再利用されます。同一スキーマの再宣言は行を**無駄に書き換えません**。 |
| 列の追加 | `ALTER TABLE ... ADD COLUMN` でその場で適用され、既存行には列の `DEFAULT` (または `NULL`) が入ります。保存済み宣言は更新されます。 |
| テーブルの追加 | `CREATE TABLE IF NOT EXISTS` で作成されます。保存済み宣言は更新されます。 |
| インデックスの追加 | `CREATE INDEX IF NOT EXISTS` で適用されます。 |
| ツールのプレフィックスを含まないインデックス名 | **拒否**されます (パーミッションエラー。インデックス名は SQLite のデータベース全体ネームスペースを共有します)。 |
| 列の型変更 | `SCHEMA_CONFLICT` エラーで**拒否**されます。 |
| テーブル/列の削除 | `SCHEMA_CONFLICT` エラーで**拒否**されます。 |
| `PRIMARY KEY`/`UNIQUE`/`AUTOINCREMENT` 付き列の追加 | `SCHEMA_CONFLICT` エラーで**拒否**されます。 |
| `DEFAULT` なしの `NOT NULL` 列の追加 | `SCHEMA_CONFLICT` エラーで**拒否**されます。 |

`SQLite` は列の型変更、列・テーブルの削除、制約付き列の追加をその場で行えず、
`NOT NULL` 列の追加には既存行を埋める `DEFAULT` が必要です。検証層と物理
テーブルが暗黙に食い違う——検証は通るのに後で `INSERT` が
`no such column` で失敗するという #423 の症状——のを防ぐため、ホストは互換性の
ない変更を拒否し、プラグイン作者に明示的な調整を求めます。追加のみの変更
(通常の新しい列とテーブル) は安全であり、自動的に適用されます。

### プラグイン作者向けガイド

- 列やテーブルの追加は安全であり、次回の `DeclareSchema` 時に既存データベースへ
  自動的に適用されます。
- 列の型を変更したりテーブルを削除したりする場合は、新しいプレフィックス付き
  テーブルを用意して自分でデータを移行するか、プラグイン自身のロジック内で差分を
  調整してください。ホストが作者の代わりにデータを書き換えたり削除したりすること
  はありません。

### 原子的バッチ (`Batch`)

複数の書き込みを原子的に適用する必要があるプラグインは、`DbWriteOp`
(`Insert` / `Upsert` / `Update` / `Delete`) のリストを載せた単一の `Batch`
リクエストを送ります。サーバーはまずすべての操作を宣言済みスキーマに対して検証し、
その後リスト全体を**1つの SQLite トランザクション**内で実行します: すべての操作が
コミットされるか、いずれかの操作が失敗した場合はバッチ全体がロールバックされ、
何も永続化されません。レスポンスはリクエスト順に操作ごとの `DbBatchOpResult` を
1つずつ持ちます。失敗時にはサーバーは失敗した操作のインデックスを名指しする
`Error` を返します。

トランザクションは単一リクエストのスコープに閉じており — IPC の往復をまたいで
保持されることは決してないため — プラグインが SQLite の書き込みロックをピン留め
して開きっぱなしにすること (これはコア自身の記憶書き込みを停滞させます) はできず、
切断された接続が中途半端に適用されたバッチを残すことも決してありません。これは、
明示的な `Begin`/`Commit`/`Rollback` を IPC 越しに露出する代わりに意図的に選ばれた
代替案です: バッチは「複数行を原子的に書き込む」ケース (例: 複数行の undo エントリ
の記録) を、ロックを長時間保持する危険なしにカバーします。`Batch` は、上記の stdio
プロトコルと同じ追加専用の規律に従い、プロトコルバージョンの引き上げなしに新しい
リクエスト/レスポンスバリアントとして追加されました。

### プラグインごとのストレージクォータ

すべてのステートフルなプラグインは**1 つの共有 `memory.db`** に書き込むため、
プレフィックス分離だけではプラグインがディスクを埋め尽くすことを防げません:
サードパーティ製（あるいは単にバグのある）プラグインのログ記録ループが、その
テーブルを無制限に肥大化させ、データベースを膨張させて記憶システムのクエリ・
バックアップ・`PRAGMA integrity_check` を劣化させかねません。これを抑制するため、
各プラグインにはストレージクォータ — `plugins.list.<name>.db_quota_mb`、既定
`256` MiB (#424) — が設けられています。

ストレージを増やす書き込み (`Insert`/`Upsert`、単独または `Batch` 内) の前に、
ホストはプラグインの使用量を、その宣言済みテーブル全体の全セルのバイト長合計 —
テーブルごとの `SUM(length(CAST(col AS BLOB)))` — によって測定し、使用量が上限に
達すると `QUOTA_EXCEEDED` エラーで書き込みを拒否します。`SQLite` にはテーブルごとの
サイズ API がないため（`dbstat` 仮想テーブルはバンドル版 `libsqlite3-sys` に
コンパイルされていません。#350 参照）、このペイロード合計をプレフィックスに
スコープされた忠実な近似値として用います。これはわずかな過小評価（行ごとの
オーバーヘッドとインデックスページを除外）ですが、ソフトキャップとしては許容
範囲です。測定はプラグイン自身のテーブルのみを読み取り、データベース全体を
スキャンすることはありません。

読み取り (`Select`/`Count`) と削除は**一切**制限されないため、クォータに達した
プラグインでも常に行を削除して空きを確保し、書き込みを再開できます。クォータを
超えるバッチは原子的にロールバックされ、失敗した操作を名指しします。無制限の
ストレージが正当に必要なプラグインには、`db_quota_mb` を `null` に設定して強制を
無効化できます。

---

## 7. プラグインセキュリティモデル

### オプトイン型ディスカバリ

プラグインのディスカバリは**オプトイン方式**です。`plugins.list` に明示的に
`enable: true` で登録されたバイナリのみが起動します。プラグインディレクトリに
バイナリを配置するだけでは実行されず、ホストは設定への追加を促す警告を
ログに出力します。これにより「バイナリ配置 → 自動実行」の攻撃経路を遮断します。

```jsonc
// settings.json (抜粋)
{
  "plugins": {
    "list": {
      "fs": { "enable": true },
      "anthropic": { "enable": true, "env_passthrough": ["ANTHROPIC_API_KEY"] },
      "openai": { "enable": true, "env_passthrough": ["OPENAI_API_KEY"] }
    }
  }
}
```

### バイナリ解決順序 (組み込み優先)

ホストがプラグイン名をバイナリへ解決する際 (`find_plugin_binary`)、
固定の順序で探索し、**最初に一致したものが採用されます**:

1. `<builtin>/ene-plugin-{name}`
2. `<builtin>/{name}`
3. `<user>/ene-plugin-{name}`
4. `<user>/{name}`

組み込みディレクトリが先に探索されるため、ユーザーが user plugins
ディレクトリに配置したバイナリは、同名の組み込みを**決して**シャドウ
できません — 常に同梱バイナリが実行されます。これは意図的で、セキュリティ上
保守的な選択です: 信頼されないドロップインが、信頼された組み込み (資格情報の
trust gate が依拠しているもの) を暗黙に置き換えることを防ぎます。実用上の帰結
として、組み込みと同じ名前のバイナリを配置したユーザーには、自分のものではなく
組み込みが実行されて見えます。挙動を上書きするには、別のプラグイン名を選んで
ください。

ある名前が trust gate において*組み込み*とみなされるかどうかは、ファイル
システムの探索ではなく、**コンパイル時に埋め込まれた**同梱プラグイン名の
リストで判定されます。debug ビルドでは組み込みディレクトリが実行ファイルの
ディレクトリ (`target/debug/...`) に解決されるため、ファイルシステムのチェック
では、そこに置かれた任意の `ene-plugin-*` バイナリが信頼された組み込みを装えて
しまいます。固定リストにより、trust gate はディスク上に何が存在するかから
独立したまま保たれます。

### 環境変数のハードニング (`env_clear`)

すべてのプラグインおよび MCP stdio サーバーは `env_clear()` 付きで起動されます。
継承された環境変数は消去され、明示的なホワイトリストのみが転送されます：

| 変数 | 用途 |
|---|---|
| `PATH` | システム実行ファイルの探索 |
| `HOME` | ユーザー設定ファイル |
| `TMPDIR` | 一時ファイル |
| `LANG` | ロケール依存出力 |
| `TZ` | タイムゾーン (設定時のみ) |
| `LD_LIBRARY_PATH` | 共有ライブラリ読み込み (Linux) |
| `SystemRoot`, `USERPROFILE`, `APPDATA`, `TEMP`, `PATHEXT` | Windows 必須変数 |
| `ENE_PLUGIN_SOCKET` | IPC チャネル (プラグインのみ) |

### プラグインごとの `env_passthrough`

追加のホスト環境変数 (API キーなど) が必要なプラグインは、`plugins.list`
エントリの `env_passthrough` で明示的に宣言します。セキュリティ上危険な名前
(`LD_PRELOAD`、`LD_AUDIT`、`DYLD_INSERT_LIBRARIES`、`ENE_PLUGIN_SOCKET` など)
は設定に関係なくブロックする組み込み拒否リストが適用されます。

プラグインの環境変数フォールバックは、そのエントリが転送して初めて機能します。
たとえば `calc.currency_convert` は `EXCHANGERATE_HOST_API_KEY` にフォール
バックしますが、デフォルトの `calc` エントリは変数を一切転送しないため、
`plugins.list.calc.env_passthrough = ["EXCHANGERATE_HOST_API_KEY"]` を設定するか、
代わりに `plugins.list.calc.config.exchangerate_host_access_key` を構成してください。

MCP stdio サーバーも `plugins.mcp_servers` エントリに同じ `env_passthrough`
フィールドをサポートしています。

### プラグイン設定フロー (`set_config` / `set_profiles`)

ツールプラグイン・**プロバイダープラグインの両方**が、IPC ハンドシェイク時およびライブ更新 (`PluginIpcRequest::SetConfig`、protocol v5+) でホストから設定を受け取ります。`plugins.list.<name>.config` ブロブは `ConfigurablePlugin::set_config` 経由でそのまま配信され、`plugins.list.<name>.profiles.<profile>` マップ（モデル/音声ごとの設定用）は `ConfigurablePlugin::set_profiles` 経由で配信されます。どちらもホストからは不透明です。ホストはそれらをそのまま保存し、キーを解釈せず、プッシュ前に接続キャッシュを更新し、再接続時にも再送信します。設定のホットリロードで enable 集合は変わらず config/profiles だけが変わった場合、ランタイムはプラグインホストを再起動せず生きている接続へ `SetConfig` を送ります。ホストの N-1 ウィンドウ（v5 以上）のすべてのピアが `SetConfig` を理解するため、ライブプッシュは常に適用されます。プロバイダープラグイン（LLM/embed/TTS/STT）はツールプラグインと同じ配信を受けるため、たとえば Anthropic プロバイダーは API キーをリクエストごとではなくハンドシェイク時に受け取れます。

ネストした `config` オブジェクト内にホスト予約キー（`enable`、`checksum`）を置かないでください——`plugins.list.<name>` のエントリフィールドと衝突します。ホストは配信ブロブにこれらのキーが含まれる場合に警告します。

プラグインは `config_schema()` で受け付ける設定の JSON Schema を広告します。そのスキーマで `x-ene-secret: true` とマークされたフィールドは、UI でマスクされる予定で、ホストのログからは redact されます（正確な形は [`configuration.md`](../ja/configuration.md) を参照）。

`GetConfigSchema` は実行時に再取得できます。外部エンジン接続後に初めて選択肢が分かるプラグインは、`DeferredCompleted` と同様にルーティングされる `ConfigSchemaChanged` を push できます。オプトインの capability フラグにより `ListConfigOptions`（動的 enum）、`ValidateConfig`（フィールド横断エラー）、`MigrateConfig`（`config_version` 自己移行）が有効になります。フラグを省略したピアは静的スキーマ + ホスト側 JSON Schema 検証のみに degrade し、マイグレーションは行いません。これらの API の UI 配線はここでは対象外です。

### 資格情報の宣言 (`x-ene-credentials`)

資格情報（API キー、OAuth2 トークン）が必要なプラグインは、`config_schema()`
が返すスキーマのトップレベルで、既存の `x-` マーカーと同じ場所に宣言します：

```json
{
  "type": "object",
  "properties": { "voice": { "type": "string" } },
  "x-ene-credentials": [
    { "id": "anthropic", "kind": "api_key", "required": true,
      "header": { "name": "x-api-key", "format": "{value}" },
      "env_fallback": "ANTHROPIC_API_KEY",
      "label": "Anthropic API Key",
      "help_url": "https://console.anthropic.com/settings/keys" },
    { "id": "google.calendar", "kind": "oauth2",
      "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
      "auth_url": "https://accounts.google.com/o/oauth2/v2/auth",
      "token_url": "https://oauth2.googleapis.com/token",
      "label": "Google カレンダー" }
  ]
}
```

両 kind 共通のフィールド: `id` は資格情報の安定 ID、`required`（既定
`false`）は必須資格情報であることを示し、`shared`（既定 `true`）は名前空間
共有を制御します（後述）。`label` / `help_url` は設定 UI の生成に使われます。
ID は `[A-Za-z0-9._-]` を受け付け、先頭・末尾に `.` を置けません —
`anthropic`、`google.calendar`、`google-calendar` はいずれも有効です。

- `kind: "api_key"` — 静的シークレット。`header`（任意）はクライアントが
  値を注入する方法を指定します。`format` は `{value}` を含むテンプレート
  である必要があります（例: `Bearer {value}`）。`env_fallback` は値が保存
  されていない場合にホストが確認する環境変数名です。
- `kind: "oauth2"` — ホストが駆動する OAuth2 フロー。`scopes` は同意画面の
  スコープ一覧、`auth_url` / `token_url` は認可エンドポイントとトークン
  エンドポイントです。

**共有ポリシー。** 宣言は既定で共有されます。両方のプラグインが `anthropic`
を宣言していれば同じ保存値を参照するため、プロバイダーを差し替えてもキーを
再入力する必要がありません。共有は「宣言した者同士でのみ」で、`anthropic`
を宣言していないプラグインは、vault に値が存在しても拒否されます。
`"shared": false` を書けばオプトアウトでき、その場合 `<plugin>:<id>` の
プラグイン専用の名前空間に解決されます。

`:` セパレータにより、private キーが共有宣言と衝突することは構造的に
ありえません。`:` は id の文字集合（`[A-Za-z0-9._-]`）にもプラグイン名の
文字集合（`[A-Za-z0-9_-]`）にも含まれないため、共有 id は private キーの形を
名乗れません。プラグイン A の private `anthropic`（`A:anthropic`）と、プラグイン
C が id `A.anthropic` を共有した場合のキー（`A.anthropic`）は、別途の
一意性 invariant なしに異なるキーへ解決されます。

**検証のタイミング。** 宣言はプラグイン起動時に検証されます。エントリは
それぞれ独立にチェックされ、不正なエントリは警告のうえ無視されます（プラグイン
自体は起動します）。同一 id の重複は最初の宣言が採用されます。要求時の強制は
credential サービス側にあり、要求プラグインの登録済み宣言に対してのみ照合し、
宣言外の id を拒否します。値の形式検証（例: `sk-ant-` 接頭辞）は保存時に
プラグイン自身の `ValidateConfig` へ委譲されます。

### バイナリチェックサム検証 (TOFU)

初回起動時にホストはプラグインバイナリの SHA-256 チェックサムを計算し、
`plugins.list.<name>.checksum` に記録します (Trust-on-First-Use)。
以降の起動ではバイナリを記録済みチェックサムと照合し、変更があれば
起動を拒否します。比較は大文字小文字を区別しません (16進エンコーディング)。

このチェックサムは監視下での再起動のたびにも再検証されます。ホストは
クラッシュまたは応答なしのプラグインを kill して再 spawn する前に、
ディスク上のバイナリのチェックサムを再計算し、起動時にピン留めした値と
比較します。ホストの稼働中にバイナリが変更された場合 (たとえば開発中に
`cargo build` がバイナリを置き換えた場合など)、再起動は中断され、その
プラグインはホスト自体を再起動するまで**永続的に無効化**されます。これは
意図的な動作です。稼働中のインスタンスは元のバイナリに対して検証済みのため、
ホストは別のバイナリを暗黙に exec することを拒否します。新しいバイナリを
反映するにはホストを再起動し、そのチェックサムを再度ピン留めしてください。

### プロセス監視と再起動予算

起動したすべてのプラグインは、定期的なヘルスプローブによって監視されます
(`plugins.health_interval_ms`、デフォルト 30 秒。`0` でプローブ無効)。
各ティックでホストはプラグインへ ping し、子プロセスの生存を確認します:

- **健全** (生存しており ping に応答) → プラグインの**再起動予算が回復**します
  (ゼロにリセット)。
- **不健全** (死亡または応答なし) → ホストは `Unhealthy` イベントを発行し、
  指数関数的に増大するバックオフ遅延を待ってからプロセスを再起動し、
  再接続します。再起動のたびに予算を 1 単位消費します。

再起動予算は**最近の不安定性を測る回復可能なスライディング尺度であり、
生涯上限ではありません**。健全なラウンドトリップを挟まずに `MAX_RESTARTS`
(5回) 再起動すると、プラグインはホスト自体を再起動するまで**永続的に
無効化**されます (`PluginHealthEvent::Disabled`、理由
`restart_budget_exhausted`)。しかし健全なラウンドトリップがあれば予算は
再びゼロに戻るため、1日に1回クラッシュするもののそれ以外は正常に動作する
プラグインは予算を蓄積せず、真のクラッシュループ (健全な区間を挟まない
繰り返しの失敗) のみが予算を使い果たして停止されます。

予算の回復は、プラグインの機能に関係なく、**あらゆる**健全なラウンド
トリップで起こります:

- 成功した**ツール呼び出し** (ツールを公開するプラグイン)、および
- 健全な**ヘルスプローブ ping** (監視下のすべてのプラグイン)。

ヘルスプローブ経路こそが、**provider 専用プラグイン** (たとえば組み込みの
`anthropic` プロバイダ) で回復を機能させる仕組みです。これらはツールを
公開しないためツールレジストリを構築せず、予算をリセットする成功した
ツール呼び出しも起こりません。プローブによる回復がなければ、その予算は
ホストセッション全体でクラッシュ5回という生涯上限になってしまいます —
長時間起動しっぱなしのデスクトップコンパニオンには致命的です。ping には
応答するが実際の処理は壊れているプラグインも別途封じ込められます。
レジストリごとのサーキットブレーカーが連続した呼び出し失敗でトリップするため、
ping ベースの回復が壊れたプラグインにトラフィックを提供させ続けることは
ありません。

### カレンダーツール: 確認とプライバシー制御

`ene-plugin-calendar` は、上記の対話型パーミッション契約をすべての変更系
操作に適用し、さらに**カレンダー単位のパーミッションフラグ**を重ねます:

- **カレンダー単位のパーミッション。** 各カレンダーアカウント行には
  `read_allowed` / `write_allowed` フラグがあります。新規カレンダーは
  読み取り許可・書き込み拒否が既定です (deny-by-default)。
  `calendar.set_permission` でフラグを変更できますが、これ自体もユーザー
  承認が必要です。読み取り (`calendar.list_events`、
  `calendar.find_free_slots`) は読み取り権限のないカレンダーに対して
  fail-closed となり、書き込み (`calendar.create_event`、
  `calendar.update_event`、`calendar.cancel_event`、
  `calendar.remove_account`) は書き込み権限がないと実行できません。
- **プレビュー付き書き込み確認。** すべての変更系アクションはストアに
  触れる**前**に `PermissionRequired` を返します。ユーザーに表示される
  `description` にはタイムゾーン・対象カレンダー・変更内容がプレビュー
  され、`update_event` は変更前 → 変更後の差分を表示します（タイムゾーン
  のみの変更も含む）。リクエスト ID は `(action, target, description)` の
  決定的ハッシュなので、承認後の再呼び出し（同一引数のリプレイ）は
  記録済みの承認と一致して再プロンプトされません。一方で説明文が変われば
  （イベント内容が異なれば）新しい承認が必要です。1回限りの承認はターン
  境界で失効します（プラグインがホストのコールコンテキスト更新時に消去）。
  「セッション中許可」は `(action, target-prefix)` パターンとして記録され、
  会話の間ゲートを通過します。
- **プライバシー。** 予定の本文（タイトル・メモ・参加者）はプラグインの
  ログやホストの監査ログに一切登場しません。パーミッションの `target` は
  `calendar:<id>` / `calendar:<id>#<イベントID>` のような安定した識別子で、
  監査ログには `action`・`target`・決定のみが記録され、カレンダーの引数
  ペイロード（タイトル・メモ・参加者・場所）は永続化前にマスクされます。
  本文が表に出るのは必要な場所だけです: 承認プロンプト（ユーザー向け表示）と、
  LLM に渡るツール結果。アカウントの連携解除 (`calendar.remove_account`)
  はアカウント行と全イベントを1トランザクションで削除するため、
  即座に反映されます。
- **プロバイダ抽象化。** イベントへのアクセスはアカウント種別をキーとする
  `CalendarProvider` トレイト経由です。現時点では `local` 種別のみ
  （イベントはプラグインの `calendar_events` テーブルに保存）。外部サービス
  （Google Calendar、CalDAV 等）は、コネクターフレームワークが資格情報の
  取り扱いを提供した後、同じトレイトの新しいプロバイダとして追加できます。

---

## 8. MCP (Model Context Protocol) 連携

`ene-plugin-host` は外部 MCP サーバーを統合します (かつての `ene-connector`
ブリッジ層は #416 で撤去されました — 接続ライフサイクルはすべてプラグインホスト内にあります)：

1. **発見と起動**: ホストは `plugins.mcp_servers` 設定を読み込み、 `stdio` または HTTP/SSE 経由で対象の MCP サーバーバイナリを起動します。
2. **ツール変換**: MCP ツールは自動的に `ToolSpec` アイテムに変換され、 `CompositeToolRegistry` に登録されます。
3. **実行ルーティング**: LLM から生成されたツール呼び出しは MCP ブリッジを経由してルーティングされ、 `ene-runtime` に返却されます。

サーバー名はルーティングとツール名前空間にそのまま使用され、文字種の検証は
行われないため、 `github-mcp` のようなハイフン入り名前も他の名前と同様に接続
できます (#417)。

### HTTP URL 検証 (SSRF 対策)

HTTP の MCP エンドポイント (`transport.type = "http"`) は、接続を試みる**前に**
`McpToolRegistry::connect_http` によって URL を検証されます。既定は拒否です：

- **HTTPS のみ。** `http://` URL は拒否されます。
- **ループバック拒否。** `127.0.0.0/8` と `::1` は拒否されます。
- **リンクローカルは常に拒否。** クラウドメタデータエンドポイント
  `169.254.169.254` を含む `169.254.0.0/16` と `fe80::/10` は、いかなる設定でも
  拒否されます。

拒否は tracing ログと返却されるエラー (`PluginHostError::McpHandshake`) の
両方に、サーバー名と理由を含めて報告されます。

ローカル開発向けに、 `plugins.mcp_allow_insecure_urls` (既定 `false`) で
プレーン `http://` URL とループバックエンドポイントを許可できます：

```jsonc
// settings.json (抜粋)
{
  "plugins": {
    "mcp_allow_insecure_urls": true,
    "mcp_servers": [
      { "name": "local-dev", "enabled": true,
        "transport": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } }
    ]
  }
}
```

このオプトインはリンクローカルのブロックを緩和しません。DNS リバインディング
(内部アドレスに解決されるホスト名) はスコープ外です：検査されるのは IP リテラルの
ホストのみです。これは、本番の接続経路で検証を一切行っていなかった以前の挙動より
弱くなることはありません。

---

## 9. カスタムツールプラグインの開発

開発者は `ene-plugin` の `#[derive(ToolAction)]`（`ene-plugin-macros` 経由）とサーバーエントリポイントを使用して独自のツールプラグインを作成できます。以下は説明用のスケッチです — 実際にコンパイルが通る現行パターンは `plugins/tool/*` 配下の既存プラグイン (例: `plugins/tool/app/src/main.rs`) や `cargo doc -p ene-plugin-macros --open` を参照してください：

```rust,ignore
use ene_plugin::prelude::*;

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "custom", name = "greet",
       summary = "パーソナライズされた挨拶文を生成します。", category = "Custom",
       keywords_primary = "greet, hello")]
pub struct GreetAction {
    pub name: String,
}

impl GreetAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("こんにちは、{}さん！", self.name))
    }
}

#[tokio::main]
async fn main() {
    use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
    use std::sync::Arc;

    let provider = ActionSetProvider::new(vec![Box::new(GreetAction { name: String::new() })]);
    let dispatch = PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    );
    if let Err(e) = run_plugin_server(dispatch).await {
        tracing::error!("fatal error: {e}");
        std::process::exit(1);
    }
}
```

### 遅延（バックグラウンド）実行

`#[derive(ToolAction)]` アクションは同期的なリクエスト–レスポンスツールです。即座に戻って結果を後から届けるツール — デスクトップ通知を発火するカウントダウンタイマーや通知送信 — は `#[tool(...)]` 属性で `background_capable` を指定します。ホストはこれらを遅延 IPC 経路（`call_tool_deferred` → `task_id` → `poll_deferred` で終端状態まで）で呼び出すため、LLM のターンはブロックされません。

`ActionSetProvider` は意図的に遅延メソッドを実装しません。タスク生成・ポーリング状態・キャンセルは各バイナリの並行モデルに固有だからです。バックグラウンドツールが必要なプラグインは `ToolProvider` を手書きで実装し、同期サーフェスを内部の `ActionSetProvider` に委譲しつつ `call_tool_deferred`・`poll_deferred`・`cancel_deferred` をオーバーライドします。実装例は `plugins/tool/utility`（`TaskRegistry`・`TimerStartAction`・`NotifySendAction`）を参照してください。

ホスト側では遅延タスクは 100 ms 間隔で最大 `tools.deferred_max_polls` 回ポーリングされます（デフォルト 600 ≈ 60 秒）。この予算を超える処理は実行自体は継続されます — タイマーは数え続け、通知は発火します — が、完了イベントは LLM に配信されません。後発の結果はツール自身のステータス面で確認できます（例: `utility.timer_stop` を名前なしで呼ぶと実行中・完了済みのタイマー一覧が返ります）。
