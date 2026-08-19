# クイックスタート

このページは、チェックアウトからデーモンとクライアント起動までの最短経路です。
ビルド環境の詳細はリポジトリの `AGENTS.md` にあります。

## 1. 要件

- **Linux** だけが開発・CI の対象です。Windows は Linux からのクロスコンパイル、
  macOS は非対応です。
- **Rust ≥ 1.85**（edition 2024）。CI は stable です。
- ネイティブ依存: Vulkan、ALSA、OpenSSL、`libclang`、`mold`、Wayland/X11。
  checked-in の Nix flake が全部出します。

```sh
nix develop --command cargo build --workspace
```

`direnv` が有効なら、そのまま `cargo` で構いません。

## 2. ビルド

```sh
cargo build --workspace
cargo build -p ene-ctl
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

desktop は必要なら `ene-core` を子プロセスとして起動し、表層にキャラクターと
チャットを出し、詳細は別窓（F4 / トレイ）です。`ene-stage` は同一 API の
任意のデバッグクライアントです。

チャットに既定モデルはありません。最初のメッセージの前に **AI** ページで選びます。

- **このパソコン** — おすすめの Gemma GGUF（モデルディレクトリへ自動ダウンロード）
  か、自分の `.gguf` ファイル。ホストが `PATH` または同梱 plugins ディレクトリの
  `llama-server` を起動し、`ai.tasks.chat.plugin = provider.openai_compat` と
  `model_path` を書きます。
- **ChatGPT 系** — vault に API キーを入れた `provider.openai_compat`
  （Base URL は「詳しく」）。
- **Claude** — vault に API キーを入れた `provider.anthropic`。

分類・埋め込み・TTS・STT は設定するまでオフです。classifier が空なら
チャットのバインドにフォールバックします。

## 5. テスト

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

必ず `--workspace` か `-p <package>` を付けてください。default-members は
`ene-ctl` だけです。
