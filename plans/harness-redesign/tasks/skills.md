# skill 体系

> 実現する要件: **P-610**(skill)。MCP prompts との接点(P-603 の一部)。

## 1. 設計の立場

- skill は**非コーディングタスクの手順知識**の主戦力。
  「旅行の立て方」「朝のブリーフィング」「議事録の取り方」のように、
  手順+参照文書+任意のスクリプトを1パッケージにしたもの。
- **業界標準に合わせる**(確定済み決定): Anthropic 系の Agent Skills
  慣習に倣い、`SKILL.md`(YAML frontmatter+本文)をエントリとする。
  これによりコミュニティの skill 資産をそのまま流用できる。
- Ene の特性(コンパニオン・感情・内面)で不足する点は
  **独自拡張**として frontmatter に追加する(§3)。

## 2. パッケージ構造

```text
skill.travel/
├─ SKILL.md              # エントリ(必須)
├─ references/           # 参照文書(任意。必要時に読み込む)
│  └─ checklist.md
├─ scripts/              # 実行可能な補助スクリプト(任意・承認制)
│  └─ calc_budget.py
└─ assets/               # テンプレート等の資産(任意)
   └─ itinerary_template.md
```

`SKILL.md`:

```markdown
---
name: travel
description: 旅行の計画・しおり作成を支援する。行き先・日程・予算から旅程を立てる。
---

# 旅行プランニング

(手順の本文。モデルが読む指示)
```

- `description` は**カタログ表示と発動判断**に使われる。
  モデルは skill のカタログ(name+description)を System Context で
  参照し、該当する依頼が来ると本文を読み込む。
- 本文の注入は**遅延読み込み**: カタログは常に載るが、
  本文は発動時のみ Context Source(`skills.active`)に載せる
  ([../core/context-assembly.md §2](../core/context-assembly.md))。
  これで多数の skill を入れても窓を圧迫しない。

## 3. Ene 独自拡張(frontmatter)

| フィールド | 型 | 説明 |
|---|---|---|
| `name` / `description` | string | 標準 |
| `ene.proactive_hint` | string? | 能動発話での発動示唆(「朝にブリーフィングを提案」) |
| `ene.emotion_note` | string? | このタスク中のトーン指示(感情ボキャブラリ参照可) |
| `ene.schedule_spec` | cron? | この skill が定期実行向けの場合の推奨スケジュール |
| `ene.artifact_kinds` | [string]? | 成果物の種別宣言([jobs-and-schedules.md §5](jobs-and-schedules.md#5-artifact-体系p-609)) |

- `ene.*` を知らない他実装はこれらを無視できる(frontmatter の
  未知キー無視は業界慣習)。標準との互換は保たれる。

## 4. カタログと読み込み

### カタログ

- skill は soul ごとに有効化される(soul 定義の `[skills].refs`、
  [../character/package-format.md §5](../character/package-format.md#5-soul-定義soultoml))。
- カタログは `<data>/skills/<name>/` のインストール済み集合+
  soul の refs の交差。
- skill の配布は**キャラパッケージの配布仕様**に乗る
  ([../character/package-format.md §11](../character/package-format.md#11-配布仕様p-806))。
  skill 単独アーカイブ(`.eneskill`)= zip 化した skill ディレクトリ。

### 発動と実行

1. モデルが依頼と skill の description を照合し、本文の読み込みを
   要求(`skill.load(name)`。skill はハーネスのコンテキスト組み立てに
   直結するので**ハーネス機能ツール**、
   [../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))。
2. ロードされた本文は `skills.active` Source に載り、
   ターンを跨いで保持(セッション中)。
3. `references/` は必要時に `skill.read(name, path)` で読む
   ( progressive disclosure。全部を一度に載せない)。
4. `scripts/` の実行は **承認制**: exec ツール経由で走り、
   承認 plane のポリシー対象になる
   ([../security/approval.md](../security/approval.md))。
   キャラクターパッケージと異なり skill はスクリプトを持ち得るが、
   その分承認を必須にする。

## 5. MCP prompts との関係(P-603)

- MCP サーバーの **prompts** は skill 素材として取り込める
  ([../tools/capabilities.md §5](../tools/capabilities.md#5-mcp-seamp-603))。
- 変換: MCP prompt(argument 付きテンプレート)→ 擬似的な
  skill 本文としてカタログに登録(`ene.source = mcp:<server>` 付き)。
  発動時の実体化は MCP の `prompts/get` を呼ぶ。
- MCP prompt は**読み取り専用**として扱い、scripts は持たない。

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| frontmatter の欠落/不正 | インストール拒否+理由通知 |
| 本文の窓超過 | 本文を要約版に切替(要約はインストール時に生成してキャッシュ) |
| script の承認拒否 | skill は本文のみで継続(スクリプトなしでできる範囲で) |
| MCP prompt の取得失敗 | その発動を諦め、ライフサイクル警告 |
| 多数 skill のカタログ肥大 | カタログは name+description のみなので肥大しにくい。上限超過で警告 |

## 7. 設定キー

数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `skills.home_dir` | インストール先(既定は `<data>/skills`) |
| `skills.catalog.max_entries` | カタログ上限 |
| `skills.scripts.require_approval` | script 実行の承認必須。**既定 `true` は変更不可** |

---

- 前: [jobs-and-schedules.md](jobs-and-schedules.md) / 次: [../platform/README.md](../platform/README.md)

