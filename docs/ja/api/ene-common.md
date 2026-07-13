# `ene-common` — APIリファレンス

> **クレート:** `ene-common`
> **役割:** Eneワークスペース全体で共有されるユーティリティ型。

---

## 概要

`ene-common` は複数のワークスペースメンバーで共有されるコードを保持する基盤ユーティリティクレートです。重い推移的依存関係を避けるため、意図的に最小限に保たれています。

このクレートの唯一のエクスポートは `Truncate` です。これは、文字数、行/バイト予算、あるいはコンテンツの先頭または末尾を基準にテキストを安全に短縮するための、一連の静的（`Self` を取らない）ヘルパー関数を提供するユニット構造体です。`Truncate` は**トレイトではありません**。実装すべきものは何もなく、`&str` や `String` に対して直接呼び出せるメソッドもありません。すべての操作は `Truncate::method(...)` という形で呼び出します。

---

## 型

### `Truncate`

```rust
/// 高性能かつ詳細な文字列トランケーションのためのヘルパー構造体。
pub struct Truncate;
```

サイズゼロのマーカー構造体です。以下のトランケーションヘルパー関数の名前空間として存在するだけであり、インスタンス化されることはありません。

### `TruncateResult`

```rust
#[derive(Debug, Clone)]
pub struct TruncateResult {
    /// 切り詰められた（または元のままの）テキスト内容。
    pub content: String,
    /// 実際に切り詰めが行われたかどうか。
    pub truncated: bool,
}
```

