//! `W++` / `AliChat` / `YAML` persona format parsing.
//!
//! Character cards carry persona prose in pseudo-structured formats: `W++`
//! blocks (`[character("Name"){Attribute("value")...}]`), `AliChat` key/value
//! text, or flat `YAML` mappings. This module detects those shapes and
//! extracts dense attribute lines so the identity kernel can drop the format
//! syntax (brackets, quotes, `key:` wrappers) and keep the content. Text that
//! does not match a recognized shape yields `None`, and the caller falls back
//! to the raw text byte-for-byte.

#![expect(
    clippy::arithmetic_side_effects,
    reason = "persona parser tracks line counters and computes quote-end byte offsets"
)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaFormat {
    /// `[character("Name"){Attribute("value")...}]` pseudo-structure.
    Wpp,
    /// `Key: value` text using the standard `AliChat` key set.
    AliChat,
    /// Flat `key: value` mapping with keys outside the `AliChat` set.
    Yaml,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructuredPersona {
    pub appearance: Option<String>,
    pub personality: Option<String>,
    /// Inner thoughts and mental model.
    pub mind: Option<String>,
    pub speech: Option<String>,
    pub background: Option<String>,
    /// Also the core-personality fallback.
    pub description: Option<String>,
    /// Remaining attributes with their original labels, in source order.
    pub extra: Vec<(String, String)>,
}

impl StructuredPersona {
    /// Dense `Label: value` lines, canonical attributes first, extras in
    /// source order.
    #[must_use]
    pub fn render_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        push_attr(&mut lines, "Appearance", self.appearance.as_deref());
        push_attr(&mut lines, "Personality", self.personality.as_deref());
        push_attr(&mut lines, "Mind", self.mind.as_deref());
        push_attr(&mut lines, "Speech pattern", self.speech.as_deref());
        push_attr(&mut lines, "Background", self.background.as_deref());
        push_attr(&mut lines, "Description", self.description.as_deref());
        for (key, value) in &self.extra {
            lines.push(format!("{key}: {value}"));
        }
        lines
    }

    /// Dense head lines minus the attribute already rendered as the core
    /// personality line (`Personality`, or `Description` when it served as the
    /// core fallback) and minus a `Name` attribute, which the kernel renders
    /// from the card's own name field.
    #[must_use]
    pub fn render_lines_excluding_core(&self) -> Vec<String> {
        let skipped = if self.personality.is_some() {
            Some("Personality: ")
        } else if self.description.is_some() {
            Some("Description: ")
        } else {
            None
        };
        self.render_lines()
            .into_iter()
            .filter(|line| {
                !line.starts_with("Name: ")
                    && skipped.is_none_or(|prefix| !line.starts_with(prefix))
            })
            .collect()
    }
}

fn push_attr(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value {
        lines.push(format!("{label}: {value}"));
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PersonaFormatParser;

impl PersonaFormatParser {
    /// `None` when the text is not a recognized shape, so callers can fall
    /// back to the raw text.
    #[must_use]
    pub fn parse(text: &str) -> Option<StructuredPersona> {
        Self::parse_with_format(text).map(|(_, persona)| persona)
    }

    #[must_use]
    pub fn parse_with_format(text: &str) -> Option<(PersonaFormat, StructuredPersona)> {
        if text.trim().is_empty() {
            return None;
        }
        if let Some(persona) = parse_wpp(text) {
            return Some((PersonaFormat::Wpp, persona));
        }
        parse_mapping(text)
    }
}

/// Attribute values may repeat (`Personality("A", "B")`), span lines, use
/// either quote character, and contain backslash escapes. Any deviation from
/// the shape (missing braces, unquoted values, trailing text, no attributes)
/// returns `None` so the text falls back to raw.
fn parse_wpp(text: &str) -> Option<StructuredPersona> {
    let mut rest = text.trim_start();
    rest = rest.strip_prefix('[')?;
    rest = rest.trim_start();
    rest = strip_prefix_ascii_case_insensitive(rest, "character")?;
    rest = rest.trim_start();
    rest = rest.strip_prefix('(')?;
    rest = rest.trim_start();
    let (_, after) = parse_quoted(rest)?;
    rest = after.trim_start();
    rest = rest.strip_prefix(')')?;
    rest = rest.trim_start();
    rest = rest.strip_prefix('{')?;

    let mut persona = StructuredPersona::default();
    let mut attribute_count = 0usize;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('}') {
            rest = after.trim_start();
            rest = rest.strip_prefix(']')?;
            if !rest.trim().is_empty() {
                return None;
            }
            break;
        }
        let (key, value, after) = parse_wpp_attribute(rest)?;
        rest = after.trim_start();
        rest = rest.strip_prefix(';').unwrap_or(rest);
        if set_attribute(&mut persona, &key, &value) {
            attribute_count = attribute_count.saturating_add(1);
        }
    }
    if attribute_count == 0 {
        return None;
    }
    Some(persona)
}

fn strip_prefix_ascii_case_insensitive<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let candidate = s.get(..prefix.len())?;
    candidate
        .eq_ignore_ascii_case(prefix)
        .then_some(&s[prefix.len()..])
}

