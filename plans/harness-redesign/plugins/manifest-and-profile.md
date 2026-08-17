# プラグイン manifest とプロファイル

> 実現する要件: **P-1002**(プロファイル/バンドル/パッチ)、
> **P-902**(Broker 仲介の宣言面)、**P-909**(オフライン可のプロバイダ構成)、
> P-1008(コミュニティプラグインの形式面)。

## 1. manifest

各プラグインは1つの manifest を持つ。形式は MessagePack/JSON 両対応
(フィールド名は共通)。配布パッケージでは署名対象になる。

```yaml
# ene-plugin.toml(配布・開発時は TOML。ホストは正規化して扱う)
[plugin]
id = "tool.fs"                  # グローバル一意識別子(<kind>.<name>)
name = "filesystem"
version = "1.0.0"
kind = "tool"                   # tool | provider | mcp-bridge
entry = "bin/ene-plugin-fs"     # ホストからの相対パス
protocol = { min = 1, max = 1 } # 対応 IPC 世代

[capabilities]                  # 要求する能力(ホストが検証して付与)
broker = [
  { op = "fs.read",  scope = "workspace" },
  { op = "fs.write", scope = "workspace" },
]
network = []                    # 直接ネットワーク(原則空。プロバイダのみ)
assets_bypass = true            # 同梱アセットは Broker 素通り(P-902)

[sandbox]
required = true                 # サンドボックス非対応環境では起動しない
fs_write = []                   # sandbox 内の書き込み可能領域(追加)

[provider]                      # kind = provider のみ
tasks = ["llm.chat", "embed"]
local = true                    # ローカル実行可能(オフライン可の判定材料)

[signature]
digest = "sha256:..."           # パッケージ内容のハッシュ
signer = "ene-official"         # 署名者(公式/コミュニティ/unsigned)
```

- **能力宣言は要求であり、許可ではない**。ホストは manifest の要求と
  プロファイルのポリシーを照合し、`hello` の `capabilities` として
  **必要最小限**を付与する([ipc.md §2](ipc.md#2-ネゴシエーション))。
- `assets_bypass` は、manifest に列挙された同梱アセットパスへの読み取りを
  Broker 承認なしで許可する(P-902 の素通り)。書き込みは対象外。
- `unsigned` プラグインは起動前にユーザー確認(承認 plane のポップアップ)
  を要する。確認は監査ログに残る。

## 2. プロファイル / バンドル / パッチ

dsh の Cordis 構成に倣い、起動するプラグイン集合を**積層**で決める。

```text
起動ツリー =
  プロファイルが列挙するバンドル(順に適用)
  → プロファイルのパッチ(patch.toml)
  → ホームレベルのパッチ
  → 起動時の --patch 上書き
```

### バンドル

プラグイン行の集合+任意のデフォルト設定。`ene-bundle.toml` で宣言:

```toml
[bundle]
id = "ene-base"

[[rows]]
plugin = "tool.fs"
config = { workspace_scope = "home" }

[[rows]]
plugin = "provider.openai"
```

### プロファイル

```toml
[profile]
id = "desktop"
bundles = ["ene-base", "ene-companion", "ene-desktop-stage"]

[policy]
approval_mode = "policy"        # 起動時に approval.mode を初期化(実行時の正は approval.mode)
allow_unsigned = false
```

出荷プロファイル:

- `desktop`(既定): 全ツール+コンパニオン系+stage。性能予算は別枠
  ([../platform/process-model.md §6](../platform/process-model.md#6-性能予算))。
- `minimal`: コア対話+最小ツール(オフライン軽量構成の土台)。**性能測定の正**。
- `headless`: CLI/自動化用。stage なし。

### パッチ

行を `id` で指定して**設定ごと置換**するか、新行を挿入する。

```toml
[[patch]]
target = "tool.fs"              # 行の置換
config = { workspace_scope = "project-x" }

[[patch]]
insert_after = "tool.fs"        # 新行の挿入
plugin = "tool.git"
```

- パッチは**上のレイヤーほど強い**(後から適用される)。
- `--dump-profile` で実際に起動するツリーを表示できる(検証手段)。

## 3. プロバイダの選択(P-909)

- 全 LLM タスク(chat/extract/classify/summarize/proactive/judge)は
  `ai.tasks.<task>` でプロバイダ+モデルを指定する。クラウド/ローカルを問わない。
- 会話タスクの**既定推奨**はクラウド(品質)だが、ローカルプラグイン
  (`provider.local-llm` 系)が有効なら自動でフォールバック可能にする設定
  (`ai.tasks.chat.fallback: local`)を持つ。
- ローカルモデル構成では、`network` capability を持つプラグイン行を
  プロファイルから外すことで、外部通信ゼロ構成を作れる。

## 4. コミュニティプラグイン(P-1008)

- 配布単位は**署名カタログ**: プラグイン manifest の集合+署名。
  カタログ外は「ローカルファイル+sha256 の手動インポート」のみ。
- インストールは `<data>/plugins/<id>@<version>/` へ展開し、
  プロファイル行で参照。展開前のハッシュ検証は必須。
- 自動更新はしない(更新はユーザー操作+確認)。

## 5. 障害モード

| 障害 | 挙動 |
|---|---|
| manifest の能力要求がポリシー超え | 起動せず、ライフサイクルイベントで「能力不足の拒否」を報告。ツリー全体は起動継続(行の無効化) |
| ダigest 不一致 | 起動拒否+監査ログ。なりすましとしてユーザー通知 |
| パッチの target 不存在 | そのパッチ行を無視し警告(起動は継続) |
| 署名カタログの期限切れ | 読み込みは可だが「検証できない」扱いで起動前確認 |
| サンドボックス必須プラグインが非対応環境 | 起動しない(行の無効化)。対話機能は継続 |

## 6. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `plugins.profile` | `desktop` | 起動プロファイル |
| `plugins.home_dir` | `<data>/plugins` | インストール先 |
| `plugins.policy.approval_mode` | `policy` | プロファイルの承認モード既定。起動時に `approval.mode` を初期化する。実行時の正は `approval.mode`([../security/approval.md](../security/approval.md)) |
| `plugins.policy.allow_unsigned` | `false` | unsigned の許可 |
| `ai.tasks.chat.fallback` | `local` | クラウド不能時のローカルフォールバック |

---

- 前: [ipc.md](ipc.md) / 次: [broker.md](broker.md)

