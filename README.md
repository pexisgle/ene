# ene

ene は、一人の Owner が管理する Host 上で継続する、Companion 中心のパーソナル AI システムである。会話と実作業を同じ個体から使い、永続状態の正本は Host に置く。

## 正本

- 製品と要件: [`docs/requirements/`](docs/requirements/README.md)
- 設計: [`docs/design/`](docs/design/README.md)
- 実装の進め方: [`docs/implementation/README.md`](docs/implementation/README.md)
- 実装の現在地: [`docs/implementation/PROGRESS.md`](docs/implementation/PROGRESS.md)
- 実装規約: [`AGENTS.md`](AGENTS.md)

製品条件は requirements、内部設計は design、実装の順序と完了条件は実装ガイド、いまの Stage は PROGRESS.md だけが持つ。コマンドと Rust 規約は AGENTS.md を見る。

設計を目的から探すときは [設計文書](docs/design/README.md) の目的別索引を使う。工程順に全文書を読むことは通常の前提にしない。

`.old/` は退役実装であり、要件・設計・実装契約ではない。