fn parse_wpp_attribute(s: &str) -> Option<(String, String, &str)> {
    let mut key = String::new();
    for c in s.chars() {
        if is_wpp_key_char(c) {
            key.push(c);
        } else {
            break;
        }
    }
    if key.is_empty() {
        return None;
    }
    // Key characters are ASCII, so the byte length equals the character count.
    let rest = s.get(key.len()..)?.trim_start();
    let rest = rest.strip_prefix('(')?;
    let (values, rest) = parse_wpp_values(rest)?;
    let value = values.join(", ");
    Some((key.trim().to_string(), value, rest))
}

const fn is_wpp_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '\'')
}

fn parse_wpp_values(s: &str) -> Option<(Vec<String>, &str)> {
    let mut rest = s;
    let mut values = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(')') {
            return Some((values, after));
        }
        let (value, after) = parse_quoted(rest)?;
        values.push(value);
        rest = after.trim_start();
        rest = match rest.strip_prefix(',') {
            Some(after) => after,
            None if rest.starts_with(')') => rest,
            None => return None,
        };
    }
}

fn parse_quoted(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    let (_, open) = chars.next()?;
    if open != '"' && open != '\'' {
        return None;
    }
    let mut content = String::new();
    let mut escaped = false;
    for (idx, c) in chars {
        if escaped {
            content.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == open {
            return Some((content, &s[idx + c.len_utf8()..]));
        } else {
            content.push(c);
        }
    }
    None
}

/// Every line must be a recognized persona-key line or a continuation of the
/// previous entry's value (bullets and prose alike, so `Example messages:`
/// dialogue survives verbatim). A syntactically key-like line whose key is
/// outside the persona vocabulary continues the previous entry; without one it
/// marks the whole text as unrecognized. At least two key/bullet lines are
/// required so a lone `Name:` inside prose never triggers detection. YAML
/// block-scalar indicators (`|`, `|-`, `>`, ...) count as an empty value and
/// collect their following lines the same way.
fn parse_mapping(text: &str) -> Option<(PersonaFormat, StructuredPersona)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut pending = None;
    let mut mapping_lines = 0usize;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = parse_mapping_line(line) {
            let value = block_scalar_indicator(value);
            let normalized = normalize_key(key);
            if PERSONA_KEYS.contains(&normalized.as_str()) {
                let entry_index = entries.len();
                entries.push((key.to_string(), value.to_string()));
                pending = Some(entry_index);
                mapping_lines = mapping_lines.saturating_add(1);
                continue;
            }
        }
        let entry_index = pending?;
        let entry = entries.get_mut(entry_index)?;
        if !entry.1.is_empty() {
            entry.1.push('\n');
        }
        entry.1.push_str(line);
        if line.starts_with('-') {
            mapping_lines = mapping_lines.saturating_add(1);
        }
    }

    if mapping_lines < 2 {
        return None;
    }
    let format = if entries
        .iter()
        .all(|(key, _)| ALICHAT_KEYS.contains(&normalize_key(key).as_str()))
    {
        PersonaFormat::AliChat
    } else {
        PersonaFormat::Yaml
    };
    Some((format, build_persona(entries)))
}

fn parse_mapping_line(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    let mut chars = key.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '\'')) {
        return None;
    }
    Some((key, unquote(value)))
}

