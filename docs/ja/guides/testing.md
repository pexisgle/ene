# テスト

Ene のテストスイートは古典的なテストピラミッドに従っています。つまり
高速なユニットテストを大量に、統合テストをしっかりした層として持ち、
実行環境が整わない場合は自動でスキップされる少数の重いエンドツーエンド
（sidecar 契約）テストを最上位に置きます。

## テストの層

| 層 | 場所 | 対象 | 数 |
|---|---|---|---|
| ユニット | コードの隣にある `#[cfg(test)]` モジュール | 純粋ロジック・アルゴリズム・設定・整形 | 約 3,400 |
| 統合 | 各クレートの `tests/` ディレクトリ | 公開 API・IPC 契約・実ハンドル経由の DB フロー | 約 240 |
| Sidecar E2E | `plugins/provider/llama-server/tests`、`plugins/provider/local-llm/tests` | ピン留めされた GGUF fixture を使った実プラグインバイナリの IPC | 少数 |

ユニットテストは可能な限りネットワーク・ファイルシステム・時計に依存
させません。壁時計が避けられない箇所（永続スケジューラ）では、仮想
クロックを注入してスケジューラを明示的に起こす方式を取ります。

## テストの実行

NixOS では Cargo がホストの PATH にないため、リポジトリルートで direnv
ラッパーを使います:

```sh
rtk direnv exec . rtk cargo test --workspace        # すべて
rtk direnv exec . rtk cargo test -p ene-runtime     # 1 クレート
rtk direnv exec . rtk cargo test -p ene-runtime --test scheduler
rtk direnv exec . rtk cargo clippy --workspace --all-targets -- -D warnings
```

裸の `cargo test` は `ene-cli`（デフォルトメンバー）しか対象になりません。
常に `--workspace` か `-p <package>` を明示してください。CI はワークスペース
を 3 つのテストジョブ（コアクレート・アプリ・プラグイン）に分割している
ため、失敗・フレークしたパッケージだけを個別に再実行できます。

## テストの方針

- **スナップショット（insta）**: 安定した出力契約（セッションエクスポート
  JSON と合成プロンプトパケット）は `src/snapshots/` 配下のスナップショット
  で固定しています。文言や形式を変えるとテストが失敗するため、スナップ
  ショットをレビュー（`cargo insta review`）してからコミットします。
- **プロパティテスト（proptest）**: 秘密情報の除去（`ene-connector`）と
  テキスト切り詰め（`ene-util`）では、事例テストに加えてランダム入力を
  使ったプロパティテストを実施しています。新しい純粋ロジックのクレートも
  同様の導入を検討してください。
- **Sidecar fixture**: llama.cpp の契約テストはピン留めされた GGUF fixture
  を blake3 検証付きキャッシュにダウンロードします。ネットワークや sidecar
  バイナリが使えない場合はテストをスキップし、ピンのハッシュがずれた場合
  は明示的に失敗します。
- **フレークするテスト**: ジョブを一度再実行してください。再現する場合は
  タイミング依存を決定的に修正します（仮想クロック・制御された解放・上限
  付きポーリング）。タイムアウトを延ばすのは最後の手段であり、修正では
  ありません。
