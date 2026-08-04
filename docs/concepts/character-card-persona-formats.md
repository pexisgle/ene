# Persona Formats (W++ / AliChat / YAML)

Character cards from Chub.ai, JanitorAI, and the SillyTavern ecosystem often
carry their persona in pseudo-structured text instead of plain prose: W++
blocks, AliChat key/value text, or flat YAML. Ene detects these shapes in the
card's `personality` and `description` fields, extracts the attributes, and
feeds a dense `Label: value` representation into the Identity Kernel — the
format syntax (brackets, quotes, `key:` wrappers) is dropped, so the same
content fits in fewer tokens.

## Formats

### W++

A `[character("Name"){...}]` block whose attributes are
`Attribute("value")` pairs. Values are comma-separated quoted strings that may
repeat (`Personality("A", "B")`), span lines, use single or double quotes, and
contain backslash escapes. Optional `;` separators between attributes are
accepted.

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

`Key: value` lines using the standard AliChat key set (`Name`, `Age`,
`Gender`, `Personality`, `Description`, `Scenario`, `First message`,
`Example messages`, ...). A key with an empty value collects the following
lines — bullet lists and prose paragraphs alike — until the next key.

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

Flat `key: value` mappings with persona keys beyond the AliChat set. Only the
scalar subset is parsed: one key per line, optional surrounding quotes.
Block-scalar indicators (`|`, `|-`, `>`, ...) are treated as an empty value
whose following lines form the value. Nested structures (maps, or lists other
than `- ` bullets) are not supported.

```
appearance: "Tall, silver hair"
personality: Curious and warm
mind: Analytical
speech_pattern: Soft-spoken
background: Lighthouse keeper
species: Half-elf
```

## Detection rules

- W++ is recognized only when the text starts with `[character("..."){...}]`
  and parses cleanly to the closing `]`.
- AliChat / YAML text must be *entirely* key lines or value continuations:
  at least two key/bullet lines and at least one persona-vocabulary key.
  A `Name:` line inside prose never triggers detection by itself. Key-like
  lines outside the vocabulary (for example `User: ...` inside
  `Example messages:`) are kept as part of the previous value; before any
  vocabulary key they mark the whole text as unrecognized.
- **Fallback**: any unrecognized text (plain prose, malformed W++, nested
  `character(...)` style, single-key mappings, unknown keys before any
  vocabulary key) is passed to the Identity Kernel byte-for-byte, exactly as
  before.

## Dense rendering

Detected attributes are rendered as short labeled lines, canonical attributes
first (`Appearance`, `Personality`, `Mind`, `Speech pattern`, `Background`,
`Description`), then the remaining keys in source order with their original
labels:

```
Core personality: Curious, Warm
Appearance: Tall, Silver hair
Mind: Analytical
Speech pattern: Soft-spoken
Background: A lighthouse keeper who lives alone
Age: 23
```

The `Personality` (or `Description`) attribute feeds the core personality
line and is not repeated below it. All extracted values still go through the
usual CBS macro expansion (`{{user}}`, `{{pick}}`, ...).

## Behavior in the Identity Kernel

- A recognized `personality` field supplies the dense core personality and
  attribute lines.
- A recognized `description` field is dense-rendered into the
  `## Background` section; when `personality` is empty it also supplies the
  core personality and attribute lines.
- Unrecognized fields keep the legacy behavior: raw text, with the
  description truncated to 240 characters when it serves as the core
  fallback.
- Kernel truncation is unchanged: when the block exceeds the token budget,
  attribute lines are dropped from the end first, and the anti-spoofing hard
  instruction always survives.

## Limitations

The parser is deliberately conservative and dependency-free. Nested YAML
structures (maps, or lists other than `- ` bullets) and the nested
`character(...)` W++ dialect are not recognized; such cards keep the raw-text
path. Block-scalar indicators (`|`, `|-`, `>`, ...) are accepted: the marker
is dropped and the following lines become the value.
