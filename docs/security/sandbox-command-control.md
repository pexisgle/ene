# サンドボックスのコマンド制御強化（Shell Allowlist/No-Shell 実行）

## 目的
正規表現ベースの `blocked_commands` だけでは回避が容易なため、**安全に実行可能なコマンドの明示的許可**と、**シェル経由実行の抑制**で実効性を高める。

## 現状
`shell_platform::execute_shell_command()` が `sh -c` / `cmd /C` を使っており、  
`blocked_commands` の正規表現をすり抜ける回避手段（エスケープ・パイプ・別名経由）が残る。

## 改善方針
### 1) No-Shell 実行を基本にする
- `shell` ツールは **コマンドラインをトークン分割**し、`std::process::Command` で直接実行する
- `|`, `;`, `&&`, `||`, `>`, `<`, `$()`, `` ` ``, `&` など **メタ文字を含む場合は拒否または要承認**
- どうしても必要な場合のみ `allow_shell_eval` を有効化

### 2) Allowlist を追加
以下のような **コマンド許可リスト**を `sandbox` 設定に追加する。
- `allowed_commands`: コマンド名の一覧（例: `ls`, `cat`, `rg`, `git`）
- `allowed_command_args`: コマンドごとの引数制約（正規表現 or JSON Schema）
- `allow_network`: ネットワーク系コマンドの許可フラグ

Allowlist にないコマンドは **拒否 or PermissionRequired** とする。

### 3) ポリシー構成
設定に以下を追加し、運用で調整できるようにする。
- `allow_shell_eval`: シェル評価（`sh -c` 等）を許可するか
- `deny_by_default`: allowlist にないコマンドを拒否するか
- `require_permission_for_shell`: シェル評価時に必ず承認を要求するか

### 4) 実行フロー（推奨）
1. コマンドラインをトークン分割（失敗時は拒否）
2. メタ文字を検出したら **拒否** もしくは **PermissionRequired**
3. allowlist でコマンド名を検査
4. 引数制約（許可パス、危険フラグ）をチェック
5. `Command` で直接実行し、既存のタイムアウト/出力制限を適用

### 5) 監査/ログ
- 実行コマンド名と許可/拒否理由をログ化
- 引数に秘密情報が含まれる可能性があるため **値はマスク**する

## 実装ステップ
1. `shell_platform` を No-Shell 実行に変更（`Command::new(cmd).args(args)`）
2. トークン分割（例: `shell-words`）とメタ文字検出を追加
3. `sandbox` セクション（`SandboxConfigData`）と `settings.schema.json` に allowlist 設定を追加
4. `blocked_commands` は「最終防衛線」として残す
5. 代表的な回避パターンに対するテストを追加

## 互換性
シェル機能（パイプ/リダイレクト）に依存する操作は制限される。  
必要な場合は `allow_shell_eval` と Permission Gate の併用を前提にする。
