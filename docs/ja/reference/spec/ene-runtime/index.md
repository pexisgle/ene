# `ene-runtime` クレート概要 & モジュール構成

`ene-runtime` クレートは、コンシューマーアプリケーション（`ene-cli` および `ene-desktop`）に対する最上位の公開インターフェース（ファサード）を提供します。アクター駆動型の並行処理ループである `EneActor` をスレッドセーフな `EneHandle` にカプセル化し、他の下位クレートを束ねて会話ターンおよびシステム制御を統制する役割を持ちます。

---

## 1. 依存関係と境界

### 物理的依存関係 (`Cargo.toml`)
- **依存先**: `tokio`, `tokio-stream`, `tokio-util`, `serde`, `serde_json`, `chrono`, `tracing`, `async-trait`
- **内部クレート依存先**: `ene-ai`, `ene-config`, `ene-mind`, `ene-store`, `ene-tool-host`, `ene-tool-proto`, `ene-tool-rag`
- **禁止されている依存**: 他のすべてのクレートは `ene-runtime` に依存できません。循環参照を防ぐため、`ene-mind` や `ene-store` などの下位ドメインは `ene-runtime` をインポートしてはなりません。

### 論理的境界
`ene-runtime` は自身の中に「認知（感情モデル、長期記憶の選択など）」や「データベーステーブル」のロジックを持ちません。それらはすべて `ene-mind` および `ene-store` に移譲し、自身はプロバイダーやツールの「通信およびディスパッチチャネルの管理」に専念します。

---

## 2. モジュール構造

クレート内部は以下のモジュールに細分化されています。

```text
ene-runtime/src/
├── lib.rs              # クレートのルート。すべての主要APIの再エクスポート
├── bootstrap.rs        # ホスト起動用のヘルパー（Config/カードのロード）
├── db_server.rs        # ツール用の安全なDBアクセスIPCサーバー
├── diagnostics.rs      # メモリ・ツールの診断および照会用ファサード
├── error.rs            # クレート全体で使用される EneRuntimeError
├── handle.rs           # スレッドセーフな EneHandle および非同期 EneActor
├── message_builder.rs  # LLM用メッセージの構築および表情PHIの組み立て
├── proactive.rs        # 能動話話判断のバックグラウンドループ
├── streaming.rs        # 会話ストリーミングおよびツール割り込みループ
├── streaming_cognitive.rs # 認知ストリーミングパスの特殊実装
└── types.rs            # TurnId や RequestId などの各種共通ID・エラー定義
```

---

## 3. 起動用のヘルパー (Bootstrap)

`bootstrap.rs` モジュールは、ホスト起動時にファイルI/Oから設定やカードをロードし、準備完了状態のアクター（`EneHandle`）をインスタンス化するためのヘルパーを提供します。

### 関数一覧

#### `open_from_disk`
*   **シグネチャ**:
    ```rust
    pub async fn open_from_disk() -> Result<(EneHandle, EneConfig), EneRuntimeError>
    ```
*   **解説**: CLI起動などで使用されるフェイルハードな初期化関数です。`ConfigStore::try_load()` を呼び出して `config.json` をロードし、`load_character_card()` でキャラクターカードV3を読み込み、それらを元に `EneHandle::open()` を呼び出して準備完了したハンドルを返します。
*   **接続先**: `ene_config::ConfigStore`, `ene_config::load_character_card`, `EneHandle::open`

#### `open_with_config`
*   **シグネチャ**:
    ```rust
    pub async fn open_with_config(config: EneConfig) -> Result<EneHandle, EneRuntimeError>
    ```
*   **解説**: すでにメモリ上にロードされた `EneConfig` を使ってアクターを起動します。デスクトップ版のブートライフサイクルに適しています。
*   **接続先**: `ene_config::load_character_card`, `EneHandle::open`

#### `open_ready`
*   **シグネチャ**:
    ```rust
    pub async fn open_ready(
        config: EneConfig,
        card: CharacterCardV3,
    ) -> Result<EneHandle, EneRuntimeError>
    ```
*   **解説**: 設定とキャラクターカードの構造体実体を直接受け取り、一切のファイルI/Oを行わずに `EneHandle::open()` を呼び出します。テストやカスタム初期化に利用されます。
*   **接続先**: `EneHandle::open`
