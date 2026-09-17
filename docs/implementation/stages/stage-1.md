# Stage 1: 最小限の共通規約と基盤の作成

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 1 の**実装順と完了条件**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。

## 目的

Stage 2 の最初の縦断スライスが、共通型や設定の作り直しをせず開始できる最小限の基盤を用意します。

## 実装順

1. `ene-primitive` に、複数 subsystem が本当に共有する基本型だけを置く。
2. `ene-config` に、起動に必要な最小設定の読込境界を置く。
3. `ene-api` に、Host / Client 間で Stage 2 が必要とする最小 wire contract を定義する。
4. Host と Client の app entrypoint を作り、共通基盤だけに依存した状態で起動できるようにする。
5. Stage 2 の接続・会話スライスを実装できる依存方向になっているか確認する。

将来必要そうという理由だけで共通 manager、巨大 coordinator、汎用 repository abstraction を先行して作りません。必要な境界が初めて現れた Stage で追加します。

## 完了条件

- Stage 2 が primitive / config / API contract を再定義せず開始できる。
- ドメイン crate から具象 store や app composition への逆依存を作っていない。
- 公開 API は、その時点で実際に利用する最小範囲に留まっている。

## 非対象

会話、推論、履歴保存、記憶、Task などの製品動作は Stage 2 以降で縦断的に実装します。
