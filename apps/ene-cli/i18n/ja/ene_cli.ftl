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

## /memory approval コマンド
memory-approval-usage = /memory approval <list|inspect <id>|approve <id>|edit <id> --title <タイトル> --content <内容> --kind <種別> --confidence <0..1>|reject <id>|history>
memory-approval-list-title = 承認待ちの候補 ({ $count })
memory-approval-history-title = 解決済み候補の履歴 ({ $count })
memory-approval-empty = 承認待ちの候補はありません。
memory-approval-history-empty = 承認・却下された候補はまだありません。
memory-approval-not-found = 候補 { $id } が見つからないか、すでに解決されています。
memory-approval-label-id = id
memory-approval-label-title = タイトル
memory-approval-label-kind = 種別
memory-approval-label-confidence = 信頼度
memory-approval-label-reason = 理由
memory-approval-label-source-quote = 出典
memory-approval-label-source-turn = 出典ターン
memory-approval-label-conflict = 競合対象
memory-approval-label-status = 状態
memory-approval-label-created = 作成日時
memory-approval-label-resolved = 解決日時
memory-approval-status-pending = 保留中
memory-approval-status-approved = 承認済み
memory-approval-status-rejected = 却下済み
memory-approval-approve-ok = 候補 { $id } を承認しました。
memory-approval-reject-ok = 候補 { $id } を却下しました。
memory-approval-edit-ok = 候補 { $id } を編集しました。
memory-approval-edit-missing-flag = 編集には --title・--content・--kind・--confidence がすべて必要です。
memory-approval-edit-invalid-confidence = 信頼度は 0 から 1 の数値で指定してください。
memory-approval-edit-invalid-kind = 不明なメモリ種別 '{ $kind }' です。有効: episodic, semantic, user_profile, relationship, affective, commitment, preference, procedure, reflection
memory-approval-error = 承認エラー: { $error }

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
