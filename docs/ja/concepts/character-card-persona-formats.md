# ペルソナフォーマット（W++ / AliChat / YAML）

Chub.ai や JanitorAI、SillyTavern エコシステムのキャラクターカードは、
ペルソナを素の散文ではなく疑似構造化テキストで持つことがよくあります。
W++ ブロック、AliChat のキー/値テキスト、フラットな YAML などです。
Ene はカードの `personality` と `description` フィールドでこれらの形式を
検出し、属性を抽出して、高密度な `Label: value` 表現として Identity Kernel
へ渡します。形式の構文（括弧、引用符、`key:` のラッパー）を捨てることで、
同じ内容をより少ないトークンで表現します。

## 形式

### W++

属性が `Attribute("value")` のペアになっている `[character("Name"){...}]`
ブロックです。値はコンマ区切りの引用文字列で複数指定でき
（`Personality("A", "B")`）、複数行にまたがり、シングルクォート /
ダブルクォートのどちらも使え、バックスラッシュによるエスケープを含められます。
属性間の省略可能な `;` 区切りも受け付けます。

```
[character("Mira")
{
Age("23")
Appearance("Tall", "Silver hair")
Personality("Curious", "Warm")
Mind("Analytical", "Occasionally dreamy")
Speech("Soft-spoken", "Pauses often")
Background("A lighthouse keeper who lives alone")
}]
```

### AliChat

標準的な AliChat のキー集合（`Name` / `Age` / `Gender` / `Personality` /
`Description` / `Scenario` / `First message` / `Example messages` など）を
使った `Key: value` 行です。値が空のキーは、次のキーが現れるまで続く行
（箇条書きも散文の段落も）を値として集めます。

```
Name: Mira
Age: 23
Gender: Female
Personality:
- Curious
- Warm
Description:
A lighthouse keeper with silver hair who lives alone.
Scenario:
A stormy night on the cliffs.
```

### YAML

AliChat のキー集合を超えるペルソナキーを持つ、フラットな `key: value`
マッピングです。解析対象はスカラーだけのサブセットで、1 行 1 キー、
値は前後を引用符で囲めるだけです。ブロックスカラー指示子（`|` / `|-` /
`>` など）は空値として扱い、後続の行を値として集めます。入れ子構造
（マップや `- ` 箇条書き以外のリスト）は対応しません。

```
appearance: "Tall, silver hair"
personality: Curious and warm
mind: Analytical
speech_pattern: Soft-spoken
background: Lighthouse keeper
species: Half-elf
```

## 検出規則

- W++ は、テキストが `[character("..."){...}]` で始まり、閉じ `]` まで
  完全にパースできた場合だけ認識されます。
- AliChat / YAML テキストは、*すべての行* がキー行か値の継続行でなければ
  なりません。キー行と箇条書き行が合計 2 行以上、ペルソナ語彙のキーが
  1 つ以上あることが条件です。散文の中の `Name:` 行だけでは検出されません。
  語彙にないキー風の行（`Example messages:` 内の `User: ...` など）は
  直前の値の一部として保持され、語彙キーの前に現れた場合はテキスト全体を
  未認識にします。
- **フォールバック**: 認識できなかったテキスト（素の散文、壊れた W++、
  入れ子の `character(...)` 形式、キー 1 行だけのマッピング、語彙キーの前の
  未知のキー）はこれまでどおりバイト単位でそのまま Identity Kernel へ渡されます。

## 高密度レンダリング

検出された属性は短いラベル付き行として描画されます。まず正規属性
（`Appearance` / `Personality` / `Mind` / `Speech pattern` / `Background` /
`Description`）、続いて残りのキーが元のラベルのまま出現順に並びます。

```
Core personality: Curious, Warm
Appearance: Tall, Silver hair
Mind: Analytical
Speech pattern: Soft-spoken
Background: A lighthouse keeper who lives alone
Age: 23
```

`Personality`（または `Description`）属性はコアの性格行に使われ、その下に
重複して描画されません。抽出された値はすべて通常どおり CBS マクロ展開
（`{{user}}` / `{{pick}}` など）を受けます。

## Identity Kernel での挙動

- 認識された `personality` フィールドは、高密度なコア性格と属性行を供給します。
- 認識された `description` フィールドは `## Background` セクションに高密度
  描画されます。`personality` が空の場合は、コア性格と属性行も供給します。
- 認識されなかったフィールドは従来どおりの挙動です。生テキストのまま、コア
  フォールバックに使われるときは description を 240 文字に切り詰めます。
- カーネルの切り詰めは変わりません。トークン予算を超えたときは末尾の属性行
  から先に落ち、なりすまし防止のハード指示は常に残ります。

## 制限

パーサーは意図的に保守的で、依存関係を持ちません。入れ子の YAML 構造
（マップや `- ` 箇条書き以外のリスト）と、入れ子の `character(...)` W++
方言は認識されず、そうしたカードは生テキスト経路のままです。ブロックスカラー
指示子（`|` / `|-` / `>` など）は受け付けます。マーカーは捨てられ、
後続の行が値になります。