/// YAML block-scalar indicators (`|`, `|-`, `|2-`, `>+`, ...) carry no inline
/// content; their value comes from the following lines, which the mapping
/// continuation logic already collects. An empty result keeps the marker out
/// of the persona text.
fn block_scalar_indicator(value: &str) -> &str {
    let value = value.trim();
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return value;
    };
    if first != '|' && first != '>' {
        return value;
    }
    if chars.all(|c| c.is_ascii_digit() || matches!(c, '-' | '+')) {
        ""
    } else {
        value
    }
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return value;
    };
    let Some(last) = chars.next_back() else {
        return value;
    };
    if matches!((first, last), ('"', '"') | ('\'', '\'')) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Vocabulary of persona keys accepted in AliChat/YAML text. Every key line
/// must normalize to one of these; anything else makes the text unrecognized.
const PERSONA_KEYS: &[&str] = &[
    "age",
    "aliases",
    "appearance",
    "background",
    "behavior",
    "creator notes",
    "creator's notes",
    "description",
    "dislikes",
    "example dialogue",
    "example message",
    "example messages",
    "fears",
    "first message",
    "first messages",
    "gender",
    "goals",
    "hobbies",
    "home",
    "likes",
    "lorebook",
    "mind",
    "name",
    "notes",
    "occupation",
    "persona",
    "personality",
    "pronouns",
    "relationships",
    "scenario",
    "species",
    "speech",
    "speech pattern",
    "world",
];

/// Standard `AliChat` key set; text whose every key falls in this set is
/// reported as [`PersonaFormat::AliChat`], otherwise [`PersonaFormat::Yaml`].
const ALICHAT_KEYS: &[&str] = &[
    "age",
    "description",
    "example message",
    "example messages",
    "first message",
    "gender",
    "name",
    "personality",
    "scenario",
    "world",
];

fn build_persona(entries: Vec<(String, String)>) -> StructuredPersona {
    let mut persona = StructuredPersona::default();
    for (key, value) in entries {
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        set_attribute(&mut persona, &key, &value);
    }
    persona
}

fn set_attribute(persona: &mut StructuredPersona, key: &str, value: &str) -> bool {
    let value = value.trim().to_string();
    if value.is_empty() {
        return false;
    }
    match canonical_field(key) {
        Some(Field::Appearance) => persona.appearance = Some(value),
        Some(Field::Personality) => persona.personality = Some(value),
        Some(Field::Mind) => persona.mind = Some(value),
        Some(Field::Speech) => persona.speech = Some(value),
        Some(Field::Background) => persona.background = Some(value),
        Some(Field::Description) => persona.description = Some(value),
        None => persona.extra.push((key.trim().to_string(), value)),
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Appearance,
    Personality,
    Mind,
    Speech,
    Background,
    Description,
}

fn canonical_field(key: &str) -> Option<Field> {
    match normalize_key(key).as_str() {
        "appearance" => Some(Field::Appearance),
        "personality" => Some(Field::Personality),
        "mind" => Some(Field::Mind),
        "speech" | "speech pattern" => Some(Field::Speech),
        "background" => Some(Field::Background),
        "description" => Some(Field::Description),
        _ => None,
    }
}

/// Lowercase a key, collapsing runs of separators (` `, `_`, `-`) to one
/// space; apostrophes are kept so `Creator's Notes` stays one token.
fn normalize_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut pending_space = false;
    for c in key.chars() {
        if c.is_ascii_alphanumeric() || c == '\'' {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wpp_fixture() -> &'static str {
        r#"[character("Mira")
{
Age("23")
Appearance("Tall", "Silver hair")
Personality("Curious", "Warm")
Mind("Analytical", "Occasionally dreamy")
Speech("Soft-spoken", "Pauses often")
Background("A lighthouse keeper who lives alone")
}]"#
    }

    fn alichat_fixture() -> &'static str {
        r"Name: Mira
Age: 23
Gender: Female
Personality:
- Curious
- Warm
Description:
A lighthouse keeper with silver hair who lives alone.
Scenario:
A stormy night on the cliffs.
"
    }

    fn yaml_fixture() -> &'static str {
        r#"appearance: "Tall, silver hair"
