# `ene-common` — APIリファレンス

> **クレート:** `ene-common`  
> **役割:** Eneワークスペース全体で共有されるユーティリティ型とトレイト。

---

## 概要

`ene-common` は複数のワークスペースメンバーで共有されるコードを保持する基盤ユーティリティクレートです。重い推移的依存関係を避けるため、意図的に最小限に保たれています。

現在の主要なエクスポートは、さまざまな単位で文字列長を安全に制限するための `Truncate` トレイトです。

---

## `Truncate` トレイト

文字列型に対して3種類のトランケーション戦略を提供します。いずれも（切り詰められた可能性のある）文字列と、実際に切り詰めが発生したかどうかを示すフラグを持つ `TruncateResult` を返します。

このトレイトはブランケット実装によって `str` と `String` の両方に実装されています。

```rust
pub trait Truncate {
    /// Unicodeスカラー値（文字）を最大 `n` 個に切り詰める。
    fn truncate_chars(&self, n: usize) -> TruncateResult;

    /// 最大 `n` 行に切り詰める。
    fn truncate_lines(&self, n: usize) -> TruncateResult;

    /// 最大 `n` バイトに切り詰める。
    fn truncate_bytes(&self, n: usize) -> TruncateResult;
}
```

### `TruncateResult`

```rust
pub struct TruncateResult {
    /// （切り詰められた可能性のある）文字列の内容。
    pub truncated: String,

    /// 入力が `n` より長く、実際に切り詰められた場合は `true`。
    pub was_truncated: bool,
}
```

---

## メソッドリファレンス

### `truncate_chars`

```rust
fn truncate_chars(&self, n: usize) -> TruncateResult
```

`n` 番目のUnicodeスカラー値の境界で分割します。マルチバイトUTF-8テキストに対して安全で、無効なUTF-8を生成しません。

| 入力 | `n` | `truncated` | `was_truncated` |
|------|-----|-------------|-----------------|
| `"Hello, world!"` | `5` | `"Hello"` | `true` |
| `"Hi"` | `10` | `"Hi"` | `false` |
| `"日本語テキスト"` | `3` | `"日本語"` | `true` |

### `truncate_lines`

```rust
fn truncate_lines(&self, n: usize) -> TruncateResult
```

最大 `n` 行を保持します（`\n` で分割）。最後に保持された行の末尾の改行は保持されます。

| 入力 | `n` | `truncated` | `was_truncated` |
|------|-----|-------------|-----------------|
| `"a\nb\nc\nd"` | `2` | `"a\nb"` | `true` |
| `"one line only"` | `5` | `"one line only"` | `false` |

### `truncate_bytes`

```rust
fn truncate_bytes(&self, n: usize) -> TruncateResult
```

最大 `n` バイトに切り詰め、最も近い有効なUTF-8文字境界まで戻すことで無効な出力を回避します。

| 入力 | `n` | `truncated` | `was_truncated` |
|------|-----|-------------|-----------------|
| `"Hello"` | `3` | `"Hel"` | `true` |
| `"Hi"` | `100` | `"Hi"` | `false` |

---

## 使用例

### 基本的な使い方

```rust
use ene_common::truncate::Truncate;

let text = "これは切り詰めたい長いテキストです。";

let result = text.truncate_chars(5);
println!("{}", result.truncated);       // "これは切り詰"
println!("{}", result.was_truncated);   // true

let short = "Hello".truncate_chars(100);
assert!(!short.was_truncated);
assert_eq!(short.truncated, "Hello");
```

### 複数行出力の切り詰め

```rust
use ene_common::truncate::Truncate;

let output = "1行目\n2行目\n3行目\n4行目\n5行目";
let result = output.truncate_lines(3);

assert_eq!(result.truncated, "1行目\n2行目\n3行目");
assert!(result.was_truncated);
```

### バッファ制限のための安全なバイトトランケーション

```rust
use ene_common::truncate::Truncate;

// バイト長制限のあるAPIを呼び出す際に便利
let user_input = get_user_input();
let safe_input = user_input.truncate_bytes(4096);

if safe_input.was_truncated {
    eprintln!("警告: 入力が4096バイトに切り詰められました");
}
send_to_api(&safe_input.truncated);
```

---

## 他のクレートからの再エクスポート

`ene-session` は利便性のために `Truncate` を再エクスポートしているため、`ene-session` の利用者は `ene-common` に直接依存する必要はありません：

```rust
// ene-session 内：
pub use ene_common::truncate::Truncate;
```

---

## 新しいユーティリティの追加

`ene-common` に新しいユーティリティを追加する際のガイドライン：

1. 依存関係は最小限に保つ — `ene-common` は重い推移的依存関係のない軽量クレートであり続ける必要があります。
2. 新しいモジュールを `lib.rs` に `pub mod` で追加する。
3. 必要に応じてコンシューマークレートから再エクスポートする。
4. 本ドキュメント（英語版 `docs/api/ene-common.md` および本ファイル）にドキュメントを追加する。

---

## 関連項目

- [`ene-session`](./ene-session.md) — `Truncate` を再エクスポートしている
- [`ene-core`](./ene-core.md) — ワークスペースのエントリーポイント
