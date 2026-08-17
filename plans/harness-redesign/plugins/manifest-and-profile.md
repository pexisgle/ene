# プラグイン manifest とプロファイル

> 実現する要件: **P-1002**(プロファイル/バンドル/パッチ)、
> **P-902**(Broker 仲介の宣言面)、**P-909**(オフライン可のプロバイダ構成)、
> P-1008(コミュニティプラグインの形式面)。

## 1. manifest

各プラグインは1つの manifest を持つ。形式は MessagePack/JSON 両対応
(フィールド名は共通)。

```toml
# ene-plugin.toml(配布・開発時は TOML。ホストは正規化して扱う)
[plugin]
id = "tool.fs"                  # グローバル一意識別子(<kind>.<name>)
name = "filesystem"
version = "1.0.0"
entry = "bin/ene-plugin-fs"     # ホストからの相対パス

[protocols]                     # 実装する副プロトコルと対応版(D-22)
core = { min = 1, max = 1 }     # 必須
tool = { min = 1, max = 1 }     # 実装しない面は書かない
# provider.llm = { min = 1, max = 1 }
# capability   = { min = 1, max = 1 }

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

[provider]                      # provider 面を実装する場合のみ
tasks = ["llm.chat", "embed"]
local = true                    # ローカル実行可能(オフライン可の判定材料)

[integrity]
digest = "sha256:..."           # パッケージ内容のハッシュ
```

- **`kind` フィールドは持たない。** プラグインが何であるかは
  `[protocols]` が実装を名乗る面で決まる。ツールとプロバイダを兼ねる
  プラグインを1つの `kind` で表せないうえ、`kind` と実際に話す面が
  食い違う余地を作らないため。
- **`[protocols]` は権限審査の対象**。ここに書かれていない副プロトコルを
  `hello_ack` で名乗ったら切断する
  ([ipc.md §3](ipc.md#3-ハンドシェイク))。
- **能力宣言は要求であり、許可ではない**。ホストは manifest の要求と
  プロファイルのポリシーを照合し、`hello` の `capabilities` として
  **必要最小限**を付与する。
- `assets_bypass` は、manifest に列挙された同梱アセットパスへの読み取りを
  Broker 承認なしで許可する(P-902 の素通り)。書き込みは対象外。

### 完全性の検証(D-26)

**v1.0 は署名を要求しない。ハッシュ検証まで。**

- `[integrity].digest` はパッケージ内容の sha256。インストール時と
  起動時に照合する。不一致なら起動拒否。
- 署名は将来の追加項目として `[integrity]` に `signature` / `signer` を
  足せる形にしておく。v1.0 の manifest 構造とインストール経路は、
  署名を足すときに**変えなくてよい**ことだけを保証する。
- 出所が不明なプラグインは起動前にユーザー確認(承認 plane)を要する。
  確認は監査ログに残る。署名がない段階では、この確認が唯一の関門になる。

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
allow_unverified = false        # digest 照合に通らないものを起動するか
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

- 全 LLM タスクは `ai.tasks.<task>` でプロバイダ+モデルを指定する。
  クラウド/ローカルを問わない。
- **補助LLM のタスクも同じ表に並ぶ**(P-524、
  [../tools/capabilities.md §8.1](../tools/capabilities.md#81-補助llmp-524--d-15))。
  記憶抽出・画面要約・能動発話の判定・compaction は会話と別の
  モデルを指定できる必要がある。頻度が高く、会話ほどの品質を要さないため。
- 会話タスクの既定推奨はクラウド(品質)だが、ローカルプラグイン
  (`provider.local-llm` 系)が有効なら自動でフォールバックする設定を持つ。
- ローカルモデル構成では、`network` capability を持つプラグイン行を
  プロファイルから外すことで、外部通信ゼロ構成を作れる。

## 4. MCP サーバーの登録(D-23)

git・ブラウザ操作・カレンダー等は内製せず MCP に委ねる
([../tools/capabilities.md §4.1](../tools/capabilities.md#41-mcp-に委ねる領域p-604--d-23))。

- MCP サーバーはプロファイルの行として書ける。プラグイン行と同じ積層
  (バンドル→パッチ)に乗る。**v1.0 の接続手段はこれだけ**。
- 設定画面から代表的な MCP サーバーを選んで追加する導線(P-616)は後継。
  委ねた領域の使い勝手は導線に依存するが、それを v1.0 の受入条件に入れると
  「どのサーバーを推奨するか」の選定まで初版に持ち込むことになる。
- MCP サーバーへ渡す資格情報はボールト経由
  ([broker.md](broker.md))。プロファイルに平文で書かせない。

## 5. コミュニティ配布(P-1008 / D-26)

コミュニティ形成は目標だが、v1.0 の要件は「そこへ大規模改修なしに
到達できる形」であって、マーケットプレイスそのものではない。

- **v1.0**: カタログは持たない。配布は「アーカイブ+公開された sha256」で、
  インストール時にハッシュを照合する。
- インストールは `<data>/plugins/<id>@<version>/` へ展開し、
  プロファイル行で参照。展開前のハッシュ検証は必須。
- 自動更新はしない(更新はユーザー操作+確認)。
- **将来**: 署名カタログとマーケット。追加時に変えたくないのは
  ①manifest の構造、②インストール先のレイアウト、③プロファイル行の
  参照形式の3つで、これらは v1.0 で固定する。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| manifest の能力要求がポリシー超え | 起動せず、ライフサイクルイベントで「能力不足の拒否」を報告。ツリー全体は起動継続(行の無効化) |
| digest 不一致 | 起動拒否+監査ログ。改竄としてユーザー通知 |
| manifest 外の副プロトコルを名乗る | 切断([ipc.md §8](ipc.md#8-障害モード)) |
| パッチの target 不存在 | そのパッチ行を無視し警告(起動は継続) |
| MCP サーバーへの接続失敗 | その供給元のツールのみ無効化。他は影響しない |
| サンドボックス必須プラグインが非対応環境 | 起動しない(行の無効化)。対話機能は継続 |

## 7. 設定キー

| キー | 説明 |
|---|---|
| `plugins.profile` | 起動プロファイル(既定 `desktop`) |
| `plugins.home_dir` | インストール先(既定は `<data>/plugins`) |
| `plugins.policy.approval_mode` | プロファイルの承認モード既定。起動時に `approval.mode` を初期化する。実行時の正は `approval.mode`([../security/approval.md](../security/approval.md)) |
| `plugins.policy.allow_unverified` | digest 照合に通らないものの許可(既定 `false`) |
| `ai.tasks.<task>` | タスク別のプロバイダ+モデル。補助LLM のタスクも含む |

---

- 前: [ipc.md](ipc.md) / 次: [broker.md](broker.md)

