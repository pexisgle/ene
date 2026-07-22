# リリース

ene の Linux 向けリリース手順です。`v*` タグの push をトリガーに、バイナリのビルド・パッケージング・changelog 生成・GitHub Release 公開までが自動で行われます。

[← 開発者ガイド](index.md) · [English](../../guide/release.md)

## 概要

| 段階 | 内容 |
|------|------|
| タグ push（`v*`） | `.github/workflows/release.yml` が `ubuntu-latest` で実行 |
| ビルド | `nix develop --command cargo build --release` で `ene-cli`・`ene-desktop`・組み込みツール一式 |
| パッケージ | `scripts/package-linux-release.sh` が CLI 用 `.tar.gz` と Desktop 用 `.deb` を生成 |
| changelog | 前タグ以降の Conventional Commits を `git-cliff`（`cliff.toml`）が整形 |
| 公開 | `softprops/action-gh-release` が成果物をアップロードし、本文に changelog を設定 |

### リリース成果物（Linux x86_64）

| ファイル | 内容 |
|----------|------|
| `ene-cli-<version>-linux-x86_64.tar.gz` | `ene-cli` と `tools/` 配下の組み込みツール |
| `ene-desktop_<version>_amd64.deb` | `/usr/bin/ene-desktop`、ツールは `/usr/bin/tools/`、`.desktop` エントリ |

Windows / macOS 向けインストーラは現時点では対象外です（[#244](https://github.com/pexisgle/ene/issues/244)）。

## 前提

- リポジトリへのタグ push 権限があること。
- `main` のコミットが [Conventional Commits](https://www.conventionalcommits.org/) 形式であること（`feat:`、`fix:`、`docs:` など）。
- ルート `Cargo.toml` の `[workspace.package].version` がタグ（先頭の `v` を除く）と一致していること。

## リリース手順

1. **ワークスペースのバージョンを更新**（必要な場合）:

   ```toml
   # Cargo.toml
   [workspace.package]
   version = "0.2.0"
   ```

2. **リリース準備を `main` にマージ**（バージョン bump、最終修正、ドキュメント更新）。

3. **注釈付きタグを作成して push**:

   ```bash
   git tag -a v0.2.0 -m "ene v0.2.0"
   git push origin v0.2.0
   ```

4. **CI を確認** — [Release ワークフロー](https://github.com/pexisgle/ene/actions/workflows/release.yml) がビルド・パッケージング・GitHub Release 作成まで実行します。

5. **Release ページを確認** — changelog、添付の `.tar.gz` / `.deb`、実機でのインストール動作。

## リリースビルドのインストール

### CLI（tarball）

```bash
tar -xzf ene-cli-0.2.0-linux-x86_64.tar.gz
cd ene-cli-0.2.0-linux-x86_64
./ene-cli --help
```

`tools/` は `ene-cli` と同じディレクトリに置いてください（リリースビルドは `<exe_dir>/tools/` を参照します）。

### Desktop（.deb）

```bash
sudo dpkg -i ene-desktop_0.2.0_amd64.deb
# 依存関係エラーが出た場合:
sudo apt-get install -f
ene-desktop
```

パッケージには GTK / Wayland / PipeWire などの一般的な依存を宣言しています。最小構成の環境では GPU や portal 関連ライブラリが別途必要な場合があります（[Desktop アプリ](apps/desktop.md) を参照）。

## ローカルでのドライラン

公開せずにパッケージだけ試す場合:

```bash
nix develop --command cargo build --release \
  -p ene-cli -p ene-desktop \
  -p ene-tool-fs -p ene-tool-web -p ene-tool-utility \
  -p ene-tool-app -p ene-tool-browser

bash scripts/package-linux-release.sh 0.2.0

nix develop --command git cliff --latest --strip header
```

成果物は `dist/` に出力されます。

## changelog 設定

- 設定ファイル: リポジトリ直下の `cliff.toml`
- `git-cliff` は Nix 開発シェルに含まれます（`flake.nix`）
- Release 本文には最新タグ分のみ（`git cliff --latest --strip header`）
- `chore(release):` コミットは本文から除外

全文プレビュー:

```bash
nix develop --command git cliff --config cliff.toml
```

## クレートメタデータ

ワークスペース内のクレートはすべて `publish = false` とし、`license`・`repository`・`version` は `[workspace.package]` から継承します。アプリ（`ene-cli`、`ene-desktop`）は `version.workspace = true` でワークスペース版を参照します。

## トラブルシュート

| 症状 | 想定原因 |
|------|----------|
| changelog が空 / 薄い | Conventional Commits ではない、または workflow の checkout で `fetch-depth: 0` が外れている |
| CI で `dpkg-deb` 失敗 | ローカルで `bash scripts/package-linux-release.sh <ver>` を再現 |
| Desktop は起動するがツールがない | tarball では `tools/` を exe 横に、`.deb` では `/usr/bin/tools/` に配置する必要がある |
| workflow が動かない | タグは `v*` 形式（例: `v0.1.0`。`0.1.0` だけでは不可） |

## 関連

- [はじめに](getting-started.md) — 日常の開発ビルド
- [CI](../../.github/workflows/ci.yml) — `main` 向け format / clippy / test
- Issue [#244](https://github.com/pexisgle/ene/issues/244) — リリース自動化の追跡
