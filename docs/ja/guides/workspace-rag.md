# ワークスペース RAG

キャラクターは**あなたのプロジェクトファイル**について質問に答えられます。
ワークスペース RAG はディレクトリのドキュメントを埋め込み付きチャンクに
インデックスし、必要なターンで関連チャンクをプロンプト
（`WorkspaceContext` セクション）に注入します。

## 設定

```json
{
  "rag": {
    "workspace": {
      "enabled": true,
      "root": "/home/me/projects/myapp",
      "include_extensions": ["md", "rs", "toml", "py", "ts", "js"],
      "ignore_globs": [".git/**", "node_modules/**", "target/**"],
      "max_file_bytes": 1048576,
      "chunk_chars": 1200,
      "chunk_overlap_chars": 200,
      "max_chunks_per_file": 256,
      "top_k": 8
    }
  }
}
```

デフォルト: 一般的なテキスト/コード拡張子を含み、`.git`・`node_modules`・
`target`・モデル重み・DB ファイルを無視。ファイル上限 1 MiB・1200 文字
チャンク・200 文字オーバーラップ。

## インデックス化

```sh
# REPL で:
/workspace sync
/workspace status
/workspace search "認証フロー"
/workspace cancel
```

インデクサーは設定されたルートを走査し、ignore glob を適用し、各ドキュメント
をチャンク化して、チャンク+埋め込みをメモリデータベースに保存します
（`workspace_document_files` / `workspace_document_chunks` テーブル）。
インデックス状態は永続化され、再同期では変更されたファイルだけを再
チャンク化します。

## 検索

ターン時に、関連チャンクがスコアリングされ（埋め込み類似度 + 語彙一致）、
重複排除されてソース引用付きでプロンプトに置かれます。`/workspace search`
はターンなしで同じ検索を表示するため、チャンクが浮上しない理由のデバッグに
使えます。

## 注意

- ワークスペースインデックスには埋め込みプロバイダー
  （`ai.tasks.embedding`）が必要です。
- ファイルはホストプロセスが読みます。インデックスルートはユーザーが選ぶ
  ディレクトリであり、サンドボックス化されたプラグインパスではありません。
