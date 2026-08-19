# クイックスタート

このページは、チェックアウトからデーモンとクライアント起動までの最短経路です。
ビルド環境の詳細はリポジトリの `AGENTS.md` にあります。

## 1. 要件

- **Linux とネイティブ Windows** を開発対象にします。macOS は非対応です。
- **Rust ≥ 1.85**（edition 2024）。stable ツールチェーンを使います。
- Linux のネイティブ依存は Vulkan、ALSA、OpenSSL、`libclang`、`mold`、
  Wayland/X11 です。checked-in の Nix flake が提供します。
- Windows では stable MSVC Rust ツールチェーン、Visual Studio 2022 Build Tools の
  **Desktop development with C++**、Windows 10/11 SDK を入れます。デスクトップは
  Windows では DX12 と WASAPI を使います。

```sh
nix develop --command cargo build --workspace
```

`direnv` が有効なら、そのまま `cargo` で構いません。

## 2. ビルド

Linux ではワークスペースのコマンドをそのまま使えます。

```sh
cargo build --workspace
cargo build -p ene-ctl
```

ネイティブ Windows でデスクトップを開発するときは、`target/debug` の
`ene-core` をデスクトップから起動できるよう、両方をビルドします。

```powershell
cargo build -p ene-daemon -p ene-desktop
```

## 3. CLI を動かす

```sh
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- --help
```

`ene-ctl` は desktop / stage / Web と同じ HTTP/WS API を使います。自分で
`ene-core` を起動した場合は `--url` と `--token`（または `ENE_API_URL` /
`ENE_API_TOKEN`）を合わせてください。

## 4. desktop を動かす

```sh
cargo run -p ene-desktop
```

ネイティブ Windows では、上記の両方のビルドを PowerShell で実行してから
同じコマンドを使います。デーモンを `target/debug` 外に置く場合は
`ENE_CORE_BIN` を設定してください。

desktop は必要なら `ene-core` を子プロセスとして起動し、表層にキャラクターと
チャットを出し、詳細は別窓（F4 / トレイ）です。`ene-stage` は同一 API の
任意のデバッグクライアントです。

チャットに既定モデルはありません。最初のメッセージの前に **AI** ページで選びます。
ページはホストカタログの **インストール済みプロバイダプラグイン**（`seam.llm`）
を出します。`provider.gguf`（このパソコン、ローカル GGUF）、OpenAI 互換、
Anthropic、足したプラグインが並びます。ローカル側はおすすめ Gemma の
ダウンロードか自分の `.gguf`。クラウドはモデル名と vault の API キーです。

埋め込みは任意です。未設定、ローカル GGUF（`provider.gguf`、おすすめ Jina）、
またはクラウドの埋め込みプラグイン。分類・能動発話が未指定なら会話モデルの
値を継承します。

## 5. テスト

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

必ず `--workspace` か `-p <package>` を付けてください。default-members は
`ene-ctl` だけです。
