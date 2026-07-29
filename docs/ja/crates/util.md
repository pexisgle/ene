# ene-util

> **クレート**: `ene-util`

## 役割

`ene-util` は、依存関係が互いに独立した小さな**純粋**ユーティリティ関数の置き場所です。
各ヘルパーは Cargo feature の背後に配置され、利用者は必要な分だけコストを支払います。

現在提供されているもの:

- `truncate`（デフォルト）— 賢い文字列切り詰めヘルパー（`Truncate`）。
- `html` — HTML から Markdown への変換とコンテンツ抽出
  （`htmd`、`scraper`、`ego-tree`、`regex` を導入）。

## 境界

- **純粋関数のみ**: I/O なし、ビジネスロジックなし、可変グローバル状態なし。
- **feature で隔離された重い依存**: `html` feature の scraper/htmd 一式は隔離されているため、
  truncate のみを利用する側（`ene-mind`、`ene-cli`、`ene-desktop`）はそれらをコンパイルしません。
- ヘルパーにデータベースアクセス、ネットワーク呼び出し、ドメイン知識が必要な場合は、
  ここではなく適切なドメインクレートに配置します。

この規律により、`ene-util` が旧 `ene-common` のような「なんでも吸着するクレート」に
なることを防ぎます（`ene-common` は内容が再分配される前にその状態に陥りました）。

## 探索

```bash
cargo doc -p ene-util --open
```
