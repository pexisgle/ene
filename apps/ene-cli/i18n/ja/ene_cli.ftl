welcome = ene インタラクティブ CLI
help-hint = コマンドのリストを表示するには '/help' と入力してください。

## REPL / 実行エラー
busy-warning = [ビジー] すでにターンが進行中です。完了を待ってください。
run-failed = [実行] 失敗しました: { $error }
stream-lag-resync = [ストリーム] { $skipped } 件のイベントを取りこぼしました（遅延）。再同期のため進行中のターンをキャンセルします。
unknown-command = 不明なコマンドです: { $command }

## 権限プロンプト
permission-prompt = 操作の権限を選択してください
permission-allow-once = 1回のみ許可
permission-allow-session = このセッションで常に許可
permission-deny = 拒否

## ユーザー入力プロンプト
user-input-select = 回答を選択 (上下キーで選択, Enterで確定)
user-input-freetext = 自由入力 (空でskip, 'cancel'で全キャンセル)
user-input-skip = (スキップ)
user-input-cancel = (全キャンセル)

## /help コマンド
help-commands-title = コマンド一覧:
help-quit = CLI を終了する

## /greeting コマンド
greeting-no-card = キャラクターカードが読み込まれていません。
greeting-no-greetings = このキャラクターには挨拶がありません。
greeting-history-not-empty = 挨拶は最初のメッセージの前でのみ選択できます。新しいセッションを始めるには REPL を再起動してください。
greeting-none-selected = 挨拶を選択しませんでした。
greeting-selected = 選択した挨拶:
greeting-failed = 挨拶を設定できませんでした: { $error }
greeting-choose = 挨拶を選択 (Enter で確定)
greeting-none = (なし)

## /card コマンド
card-loaded = キャラクターカードを読み込みました: { $name }

init-failed = 初期化に失敗しました: { $error }
turn-failed = エラー: { $detail }
runtime-error-no-character-card = キャラクターカードが見つからないか読み込めませんでした。
runtime-error-channel-closed = AI ランタイムとの接続が切断されました。
runtime-error-mind-prerequisite = 必要なコンポーネントがありません: { $name }
runtime-error-bootstrap = 起動に失敗しました: { $message }
runtime-error-config = 設定エラー: { $detail }
runtime-error-memory = メモリストアエラー: { $detail }
runtime-error-mind = マインドエンジンエラー: { $detail }
runtime-error-tool = ツールエラー: { $detail }
runtime-error-actor-busy = Ene は現在ほかのリクエストを処理中です。しばらくしてから再試行してください。
runtime-error-store-required = スケジューラーにはメモリストアが必要です。設定で `store.enabled` を有効にしてください。
runtime-error-ai-auth = AI プロバイダーの認証に失敗しました。API キーを確認してください。
runtime-error-ai-rate-limit = AI プロバイダーのレート制限に達しました。しばらくしてから再試行してください。
runtime-error-ai-network = AI プロバイダーに接続できませんでした。ネットワーク接続を確認してください。
runtime-error-ai-local-llm = ローカルモデルエラー: { $detail }
runtime-error-ai-busy = AI プロバイダーが現在混み合っています。しばらくしてから再試行してください。
runtime-error-ai-provider = AI プロバイダーエラー: { $detail }
runtime-error-ai-embedding = 埋め込みプロバイダーエラー: { $detail }
runtime-error-turn-failed = { $detail }
runtime-error-turn-failed-unknown = 不明な理由でリクエストが失敗しました。
