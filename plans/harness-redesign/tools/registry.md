# 統一ツールレジストリ

> 実現する要件: **P-601**(統一レジストリ)、P-503 のツール面、
> P-613(ユーザー可視化)、P-614(巨大出力)、**P-615**(ハーネス機能ツールの目次)。

## Tool Calling と MCP の分離(この文書の前提)

この2つは**レイヤーが異なる**ので、競合ではなく統合する。

- **Tool Calling**: モデル⇄ハーネスの呼び出し規約。モデルはスキーマを
  見て `tool_call` を出し、実行はすべてハーネスが行う。
  レジストリはこの「モデルに見える唯一の面」を所有する。
- **MCP**: 外部のツール/リソース/プロンプト**供給元**への接続規約。
  MCP サーバーはレジストリに供給元の一つとして接続する
  ([capabilities.md §5](capabilities.md#5-mcp-seamp-603))。

```text
モデル ⇄ Tool Calling(層ごとにフィルタされたスキーマ面)
              ↑
      統一ツールレジストリ(この文書)
              ↑
  供給元: ツールプラグイン(IPC) / MCP / ハーネス機能(ホスト内) / 委譲
```

## 0. 2軸の分類(D-10)

「全部アウトプロセス」ではない。実行場所と、モデルから呼べるかは**別の軸**である。

| | ホスト内実行 | アウトプロセス実行 |
|---|---|---|
| **モデルから呼べる** | **ハーネス機能ツール**(§0.1) | **ツール**(§0.2) |
| **モデルから呼べない** | ハーネス内部処理(コンパクション・記憶抽出・感情更新・能動発話の観測)。レジストリに現れない | — |

### 0.1 ハーネス機能ツール(ホスト内)

ハーネス状態と密結合するため、プロセス境界の向こうに置けないもの。
引数と振る舞いの本体は各領域の文書。この節は**名前の目次**だけを置く(P-615)。

| 名前 | 公開 | 本体 |
|---|---|---|
| `memory.recall` | 表層+裏層 | [../companion/memory.md §5](../companion/memory.md#5-想起p-203--d-18) |
| `memory.write_shared` | 裏層 | [../companion/memory.md §4.3](../companion/memory.md#43-明示的な書き込みp-212) |
| `delegate.start` | 表層+裏層 | [../core/delegation.md §4](../core/delegation.md#4-層間エンベロープと表層--裏層) |
| `delegate.instruct` | 表層+裏層 | 同上 |
| `delegate.message` | 表層+裏層 | 同上 |
| `delegate.answer` | 表層+裏層 | 同上 |
| `delegate.status` | 表層+裏層 | 同上 |
| `delegate.cancel` | 表層+裏層 | 同上 |
| `delegation.send` | 裏層 | [../core/delegation.md §5](../core/delegation.md#5-子--親メールボックス) |
| `question.ask` | 表層+裏層 | [../core/agent-loop.md §9](../core/agent-loop.md#9-人間協調面plan--ask-userp-511-p-512) |
| `approval.policy_add` | 裏層 | [../security/approval.md §5](../security/approval.md#5-対話からのポリシー追加p-906) |
| `skill.load` | 表層+裏層 | [../tasks/skills.md](../tasks/skills.md) |
| `skill.read` | 表層+裏層 | 同上 |
| `job.plan_write` | 裏層 | [../tasks/jobs-and-schedules.md §4](../tasks/jobs-and-schedules.md#4-ワークフローp-608) |
| `artifact.register` | 裏層 | [../tasks/jobs-and-schedules.md §5](../tasks/jobs-and-schedules.md#5-artifact-体系p-609) |
| `spill.read` | 表層+裏層 | [../core/context-assembly.md §5](../core/context-assembly.md#5-spill) |

内面はツールではない。出力契約とシステム生成
([../companion/inner-channel.md](../companion/inner-channel.md))。
compaction・記憶抽出・感情更新・能動発話の観測もツールではない(§0 の下段)。

公開列は §2.1 のフィルタ結果。`delegate.*` は副作用があっても表層に出す例外。
`memory.write_shared` / `approval.policy_add` / `job.plan_write` /
`artifact.register` / `delegation.send` は副作用があるので表層には出ない。

これらをアウトプロセスにすると、記憶ストアやレーン状態への操作が
IPC の往復になり、コストにも整合性にも見合わない。委譲に至っては、
「ハーネスにレーンを起こさせる」操作なので、外から呼ぶ意味がない。

### 0.2 ツール(アウトプロセス)

外部世界に触れるもの。fs / exec / web / app / utility と、
サードパーティのプラグイン。

**ビルトインもサードパーティと同じ API に載せる。** 別扱いにすると
実装が二重になるうえ、「`fs` と同等のツールを後から third-party が作れる」
という保証が失われる。ビルトインを特別扱いしないことが、
プラグインコミュニティ(D-26)の前提になる。

`tool/call.source` は `plugin:<id>` / `mcp:<server>` / `harness:<name>` /
`delegated` のいずれか
([../core/session-log.md §3.4](../core/session-log.md#34-ツール))。

登録そのものは供給元ファイバーの巻き戻し可能な effect である(D-32)。
ファイバーが `unloading` に入ると、その供給元のツールはスキーマ面から
降り、進行中の呼び出しは `cancel` される。降りたあとに表層/裏層が
その名前を見てはならない
([../plugins/composition.md §3](../plugins/composition.md#3-巻き戻し可能な-effect時間))。

## 1. 責務と責務外

- **責務**: ツール定義の登録・照合、層ごとのモデル向けスキーマの公開、
  実行パイプラインの駆動(IPC / MCP / ホスト内)、出力の正規化と上限、
  表示用の投影。
- **責務外**: アウトプロセスツール本体の実装(供給元プラグインの責務)、
  承認判断([../security/approval.md](../security/approval.md))、
  spill の保管([../core/context-assembly.md](../core/context-assembly.md))。

## 2. ToolDefinition

登録される1ツールの全フィールド。**モデルに見えるフィールド**と
**ホスト専用フィールド**を明確に分ける(dsh に倣う)。

### モデル公開(スキーマ面)

| フィールド | 型 | 説明 |
|---|---|---|
| `name` | string | 名前空間付き `<namespace>.<action>`(例: `fs.read`) |
| `description` | string | モデル向けの説明 |
| `parameters` | JSON Schema | 引数のスキーマ。`additionalProperties` は必ず明示 |

### ホスト専用(絶対リークしない)

| フィールド | 型 | 説明 |
|---|---|---|
| `output` | OutputDefinition | 正規出力の宣言。`schema`(JSON Schema)+ `render`(正規値→表示ブロックの純関数) |
| `execute` | 実行ハンドラ | 供給元への IPC(プラグイン)、MCP 呼び出し、またはホスト内呼び出し(ハーネス機能ツールのみ、§0.1) |
| `timeout_ms` | u32? | 協調的期限。モデルには送らない |
| `is_concurrency_safe` | 判定関数? | 並行実行の可否を引数から判定。`true` の返却のみが並行を許可 |
| `side_effects` | [string] | 副作用の宣言(`fs.write`, `net`, `send` 等)。**空であることが「表層に公開してよい」の唯一の条件**(§2.1)。承認 plane も参照する |
| `present_call` / `present_result` | 投影関数? | UI の「今何をしているか」表示。純関数で、ライブとリプレイの両方から呼ばれる |
| `finalize_content` | 変換関数? | モデル向けコンテンツの最終変換(出力上限の強制)。total で例外を投げない義務 |

- `schemas()` がモデル向けスキーマ配列を作る際、ホスト専用フィールドは
  ホワイトリスト(`name`/`description`/`parameters`)で**排除**する。
  これがリーク防止の実装契約である。
- ループがモデルへ渡す配列は、`schemas()` の**そのステップ開始時のコピー**
  である([../core/agent-loop.md §5](../core/agent-loop.md#5-ツール実行の駆動))。
  ライブのレジストリはファイバー unload で直ちに行を降ろすが、
  今のステップのモデル要求は書き換えない。
- `output` は**必須**。正規値(スキーマで検証済み)がログとリプレイの
  安定単位になり、表示は `render` で後から作れる。

### 2.1 層ごとの公開面(P-522 / D-2)

レジストリは1つだが、モデルに渡すスキーマ面は層で分ける。

**分ける基準は `side_effects` が空かどうか、ただ1つ。**

| 層 | 公開するツール |
|---|---|
| 表層 soul | `side_effects` が空のツールすべて + `delegate.*` |
| 裏層ハーネス | 全ツール + `delegation.send` + 再帰用 `delegate.*` |

表層に読み取り専用ツールを出すのは、「今何時?」「さっきのファイル名は?」の
ために委譲の往復を起こさないためである(D-1)。会話の即応性は存在感の中核で、
一律委譲はそれを壊す。

一方、境界を「簡単さ」で引かないのは、簡単さがモデルの自己申告でしか
判定できないからである。それを副作用の許可条件にすると事故が防げない。
`side_effects` はツール定義から静的に決まるので、境界として信頼できる。

実装契約:

- `schemas()` は呼び出し元レーンの層を見てフィルタする。
  表層モデルが `fs.write` を見てはならない。
- 表層のフィルタは**ホワイトリストではなく `side_effects` の判定**で行う。
  ツール名を列挙すると、新しい読み取り専用ツールを足すたびに
  表層のリストを更新し忘れる。
- 表層のモデルが未公開の副作用ツールを呼ぼうとした場合、そのツールは
  **実行せず**、委譲へ自動昇格する
  ([../core/agent-loop.md §2.3](../core/agent-loop.md#23-委譲への自動切り替えp-523--d-3))。
  実行してから昇格するのでは境界を設けた意味がない。
- 供給元の登録そのものは層を問わない。ターン予算・Context Source・waterfall は
  層の専用実装
  ([../core/agent-loop.md §2.1](../core/agent-loop.md#21-エージェント実装の登録表p-522))。

## 3. 実行パイプライン

```text
tool.call(モデル要求)
  → 引数の検証(parameters スキーマ。失敗は即 error、実行しない)
  → tools/pre-execute(waterfall)
      ├─ 承認 plane: 副作用と対象の機微さでポリシー照合(保留/許可/拒否)
      ├─ guard: 期限の設定
      └─ delegation: 長時間ツールの非同期委譲への移管提案([../core/delegation.md](../core/delegation.md)。判定のみ、強制しない)
  → execute(供給元へ委譲。signal/deadline を伝播)
  → tools/post-execute(waterfall)
      ├─ 正規出力の検証(output.schema)
      ├─ guard: 繰り返し検知・出力肥大(spill 判定)
      └─ フック(監査・メモリ抽出の tool_grounding)
  → tool.result(正規値を記録し、render で表示ブロック化)
```

- **ハーネス機能ツール(§0.1)も同じパイプラインを通る。** 実行段が
  IPC ではなくホスト内呼び出しになるだけで、引数検証・承認・出力正規化・
  ログ記録は共通である。ここを分岐させると、記憶想起だけ監査に残らない
  といった穴が空く。
- パイプラインの失敗は `tools/post-execute` を**バイパス**しても
  `finalize_content` を必ず1回呼ぶ(出力上限の保証のため)。
- 実行の中断: `signal` を伝播し、供給元は quiescence に至る義務
  ([../plugins/ipc.md §5](../plugins/ipc.md#5-キャンセル伝播と期限))。
- 並行呼び出し: `is_concurrency_safe(args) == true` 同士のみ並行
  ([../core/agent-loop.md §5](../core/agent-loop.md#5-ツール実行の駆動))。

## 4. 出力の正規化と上限

- ツールは**正規値**(output.schema に従う JSON)を返す。
  テキスト・画像・表はブロックではなく正規値のフィールドとして表現。
- 上限([../core/context-assembly.md §6](../core/context-assembly.md)):
  正規値がソフトリミット超えで spill 候補、ハードリミット超えで必ず spill。
  `finalize_content` が最終的なコンテンツ境界を強制する。
- エラーも正規化: `{ status: error, error_class, message? }` を
  モデルに返す(例外の生のスタックは出さない)。

## 5. 表示の投影(P-613 / D-11)

投影は表示面の深さごとに違う([../core/visibility.md](../core/visibility.md))。

| 表示面 | ツール呼び出しの見え方 |
|---|---|
| 表層UI | `present_call` / `present_result` が作る**1行の要約**のみ |
| 詳細画面 | 引数原文・正規値・生出し(spill 先への参照を含む) |

- `present_call(args)`: 呼び出しの「保留中」表示を args から作る。
  ライブとリプレイの両方から呼ばれるため**純関数**。
- `present_result(args, result)`: 完了表示。既定はツール名+正規値の要約。
- 内部機構(サブエージェントの委譲、compaction、メモリ抽出)は
  表層UIの投影に出さない(P-508)。詳細画面には出る。
- 深さの判定はサーバ側で行い、クライアントのフィルタに依存しない。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| 引数のスキーマ違反 | 実行せず `error{error_class: invalid_args}` をモデルへ。連続違反は guard が検知 |
| 供給元の死亡(IPC/MCP) | `plugin_dead` / `mcp_unavailable`。ファイバー unload でスキーマ面から降り、再起動待ち後に1回再試行([../core/agent-loop.md §8](../core/agent-loop.md#8-ガードとエラー回復)) |
| ファイバー unload 中の呼び出し | 新規は受け付けず `plugin_unloading`。進行中は `cancel` |
| 正規出力の検証失敗 | 供給元のバグ。`error{error_class: bad_output}` をモデルへ、ライフサイクルで警告 |
| 承認のタイムアウト | `denied{reason: timeout}` をモデルへ。対話は継続 |
| finalize_content の例外 | total の義務違反。既定のコンテンツを維持し、警告ログ |
| 表層から副作用ツールの呼び出し | 実行せず委譲へ昇格(§2.1)。モデルにはエラーを返さない |
| `side_effects` の宣言漏れ | サードパーティの誤りが表層への公開に直結する。登録時に、宣言が空で実行系に書き込み系ケイパビリティを要求するプラグインを拒否する |

## 7. 設定キー

具体的な数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `tools.timeout.default` | timeout 未宣言ツールの既定 |
| `tools.output.soft_limit_tokens` | spill 候補([context-assembly](../core/context-assembly.md) と同一値を保つ) |

---

- 次: [capabilities.md](capabilities.md)
