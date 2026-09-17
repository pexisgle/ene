# Stage 0: リポジトリとビルド基盤の構築

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 0 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

後続 Stage が同じ手順で安全に開発できる、最小限の Rust workspace と CI 基盤を作ります。

## 実装順

1. ルート Cargo workspace と最小 crate / app の配置を確定する。
2. Rust toolchain、formatter、Clippy の共通設定を固定する。
3. Linux / Windows の最小 CI を用意し、build・test・lint を同じ repository policy で実行する。
4. 空または最小構成の workspace でローカルと CI の結果が一致することを確認する。

正確な開発コマンドと lint policy は [`AGENTS.md`](../../../AGENTS.md) を source of truth とし、この文書へ重複させません。

## 完了条件

- workspace 全体が Linux / Windows の両方で build・test・lint を通る。
- 後続 Stage が個別の toolchain / formatter / CI 手順を追加せずに実装を開始できる。

## 非対象

製品機能、ドメイン API、永続化スキーマなど、Stage 1 以降が所有する実装は先行して作りません。