[`Truncate::output`](#truncateoutput) と [`Truncate::tail`](#truncatetail) の戻り値です。結果のテキストと、入力が実際に切り詰めを必要としたかどうかを示すフラグを保持します。

---

## メソッドテーブル

すべてのメソッドは `Truncate` に対する `Self` を取らない固有関数です（トレイト経由ではなく `Truncate::simple(...)` のように呼び出します）。

| メソッド | シグネチャ | 戻り値 | 説明 |
|---|---|---|---|
| `simple` | `fn simple(text: &str, max_chars: usize) -> String` | `String` | `max_chars` Unicode文字に切り詰め、切り詰めが発生した場合は `"..."` を付加する。 |
| `detailed` | `fn detailed(text: &str, max_chars: usize) -> String` | `String` | `max_chars` Unicode文字に切り詰め、切り詰めが発生した場合は元の文字数を含む詳細な注記を付加する。 |
| `chars` | `fn chars(text: &str, max_chars: usize) -> String` | `String` | [`detailed`](#truncatedetailed) のエイリアス。ツールから利用される。 |
| `output` | `fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult` | `TruncateResult` | `text` を行数・バイト数の両方の予算に収め、**先頭**からコンテンツを保持する。 |
| `tail` | `fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult` | `TruncateResult` | `text` を行数・バイト数の両方の予算に収め、**末尾**からコンテンツを保持する。 |

これらのメソッドはいずれも不正なUTF-8入力に対してパニックすることはありません。文字境界の計算はすべて `char_indices()` の境界にスナップされ、行/バイト分割は `str::lines()`／元の文字列の有効な境界内に収まるバイトスライス上で行われます。

---

## メソッドリファレンス

### `Truncate::simple`

```rust
pub fn simple(text: &str, max_chars: usize) -> String
```

`text` を最大 `max_chars` Unicodeスカラー値に切り詰めます。テキストがそれより長い場合、超過分は削られ `"..."` が付加されます。すでに収まっている場合はそのまま返されます。

| 入力 | `max_chars` | 結果 |
|---|---|---|
| `"hello"` | `10` | `"hello"` |
| `"hello world"` | `5` | `"hello..."` |

### `Truncate::detailed`

```rust
pub fn detailed(text: &str, max_chars: usize) -> String
```

[`simple`](#truncatesimple) と同様ですが、切り詰めが発生した場合は単なる `"..."` の代わりに、元の文字数を含む説明的な通知を付加します。

```text
{先頭部分}

[... truncated, total {char_count} chars ...]
```

| 入力 | `max_chars` | 結果（切り詰めが発生する場合） |
|---|---|---|
| `"hello"` | `10` | `"hello"`（変更なし） |
| `"hello world"` | `5` | `"hello\n\n[... truncated, total 11 chars ...]"` |

### `Truncate::chars`

```rust
pub fn chars(text: &str, max_chars: usize) -> String
```

[`Truncate::detailed`](#truncatedetailed) への直接のエイリアスです。より簡潔で説明的な名前を好む呼び出し元（主にツール実装）向けに、別名として保持されています。

### `Truncate::output`

```rust
pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult
```

`text` を最大行数・最大バイト数の**両方**の予算に収め、テキストの先頭（head）から行を保持します。`text` が両方の予算をすでに満たしている場合、`TruncateResult { content: text.to_string(), truncated: false }` が変更なしで返されます。

そうでない場合、いずれかの予算を超えるまで先頭から行が積み上げられ、結果には以下の注記が付加されます。

```text
{保持された行}

...{removed} {"bytes" | "lines"} truncated...

Use offset/limit or grep to view specific sections.
```

コンテンツが実際に切り詰められた場合、`truncated` フィールドは `true` になります。最も重要な情報が先頭付近にあることが多い、コマンド／ツールの標準出力を切り詰める際の典型的な選択肢です。

### `Truncate::tail`

```rust
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult
```

[`output`](#truncateoutput) と同じ予算ロジックですが、先頭ではなくテキストの**末尾**から行を保持します。注記は末尾に付加されるのではなく先頭に付加されます。

```text
...{removed} {"bytes" | "lines"} truncated...

{保持された行}
```

最新の行が最も重要となる、ログや長時間実行されるコマンド出力の切り詰めに便利です。

---

## エラー

`ene-common` にはエラー型は存在しません。すべての `Truncate` メソッドは `&str` 入力に対する全域関数（total function）であり、パニックすることも失敗することもなく、常に値を返します（切り詰めが不要な場合は元のテキストをそのまま返すだけです）。

---

## 使用例

### 省略記号によるシンプルな切り詰め

```rust,no_run
use ene_common::Truncate;

let text = "This is a long piece of text that we want to shorten.";
let short = Truncate::simple(text, 10);
assert_eq!(short, "This is a ...");
```

### 文字数付きの詳細な切り詰め

```rust,no_run
use ene_common::Truncate;

let text = "This is a long piece of text that we want to shorten.";
let detailed = Truncate::detailed(text, 10);
assert!(detailed.contains("truncated, total"));
```

### ツール出力を行/バイト予算に収める

```rust,no_run
use ene_common::Truncate;

let stdout = run_some_command();
let result = Truncate::output(&stdout, /* max_lines */ 200, /* max_bytes */ 16_000);

if result.truncated {
    eprintln!("警告: コマンド出力が切り詰められました");
}
send_to_model(&result.content);

fn run_some_command() -> String {
    unimplemented!()
}
fn send_to_model(_: &str) {}
```

### 長いログの末尾を保持する

```rust,no_run
use ene_common::Truncate;

let log = read_log_file();
let result = Truncate::tail(&log, /* max_lines */ 100, /* max_bytes */ 8_000);

println!("{}", result.content);

fn read_log_file() -> String {
    unimplemented!()
}
```

---

## 他のクレートからの再エクスポート

`ene-common::Truncate`（および `TruncateResult`）は、`ene-common` に直接依存する必要がないよう、複数のコンシューマークレートから再エクスポートされています。

```rust,no_run
// crates/ene-runtime/src/lib.rs
#[doc(no_inline)]
pub use ene_common::Truncate;

// crates/ene-mind/src/lib.rs
pub use ene_common::truncate::Truncate;

// crates/ene-tool-common/src/lib.rs
pub mod truncate {
    #[doc(no_inline)]
    pub use ene_common::truncate::{Truncate, TruncateResult};
}
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

- [`ene-tool-common`](./ene-tool-common.md) — ツール実装向けに `Truncate`/`TruncateResult` を再エクスポートしている
- [`ene-mind`](./ene-mind.md) — `Truncate` を再エクスポートしている
- [`ene-runtime`](./ene-runtime.md) — ワークスペースのエントリーポイント
