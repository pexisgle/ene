# ene

ene は、ローカルでキャラクター（VRM / VRMA）を扱い、AIを使った会話やアプリ・CLI を提供する Rust ワークスペースです。

## 概要
- **ワークスペース構成**: このリポジトリは複数クレートを含む Cargo ワークスペースです。
- **主なクレート**:
- `ene-core`: LLM対話、セッション管理、長期記憶、ツール実行基盤を統合したコアライブラリ。
- `ene-desktop`: デスクトップ GUI アプリ（Bevy ベース）。
- `ene-cli`: CLI クライアント（ヘッドレスやスクリプト用途）。

## リポジトリ構造（抜粋）
- `crates/ene-core/` — コアライブラリ
- `apps/ene-desktop/` — GUI アプリケーション
- `apps/ene-cli/` — コマンドラインインターフェース
- `assets/` — サンプルキャラクターやアセット（`characters/`、`vrm/`、`vrma/` 等）

## 前提条件
- Rust（ツールチェーンは `rust-toolchain.toml` を参照）。
- ネイティブ依存（GUI をビルドする場合は GTK や Wayland/Windows 開発ヘッダ等が必要になることがあります）。

## ビルド
ワークスペース全体をビルドするには:

```bash
cargo build --workspace
```

リリースビルド:

```bash
cargo build --workspace --release
```

## 実行
GUI アプリを実行するには:

```bash
cargo run -p ene-desktop --release
```

CLI を実行するには（オプションは `--help` で確認してください）:

```bash
cargo run -p ene-cli -- --help
cargo run -p ene-cli -- <args>
```

## テスト
ワークスペースのテストを実行するには:

```bash
cargo test --workspace
```

## 開発メモ
- `crates/ene-core` は `async-openai` 等を使い非同期でモデルと通信します。
- GUI は `bevy` と `bevy_vrm1`、`bevy_egui` 等を利用しています。

## 資産（assets）
プロジェクトには `assets/characters`、`assets/vrm`、`assets/vrma` にサンプルファイルが含まれます。カスタムキャラクターを追加する場合はこれらのフォルダを参照してください。

## 新機能追加計画 (Cowork Agent 機能)

現在、ene は単なる対話エージェントを超え、Claude Cowork のような**自律的デスクトップエージェント**（Cowork Agent）へと進化するための開発を進めています。
OpenCode のアーキテクチャを参考に、AI キャラクターがユーザーに代わって様々なタスクを自動実行できる基盤を構築します。

### 実装予定のツールグループ

以下の5つのカテゴリのツールを AI に提供し、サンドボックス環境内で安全に実行できるようにします。

1. **ファイルシステムツール** (`filesystem_tools`)
   - ファイルの読み書き、検索、編集（行番号指定、Levenshtein 類似度を用いた柔軟な置換）
   - ディレクトリの作成、移動、削除
   - オープンソースコーディングエージェント OpenCode の設計を踏襲した安全なファイル操作
2. **シェル実行ツール** (`shell_tools`)
   - シェルコマンドの実行と結果の取得
   - バックグラウンドプロセスの起動と状態監視
   - コマンド解析に基づく詳細なパーミッション制御（AST ではなく正規表現を利用した簡易解析）
3. **ブラウザ操作ツール** (`browser_tools`)
   - Chromium と Chrome DevTools Protocol (CDP) (`chromiumoxide` クレート等) を用いたブラウザ自動化
   - ページ遷移、クリック、テキスト入力、要素の待機
   - スクリーンショット取得（Vision API との連携）と DOM コンテンツの抽出
4. **アプリ操作ツール** (`app_tools`)
   - `enigo` や `xcap` を活用した OS レベルの GUI 操作
   - ウィンドウの列挙とフォーカス
   - キーボードの打鍵シミュレーション、マウスカーソルの移動とクリック
   - クリップボードの読み書き
5. **Web検索ツール** (`websearch_tools`)
   - 最新情報や技術リファレンスを検索・取得するための検索 API 統合（例: DuckDuckGo API、Tavily、Brave Search API 等の統合）
   - 検索結果の要約と、必要に応じた特定ページへの遷移トリガー

### アーキテクチャの拡張方針

これらの強力な機能を実現するため、コアライブラリ (`ene-core`) に以下の基盤を追加します。

- **高度なサンドボックスとパーミッション管理** (`sandbox.rs`)
  - 許可ディレクトリ (`allowed_directories`) と禁止コマンド (`blocked_commands`) の設定
  - 破壊的操作（削除、上書きなど）の実行前にユーザーの承認を求める `PermissionGate` システム
- **自律タスク実行エンジン（タスクプランナー）**
  - ユーザーの高レベルな指示からステップごとの計画を立案
  - `Plan` -> `Execute` -> `Verify` のエージェンティック・ループによる自律進行
- **バックグラウンド実行モード**
  - ユーザー入力を待たずに裏でタスクを進行し、CLI や GUI で進捗 (`TaskProgress`) を表示
- **非同期イベントストリームの拡張**
  - 既存の `AiStreamEvent` にパーミッション要求 (`PermissionRequired`) やタスク進捗を追加し、UI とシームレスに連携

### 開発フェーズ

1. **Phase 1: 基盤構築** - サンドボックス、ファイル操作、シェル実行ツールの実装
2. **Phase 2: 自律実行** - タスクプランナー、バックグラウンドモード、ユーザー確認 UI、Web検索ツールの実装
3. **Phase 3: GUI 自動化** - ブラウザ操作、アプリ操作ツールの実装
4. **Phase 4: 拡張** - Skills（ワークフロー保存機能）や複数ファイル横断分析などの高度な機能