personality: Curious and warm
mind: Analytical
speech_pattern: Soft-spoken
background: Lighthouse keeper
species: Half-elf
"#
    }

    #[test]
    fn wpp_fixture_extracts_attributes() {
        let (format, persona) =
            PersonaFormatParser::parse_with_format(wpp_fixture()).expect("parse W++ fixture");
        assert_eq!(format, PersonaFormat::Wpp);
        assert_eq!(persona.appearance.as_deref(), Some("Tall, Silver hair"));
        assert_eq!(persona.personality.as_deref(), Some("Curious, Warm"));
        assert_eq!(
            persona.mind.as_deref(),
            Some("Analytical, Occasionally dreamy")
        );
        assert_eq!(persona.speech.as_deref(), Some("Soft-spoken, Pauses often"));
        assert_eq!(
            persona.background.as_deref(),
            Some("A lighthouse keeper who lives alone")
        );
        assert_eq!(persona.extra, vec![("Age".to_string(), "23".to_string())]);
    }

    #[test]
    fn wpp_variants_parse() {
        let semicolon = r#"[character("A"){Age("25");Personality("Kind")}]"#;
        let persona = PersonaFormatParser::parse(semicolon).expect("parse semicolon W++");
        assert_eq!(persona.personality.as_deref(), Some("Kind"));
        assert_eq!(persona.extra, vec![("Age".to_string(), "25".to_string())]);

        let spaced = r#"[ Character('B') { Personality("Quiet") } ]"#;
        let persona = PersonaFormatParser::parse(spaced).expect("parse spaced W++");
        assert_eq!(persona.personality.as_deref(), Some("Quiet"));

        let multiline = "[character(\"C\")\n{\nPersonality(\"Loud\")\n}]";
        let persona = PersonaFormatParser::parse(multiline).expect("parse multiline W++");
        assert_eq!(persona.personality.as_deref(), Some("Loud"));
    }

    #[test]
    fn wpp_escaped_quotes_and_parentheses_stay_in_values() {
        let text = r#"[character("A"){Personality("Says \"hi\" (often)")}]"#;
        let persona = PersonaFormatParser::parse(text).expect("parse escaped W++ value");
        assert_eq!(persona.personality.as_deref(), Some("Says \"hi\" (often)"));
    }

    #[test]
    fn wpp_malformed_or_attribute_less_falls_back() {
        for text in [
            r#"[character("A"){Personality("unclosed]"#,
            "[character(\"A\"){}]",
            "character(\"A\"){Personality(\"X\")}",
            "[character(A){Personality(\"X\")}]",
            r#"[character("A"){Personality("X")} ] extra"#,
            r#"[character("A"){Personality("X")]"#,
            r#"[character("A"){Personality("X")}]]"#,
            r#"[character("A"){Personality X}]"#,
            r#"[character("A"){Personality("A" "B")}]"#,
        ] {
            assert_eq!(
                PersonaFormatParser::parse(text),
                None,
                "must fall back to raw: {text}"
            );
        }
    }

    #[test]
    fn wpp_empty_values_count_as_no_attributes() {
        for text in [
            r#"[character("A"){Personality("")}]"#,
            r#"[character("A"){Age("")Personality("  ")}]"#,
        ] {
            assert_eq!(
                PersonaFormatParser::parse(text),
                None,
                "an all-empty W++ block must fall back to raw: {text}"
            );
        }
    }

    #[test]
    fn alichat_fixture_extracts_attributes_and_bullets() {
        let (format, persona) =
            PersonaFormatParser::parse_with_format(alichat_fixture()).expect("parse AliChat");
        assert_eq!(format, PersonaFormat::AliChat);
        assert_eq!(persona.personality.as_deref(), Some("- Curious\n- Warm"));
        assert_eq!(
            persona.description.as_deref(),
            Some("A lighthouse keeper with silver hair who lives alone.")
        );
        let extras: Vec<(&str, &str)> = persona
            .extra
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            extras,
            vec![
                ("Name", "Mira"),
                ("Age", "23"),
                ("Gender", "Female"),
                ("Scenario", "A stormy night on the cliffs."),
            ]
        );
    }

    #[test]
    fn alichat_inline_values_and_multiline_prose_parse() {
        let text = "Name: Mira\nPersonality: Confident and dry\nDescription: She keeps a journal.\n  The journal is full of sea charts.";
        let (format, persona) =
            PersonaFormatParser::parse_with_format(text).expect("parse AliChat prose");
        assert_eq!(format, PersonaFormat::AliChat);
        assert_eq!(persona.personality.as_deref(), Some("Confident and dry"));
        assert_eq!(
            persona.description.as_deref(),
            Some("She keeps a journal.\nThe journal is full of sea charts.")
        );
    }

    #[test]
    fn yaml_fixture_extracts_attributes_and_unquotes() {
        let (format, persona) =
            PersonaFormatParser::parse_with_format(yaml_fixture()).expect("parse YAML");
        assert_eq!(format, PersonaFormat::Yaml);
        assert_eq!(persona.appearance.as_deref(), Some("Tall, silver hair"));
        assert_eq!(persona.personality.as_deref(), Some("Curious and warm"));
        assert_eq!(persona.mind.as_deref(), Some("Analytical"));
        assert_eq!(persona.speech.as_deref(), Some("Soft-spoken"));
        assert_eq!(persona.background.as_deref(), Some("Lighthouse keeper"));
        assert_eq!(
            persona.extra,
            vec![("species".to_string(), "Half-elf".to_string())]
        );
    }

    #[test]
    fn mapping_example_messages_dialogue_is_preserved() {
        let text = "Name: Mira\nExample messages:\n{{user}}: Are you awake?\n{{char}}: Always.";
        let persona = PersonaFormatParser::parse(text).expect("parse AliChat dialogue");
        assert_eq!(
            persona
                .extra
                .iter()
                .find(|(key, _)| key == "Example messages")
                .map(|(_, value)| value.as_str()),
            Some("{{user}}: Are you awake?\n{{char}}: Always.")
        );
    }

    #[test]
    fn yaml_block_scalar_indicators_do_not_leak() {
        let text =
            "name: Mira\ndescription: |-\n  She keeps a lighthouse.\n  It has stood for decades.";
        let persona = PersonaFormatParser::parse(text).expect("parse block scalar");
        assert_eq!(
            persona.description.as_deref(),
            Some("She keeps a lighthouse.\nIt has stood for decades.")
        );
        let folded = "name: Mira\ndescription: >+\n  She keeps a lighthouse.";
        let persona = PersonaFormatParser::parse(folded).expect("parse folded block scalar");
        assert_eq!(
            persona.description.as_deref(),
            Some("She keeps a lighthouse.")
        );
    }

    #[test]
    fn name_attribute_is_kept_in_full_render_but_not_head_lines() {
        let persona = PersonaFormatParser::parse(alichat_fixture()).expect("parse AliChat");
        assert!(persona.render_lines().contains(&"Name: Mira".to_string()));
        assert!(
            !persona
                .render_lines_excluding_core()
                .contains(&"Name: Mira".to_string())
        );
    }

    #[test]
    fn render_lines_are_dense_and_core_is_excluded() {
        let persona = PersonaFormatParser::parse(wpp_fixture()).expect("parse W++ fixture");
        let lines = persona.render_lines();
        assert!(lines.contains(&"Appearance: Tall, Silver hair".to_string()));
        assert!(lines.contains(&"Speech pattern: Soft-spoken, Pauses often".to_string()));
        assert!(lines.contains(&"Age: 23".to_string()));
        assert!(
            !lines.iter().any(|line| line.contains('"')),
            "dense lines must not retain W++ quoting: {lines:?}"
        );

        let core_lines = persona.render_lines_excluding_core();
        assert!(!core_lines.contains(&"Personality: Curious, Warm".to_string()));
        assert!(!core_lines.contains(&"Description: ".to_string()));
    }

    #[test]
    fn description_fallback_is_excluded_when_consumed_as_core() {
        let persona = StructuredPersona {
            description: Some("Long prose description".to_string()),
            ..StructuredPersona::default()
        };
        assert_eq!(
            persona.render_lines_excluding_core(),
            Vec::<String>::new(),
            "the attribute consumed as core personality must not repeat"
        );
    }

    #[test]
    fn unrecognized_shapes_fall_back() {
        for text in [
            "",
            "   ",
            "Friendly and cheerful.",
            "Name: Mira",
            "Name: Mira\nLocation: Tokyo",
            "Alice: Hello\nBob: Hi",
            "Once upon a time:\nName: Bob\nAge: 25",
            "She said: hello.\nName: Mira",
            "Age: 25\n",
            "name: Mira\nlocation: Tokyo",
        ] {
            assert_eq!(
                PersonaFormatParser::parse(text),
                None,
                "must fall back to raw: {text:?}"
            );
        }
    }

    #[test]
    fn keys_normalize_separators_and_keep_apostrophes() {
        assert_eq!(normalize_key("Speech_Pattern"), "speech pattern");
        assert_eq!(normalize_key("speech-pattern"), "speech pattern");
        assert_eq!(normalize_key("Creator's Notes"), "creator's notes");
        assert_eq!(normalize_key("first message"), "first message");
    }
}
