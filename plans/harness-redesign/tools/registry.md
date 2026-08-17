# 統一ツールレジストリ

> 実現する要件: **P-601**(統一レジストリ)、P-503 のツール面、
> P-613(ユーザー可視化)、P-614(巨大出力)。

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
  供給元: バンドル済みプラグイン(IPC) / 追加プラグイン(IPC) / MCP / 委譲
```

**ビルトイン = バンドル済みアウトプロセスプラグイン**。ホスト内の
関数呼び出し(in-process execute)は禁止する(P-1001)。
`tool/call.source` は常に `plugin:<id>` / `mcp:<server>` / `delegated`
([../core/session-log.md §3.4](../core/session-log.md#34-ツール))。

## 1. 責務と責務外

- **責務**: ツール定義の登録・照合、層ごとのモデル向けスキーマの公開、
  実行パイプラインの駆動(IPC/MCP)、出力の正規化と上限、UI 表示用の投影。
- **責務外**: ツール本体の実装(供給元プラグインの責務。ホスト内には置かない)、承認判断
  ([../security/approval.md](../security/approval.md))、
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
| `execute` | 実行ハンドラ | 供給元への IPC(プラグイン)または MCP 呼び出し。ホスト内関数は置かない |
| `timeout_ms` | u32? | 協調的期限。モデルには送らない |
| `is_concurrency_safe` | 判定関数? | 並行実行の可否を引数から判定。`true` の返却のみが並行を許可 |
| `read_only` | bool | 副作用の有無(承認ポリシーの材料) |
| `side_effects` | [string] | 副作用の宣言(`fs.write`, `net`, `send` 等)。承認 plane が参照 |
| `present_call` / `present_result` | 投影関数? | UI の「今何をしているか」表示。純関数で、ライブとリプレイの両方から呼ばれる |
| `finalize_content` | 変換関数? | モデル向けコンテンツの最終変換(出力上限の強制)。total で例外を投げない義務 |

- `schemas()` がモデル向けスキーマ配列を作る際、ホスト専用フィールドは
  ホワイトリスト(`name`/`description`/`parameters`)で**排除**する。
  これがリーク防止の実装契約である。
- `output` は**必須**。正規値(スキーマで検証済み)がログとリプレイの
  安定単位になり、表示は `render` で後から作れる。

### 層ごとの公開面(P-522)

レジストリは1つだが、モデルに渡すスキーマ面は層で分ける。

| 層 | 公開するツール | 公開しないもの |
|---|---|---|
| 表層 soul | `delegate.*`(層間)、`memory.recall`、`clock.now`、内面局所 | fs / exec / web / browser / 送信系など副作用のある作業ツール。短い検索も裏層へ |
| 裏層ハーネス | 統一レジストリの作業ツール + `delegation.send` + 再帰用 `delegate.*` | ユーザー発話チャネル、表層専用の感情/能動発話ツール |

- `schemas()` は呼び出し元レーンの層を見てフィルタする。表層モデルが
  `fs.write` を見てはならない。
- 供給元の登録そのものは層を問わない。**公開ツール面**の差はレジストリの
  投影。ターン予算・Context Source・waterfall は層の専用実装
  ([../core/agent-loop.md §2.1](../core/agent-loop.md#21-エージェント実装の登録表p-522))。
  実行は常にアウトプロセス(バンドル済みでも IPC)。

## 3. 実行パイプライン

```text
tool.call(モデル要求)
  → 引数の検証(parameters スキーマ。失敗は即 error、実行しない)
  → tools/pre-execute(waterfall)
      ├─ 承認 plane: read_only 以外はポリシー照合(保留/許可/拒否)
      ├─ guard: 期限の設定
      └─ delegation: 長時間ツールの非同期委譲への移管提案([../core/delegation.md](../core/delegation.md)。判定のみ、強制しない)
  → execute(供給元へ委譲。signal/deadline を伝播)
  → tools/post-execute(waterfall)
      ├─ 正規出力の検証(output.schema)
      ├─ guard: 繰り返し検知・出力肥大(spill 判定)
      └─ フック(監査・メモリ抽出の tool_grounding)
  → tool.result(正規値を記録し、render で表示ブロック化)
```

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

## 5. UI 投影(P-613)

- `present_call(args)`: 呼び出しの「保留中」表示を args から作る。
  ライブとリプレイの両方から呼ばれるため**純関数**。
- `present_result(args, result)`: 完了表示。既定はツール名+正規値の要約。
- 内部機構(サブエージェントの委譲、compaction、メモリ抽出)は
  この投影に出さない(P-508、設計原則3)。
- 引数原文・生出し・thinking は投影しない(要約のみ、
  [../core/visibility.md](../core/visibility.md) §5)。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| 引数のスキーマ違反 | 実行せず `error{error_class: invalid_args}` をモデルへ。連続違反は guard が検知 |
| 供給元の死亡(IPC/MCP) | `plugin_dead` / `mcp_unavailable`。再起動待ち後に1回再試行([../core/agent-loop.md §8](../core/agent-loop.md#8-ガードとエラー回復)) |
| 正規出力の検証失敗 | 供給元のバグ。`error{error_class: bad_output}` をモデルへ、ライフサイクルで警告 |
| 承認のタイムアウト | `denied{reason: timeout}` をモデルへ。対話は継続 |
| finalize_content の例外 | total の義務違反。既定のコンテンツを維持し、警告ログ |

## 7. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `tools.timeout.default_ms` | `60000` | timeout 未宣言ツールの既定 |
| `tools.output.soft_limit_tokens` | `8000` | spill 候補([context-assembly](../core/context-assembly.md) と同一値を保つ) |
| `tools.registry.show_internal` | `false` | 内部機構の UI 投影を絶対に出さない(変更不可の既定) |

---

- 次: [capabilities.md](capabilities.md)
