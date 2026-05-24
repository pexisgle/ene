# Permission Gate 強化設計（破壊的操作の承認フロー）

## 目的
破壊的操作（削除・上書き・シェル実行・GUI/ブラウザ操作など）を**明示的なユーザー承認**なしに実行しないようにし、GUI/CLI の両方で一貫した確認 UX を提供する。

## 現状
`crates/ene-tools/fs/src/permission.rs` に `PermissionGate` があり、`SandboxConfig` から許可判定を作れるが、各ツール処理に十分に組み込まれていない。  
`AiStreamEvent::PermissionRequired` は定義済みだが、実際のツール呼び出しフローと接続されていない。

## あるべきフロー
1. ツールが破壊的操作を検知した時点で **PermissionRequired を発生**。
2. core が **AiStreamEvent::PermissionRequired** を UI に通知。
3. UI が **Allow / Deny / Scope** を返却。
4. core が **同一ツール呼び出しを「承認済み」状態で再実行**。

## 仕様詳細
### 判定対象（最低限）
- fs: delete / write / edit / patch（ファイル破壊の可能性）
- shell: 任意コマンド（特に書き込み・削除・ネットワーク）
- app/browser: 外部入力や遷移を伴う操作

### 決定スコープ
最低でも以下の 3 段階を持つ。
- Allow Once（単発）
- Allow Session（同一セッションで再利用）
- Deny（1回拒否）

### IPC/エラー表現の拡張
現状は `ToolError::PermissionDenied { message }` しかないため、**構造化情報が失われる**。  
以下のいずれかで構造化データを伝搬する。
- 追加案A: `ToolError::PermissionRequired { request_id, action, target, description }` を追加
- 追加案B: `PermissionDenied` の `message` に JSON 形式で埋め込み（非推奨）

推奨は追加案A。`ene-tool-proto` の enum 追加と IPC roundtrip の更新で済む。

### 承認の受け渡し
**再実行時に「承認済み」トークン**を渡す必要がある。  
例: `{"approved_request_id": "<uuid>"}` を tool 引数に付与するか、`IpcRequest::CallTool` のメタに追加する。

### キャッシュと監査
- セッション単位のメモリに `[(action, target_pattern) -> scope]` を保存
- 許可/拒否を `conversation_logs` とは別に監査ログとして保存（任意）

## 実装ステップ（具体）
1. `ene-tool-proto::ToolError` に `PermissionRequired` を追加
2. fs/shell/app/browser の実装で `sandbox.check_permission(...)` を呼び出し、拒否時は `PermissionRequired` を返す
3. `ene-ai-core::run_ai_with_tools` で `PermissionRequired` を検出し `AiStreamEvent::PermissionRequired` を送出
4. CLI: `Allow Once/Session/Deny` を対話で選択
5. GUI: モーダルで承認・拒否・スコープを提示
6. 承認済みの場合のみ、同一 tool_call を再実行

## 影響と互換性
- IPC/ToolError の拡張が入るため、ツールバイナリと core の**バージョン整合性**が必須
- 未対応 UI では `PermissionRequired` を「拒否扱い」にするフォールバックを用意する
