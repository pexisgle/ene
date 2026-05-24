# APIキー/秘密情報の管理改善

## 目的
`settings.json` に API キーを平文保存する状態を避け、**OS の秘密情報ストア**や **環境変数**を利用できるようにする。  
GUI/CLI の双方で一貫した取り扱いを提供する。

## 現状
`ProviderSettings::resolve_api_key()` は設定ファイルの `api_key` を最優先で使用する。  
環境変数 `API_TOKEN` は **デバッグビルドのみ**のフォールバックであり、運用では平文保存になりやすい。

## 改善方針
### 1) 秘密情報の「取得元」を明示する
`provider` 設定に以下を追加する。
- `api_key_source`: `inline` / `env` / `keyring`
- `api_key_env`: 使用する環境変数名（例: `OPENAI_API_KEY`）
- `api_key_keyring_service` / `api_key_keyring_account`: keyring の識別子

### 2) GUI での入力は keyring に保存
GUI で API キーを入力した場合は、**settings.json には保存せず** keyring に格納する。  
`settings.json` には `api_key_source = "keyring"` のみを保存する。

### 3) CLI での取り扱い
CLI では以下の 2 方式を推奨。
- 環境変数運用（CI/サーバー向け）
- `--set-api-key` オプションで keyring に保存

## 実装ステップ
1. `ProviderSettings` に `api_key_source` などの新規フィールドを追加
2. `resolve_api_key()` を **source 分岐**で実装
3. GUI の保存処理で `api_key` をファイル保存しないよう修正
4. CLI に keyring 保存のサブコマンド/フラグを追加
5. 既存 `settings.json` の `api_key` が残っている場合は **移行プロンプト**を表示

## 移行ポリシー
- 起動時に `api_key` が平文で存在する場合は警告
- ユーザーが同意したら keyring に移行し、`api_key` を空文字に上書き保存

## 影響と互換性
keyring 未対応環境では `env` をフォールバックにし、  
`inline` は **明示的選択時のみ許可**する（安全なデフォルトを維持）。
