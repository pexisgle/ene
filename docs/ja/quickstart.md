# クイックスタート

このページは、チェックアウトからコアとクライアント起動までの最短経路です。
ビルド環境の詳細はリポジトリの `AGENTS.md` にあります。

## 1. 要件

- **Linux とネイティブ Windows** を開発対象にします。macOS は非対応です。
- **Rust 1.98.0**（edition 2024。バージョンは `rust-toolchain.toml` に固定）。
- Linux のネイティブ依存は Vulkan、ALSA、OpenSSL、`libclang`、`mold`、
  fontconfig、Wayland/X11 です。checked-in の Nix flake が提供します。
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
`ene-core` を stage から起動できるよう、両方をビルドします。

```powershell
cargo build -p ene-core -p ene-stage
```

## 3. CLI を動かす

```sh
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- --help
```

`ene-ctl` は stage / Web と同じ HTTP/WS API を使います。自分で
`ene-core` を起動した場合は `--url` と `--token`（または `ENE_API_URL` /
`ENE_API_TOKEN`）を合わせてください。

## 4. 製品 GUI（stage）を動かす

```sh
cargo run -p ene-stage
```

ネイティブ Windows では、上記の両方のビルドを PowerShell で実行してから
同じコマンドを使います。コアを `target/debug` 外に置く場合は
`ENE_CORE_BIN` を設定してください。

stage は必要なら `ene-core` を子プロセスとして起動し、表層にキャラクターと
チャットを出し、詳細は別窓（トレイ）で設定・記憶・キャラ・ジョブ・
内部ログを扱います。

チャットに既定モデルはありません。最初のメッセージの前に詳細の **Conversation**
タブで選びます。ホストカタログの **インストール済みプロバイダプラグイン**
（`seam.llm`）を束縛します。`provider.gguf`（このパソコン、ローカル GGUF）、
OpenAI 互換、Anthropic、足したプラグインが使えます。ローカル側はおすすめ
Gemma のダウンロードか自分の `.gguf`。クラウドはモデル名と vault の API キーです。

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

リポジトリの `assets/settings.json` は開発用サンプルです。`ene-core` が読む
のはデータディレクトリの `settings.json` で、省略したキーは既定値になります。
