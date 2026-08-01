//! JSON Schema → GBNF conversion (100% Pure Rust implementation).

use ene_ai::error::LlmProviderError;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

const SPACE_RULE: &str = "| \" \" | \"\\n\"{1,2} [ \\t]{0,20}";

struct BuiltinRule {
    content: &'static str,
    deps: &'static [&'static str],
}

static PRIMITIVE_RULES: LazyLock<HashMap<&'static str, BuiltinRule>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "boolean",
        BuiltinRule {
            content: "(\"true\" | \"false\") space",
            deps: &[],
        },
    );
    m.insert(
        "decimal-part",
        BuiltinRule {
            content: "[0-9]{1,16}",
            deps: &[],
        },
    );
    m.insert(
        "integral-part",
        BuiltinRule {
            content: "[0] | [1-9] [0-9]{0,15}",
            deps: &[],
        },
    );
    m.insert(
        "number",
        BuiltinRule {
            content: "(\"-\"? integral-part) (\".\" decimal-part)? ([eE] [-+]? integral-part)? space",
            deps: &["integral-part", "decimal-part"],
        },
    );
    m.insert(
        "integer",
        BuiltinRule {
            content: "(\"-\"? integral-part) space",
            deps: &["integral-part"],
        },
    );
    m.insert(
        "value",
        BuiltinRule {
            content: "object | array | string | number | boolean | null",
            deps: &["object", "array", "string", "number", "boolean", "null"],
        },
    );
    m.insert(
        "object",
        BuiltinRule {
            content:
                "\"{\" space ( string \":\" space value (\",\" space string \":\" space value)* )? \"}\" space",
            deps: &["string", "value"],
        },
    );
    m.insert(
        "array",
        BuiltinRule {
            content: "\"[\" space ( value (\",\" space value)* )? \"]\" space",
            deps: &["value"],
        },
    );
    m.insert(
        "uuid",
        BuiltinRule {
            content:
                "\"\\\"\" [0-9a-fA-F]{8} \"-\" [0-9a-fA-F]{4} \"-\" [0-9a-fA-F]{4} \"-\" [0-9a-fA-F]{4} \"-\" [0-9a-fA-F]{12} \"\\\"\" space",
            deps: &[],
        },
    );
    m.insert(
        "char",
        BuiltinRule {
            content: "[^\"\\\\\\x7F\\x00-\\x1F] | [\\\\] ([\"\\\\bfnrt] | \"u\" [0-9a-fA-F]{4})",
            deps: &[],
        },
    );
    m.insert(
        "string",
        BuiltinRule {
            content: "\"\\\"\" char* \"\\\"\" space",
            deps: &["char"],
        },
    );
    m.insert(
        "null",
        BuiltinRule {
            content: "\"null\" space",
            deps: &[],
        },
    );
    m
});

static STRING_FORMAT_RULES: LazyLock<HashMap<&'static str, BuiltinRule>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "date",
        BuiltinRule {
            content:
                "[0-9]{4} \"-\" ( \"0\" [1-9] | \"1\" [0-2] ) \"-\" ( \"0\" [1-9] | [1-2] [0-9] | \"3\" [0-1] )",
            deps: &[],
        },
    );
    m.insert(
        "time",
        BuiltinRule {
            content:
                "([01] [0-9] | \"2\" [0-3]) \":\" [0-5] [0-9] \":\" [0-5] [0-9] ( \".\" [0-9]{3} )? ( \"Z\" | ( \"+\" | \"-\" ) ( [01] [0-9] | \"2\" [0-3] ) \":\" [0-5] [0-9] )",
            deps: &[],
        },
    );
    m.insert(
        "date-time",
        BuiltinRule {
            content: "date \"T\" time",
            deps: &["date", "time"],
        },
    );
    m.insert(
        "date-string",
        BuiltinRule {
            content: "\"\\\"\" date \"\\\"\" space",
            deps: &["date"],
        },
    );
    m.insert(
        "time-string",
        BuiltinRule {
            content: "\"\\\"\" time \"\\\"\" space",
            deps: &["time"],
        },
    );
    m.insert(
        "date-time-string",
        BuiltinRule {
            content: "\"\\\"\" date-time \"\\\"\" space",
            deps: &["date-time"],
        },
    );
    m
});

static RESERVED_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("root");
    for k in PRIMITIVE_RULES.keys() {
        s.insert(*k);
    }
    for k in STRING_FORMAT_RULES.keys() {
        s.insert(*k);
    }
    s
});

static INVALID_RULE_CHARS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[^a-zA-Z0-9-]+").unwrap_or_else(|_| unreachable!()));

fn is_reserved_name(name: &str) -> bool {
    RESERVED_NAMES.contains(name)
}

fn string_repeat(s: &str, count: usize) -> String {
    s.repeat(count)
}

fn build_repetition(
    item_rule: &str,
    min_items: usize,
    max_items: Option<usize>,
    separator_rule: &str,
) -> String {
    let has_max = max_items.is_some();
    let max_val = max_items.unwrap_or(usize::MAX);

    if max_val == 0 {
        return String::new();
    }
    if min_items == 0 && max_items == Some(1) {
        return format!("{item_rule}?");
    }

    if separator_rule.is_empty() {
        if min_items == 1 && !has_max {
            return format!("{item_rule}+");
        }
        if min_items == 0 && !has_max {
            return format!("{item_rule}*");
        }
        let max_str = if let Some(m) = max_items {
            m.to_string()
        } else {
            String::new()
        };
        return format!("{item_rule}{{{min_items},{max_str}}}");
    }

    let next_min = if min_items == 0 { 0 } else { min_items - 1 };
    let next_max = max_items.map(|m| m.saturating_sub(1));
    let sub = build_repetition(
        &format!("({separator_rule} {item_rule})"),
        next_min,
        next_max,
        "",
    );

    let mut result = format!("{item_rule} {sub}");
    if min_items == 0 {
        result = format!("({result})?");
    }
    result
}

#[expect(
    clippy::only_used_in_recursion,
    reason = "recursively passed for decimal precision tracking"
)]
fn build_min_max_int(
    min_value: Option<i64>,
    max_value: Option<i64>,
    out: &mut String,
    decimals_left: usize,
    top_level: bool,
) {
    let _has_min = min_value.is_some();
    let _has_max = max_value.is_some();

    let digit_range = |out: &mut String, from: char, to: char| {
        out.push('[');
        if from == to {
            out.push(from);
        } else {
            out.push(from);
            out.push('-');
            out.push(to);
        }
        out.push(']');
    };

    let more_digits = |out: &mut String, min_digits: usize, max_digits: usize| {
        out.push_str("[0-9]");
        if min_digits == max_digits && min_digits == 1 {
            return;
        }
        out.push('{');
        out.push_str(&min_digits.to_string());
        if max_digits != min_digits {
            out.push(',');
            if max_digits != usize::MAX {
                out.push_str(&max_digits.to_string());
            }
        }
        out.push('}');
    };

    fn uniform_range(
        out: &mut String,
        from: &str,
        to: &str,
        decimals_left: usize,
        digit_range: &dyn Fn(&mut String, char, char),
        more_digits: &dyn Fn(&mut String, usize, usize),
    ) {
        let from_chars: Vec<char> = from.chars().collect();
        let to_chars: Vec<char> = to.chars().collect();
        let mut i = 0;
        while i < from_chars.len() && i < to_chars.len() && from_chars[i] == to_chars[i] {
            i += 1;
        }
        if i > 0 {
            let prefix: String = from_chars[..i].iter().collect();
            out.push('"');
            out.push_str(&prefix);
            out.push('"');
        }
        if i < from_chars.len() && i < to_chars.len() {
            if i > 0 {
                out.push(' ');
            }
            let sub_len = from_chars.len() - i - 1;
            if sub_len > 0 {
                let from_sub: String = from_chars[i + 1..].iter().collect();
                let to_sub: String = to_chars[i + 1..].iter().collect();
                let sub_zeros = string_repeat("0", sub_len);
                let sub_nines = string_repeat("9", sub_len);

                let mut to_reached = false;
                out.push('(');
                if from_sub == sub_zeros {
                    if (from_chars[i] as u8) <= (to_chars[i] as u8).saturating_sub(1) {
                        let to_c = (to_chars[i] as u8 - 1) as char;
                        digit_range(out, from_chars[i], to_c);
                    }
                    out.push(' ');
                    more_digits(out, sub_len, sub_len);
                } else {
                    out.push('[');
                    out.push(from_chars[i]);
                    out.push_str("] (");
                    uniform_range(
                        out,
                        &from_sub,
                        &sub_nines,
                        decimals_left,
                        digit_range,
                        more_digits,
                    );
                    out.push(')');
                    if (from_chars[i] as u8) < (to_chars[i] as u8).saturating_sub(1) {
                        out.push_str(" | ");
                        if to_sub == sub_nines {
                            let from_c = (from_chars[i] as u8 + 1) as char;
                            digit_range(out, from_c, to_chars[i]);
                            to_reached = true;
                        } else {
                            let from_c = (from_chars[i] as u8 + 1) as char;
                            let to_c = (to_chars[i] as u8 - 1) as char;
                            digit_range(out, from_c, to_c);
                        }
                        out.push(' ');
                        more_digits(out, sub_len, sub_len);
                    }
                }
                if !to_reached {
                    out.push_str(" | ");
                    digit_range(out, to_chars[i], to_chars[i]);
                    out.push(' ');
                    uniform_range(
                        out,
                        &sub_zeros,
                        &to_sub,
                        decimals_left,
                        digit_range,
                        more_digits,
                    );
                }
                out.push(')');
            } else {
                digit_range(out, from_chars[i], to_chars[i]);
            }
        }
    }

    if let (Some(min_v), Some(max_v)) = (min_value, max_value) {
        if min_v < 0 && max_v < 0 {
            out.push_str("\"-\" (");
            build_min_max_int(
                Some(-max_v),
                Some(-min_v),
                out,
                decimals_left,
                /* top_level= */ true,
            );
            out.push(')');
            return;
        }

        let mut current_min = min_v;
        if current_min < 0 {
            out.push_str("\"-\" (");
            build_min_max_int(
                Some(0),
                Some(-current_min),
                out,
                decimals_left,
                /* top_level= */ true,
            );
            out.push_str(") | ");
            current_min = 0;
        }

        let mut min_s = current_min.to_string();
        let max_s = max_v.to_string();
        let min_digits = min_s.len();
        let max_digits = max_s.len();

        for digits in min_digits..max_digits {
            uniform_range(
                out,
                &min_s,
                &string_repeat("9", digits),
                decimals_left,
                &digit_range,
                &more_digits,
            );
            min_s = format!("1{}", string_repeat("0", digits));
            out.push_str(" | ");
        }
        uniform_range(
            out,
            &min_s,
            &max_s,
            decimals_left,
            &digit_range,
            &more_digits,
        );
        return;
    }

    let less_decimals = decimals_left.saturating_sub(1).max(1);

    if let Some(min_v) = min_value {
        if min_v < 0 {
            out.push_str("\"-\" (");
            build_min_max_int(
                Some(i64::MIN),
                Some(-min_v),
                out,
                decimals_left,
                /* top_level= */ false,
            );
            out.push_str(") | [0] | [1-9] ");
            more_digits(out, 0, decimals_left.saturating_sub(1));
        } else if min_v == 0 {
            if top_level {
                out.push_str("[0] | [1-9] ");
                more_digits(out, 0, less_decimals);
            } else {
                more_digits(out, 1, decimals_left);
            }
        } else if min_v <= 9 {
            let c = (b'0' + min_v as u8) as char;
            let range_start = if top_level { '1' } else { '0' };
            if c > range_start {
                digit_range(out, range_start, (c as u8 - 1) as char);
                out.push(' ');
                more_digits(out, 1, less_decimals);
                out.push_str(" | ");
            }
            digit_range(out, c, '9');
            out.push(' ');
            more_digits(out, 0, less_decimals);
        } else {
            let min_s = min_v.to_string();
            let len = min_s.len();
            if let Some(c) = min_s.chars().next() {
                let top_start = if top_level { '1' } else { '0' };
                if c > top_start {
                    digit_range(out, top_start, (c as u8 - 1) as char);
                    out.push(' ');
                    more_digits(out, len, less_decimals);
                    out.push_str(" | ");
                }
                digit_range(out, c, c);
                out.push_str(" (");
                let sub_val = min_s[1..].parse::<i64>().unwrap_or(0);
                build_min_max_int(
                    Some(sub_val),
                    Some(i64::MAX),
                    out,
                    less_decimals,
                    /* top_level= */ false,
                );
                out.push(')');
                if c < '9' {
                    out.push_str(" | ");
                    digit_range(out, (c as u8 + 1) as char, '9');
                    out.push(' ');
                    more_digits(out, len.saturating_sub(1), less_decimals);
                }
            }
        }
        return;
    }

    if let Some(max_v) = max_value {
        if max_v >= 0 {
            if top_level {
                out.push_str("\"-\" [1-9] ");
                more_digits(out, 0, less_decimals);
                out.push_str(" | ");
            }
            build_min_max_int(
                Some(0),
                Some(max_v),
                out,
                decimals_left,
                /* top_level= */ true,
            );
        } else {
            out.push_str("\"-\" (");
            build_min_max_int(
                Some(-max_v),
                Some(i64::MAX),
                out,
                decimals_left,
                /* top_level= */ false,
            );
            out.push(')');
        }
    }
}

fn format_literal(literal: &str) -> String {
    let mut escaped = String::new();
    for c in literal.chars() {
        match c {
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(c),
        }
    }
    format!("\"{escaped}\"")
}

fn generate_constant_rule(value: &Value) -> String {
    format_literal(&value.to_string())
}

#[derive(Debug, thiserror::Error)]
enum GrammarError {
    #[error("schema parse error: {0}")]
    Parse(String),
}

struct SchemaConverter {
    rules: BTreeMap<String, String>,
    refs: HashMap<String, Value>,
    refs_being_resolved: HashSet<String>,
    errors: Vec<String>,
    warnings: Vec<String>,
    dotall: bool,
}

impl SchemaConverter {
    fn new(dotall: bool) -> Self {
        let mut rules = BTreeMap::new();
        rules.insert("space".to_string(), SPACE_RULE.to_string());
        Self {
            rules,
            refs: HashMap::new(),
            refs_being_resolved: HashSet::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            dotall,
        }
    }

    fn add_rule(&mut self, name: &str, rule: &str) -> String {
        let esc_name = INVALID_RULE_CHARS_RE.replace_all(name, "-").to_string();
        if !self.rules.contains_key(&esc_name)
            || self.rules.get(&esc_name).map(String::as_str) == Some(rule)
        {
            self.rules.insert(esc_name.clone(), rule.to_string());
            return esc_name;
        }
        let mut i = 0;
        loop {
            let key = format!("{esc_name}{i}");
            if !self.rules.contains_key(&key)
                || self.rules.get(&key).map(String::as_str) == Some(rule)
            {
                self.rules.insert(key.clone(), rule.to_string());
                return key;
            }
            i += 1;
        }
    }

    fn add_primitive(&mut self, name: &str, rule: &BuiltinRule) -> String {
        let n = self.add_rule(name, rule.content);
        for dep in rule.deps {
            if let Some(dep_rule) = PRIMITIVE_RULES.get(dep) {
                if !self.rules.contains_key(*dep) {
                    let dep_rule_clone = BuiltinRule {
                        content: dep_rule.content,
                        deps: dep_rule.deps,
                    };
                    self.add_primitive(dep, &dep_rule_clone);
                }
            } else if let Some(dep_rule) = STRING_FORMAT_RULES.get(dep) {
                if !self.rules.contains_key(*dep) {
                    let dep_rule_clone = BuiltinRule {
                        content: dep_rule.content,
                        deps: dep_rule.deps,
                    };
                    self.add_primitive(dep, &dep_rule_clone);
                }
            } else {
                self.errors.push(format!("Rule {dep} not known"));
            }
        }
        n
    }

    fn add_named_primitive(&mut self, name: &str, prim_key: &str) -> String {
        if let Some(rule_def) = PRIMITIVE_RULES.get(prim_key) {
            let rule = BuiltinRule {
                content: rule_def.content,
                deps: rule_def.deps,
            };
            self.add_primitive(name, &rule)
        } else {
            self.errors
                .push(format!("Primitive rule {prim_key} not known"));
            String::new()
        }
    }

    fn generate_union_rule(&mut self, name: &str, alt_schemas: &[Value]) -> String {
        let mut rules = Vec::with_capacity(alt_schemas.len());
        for (i, schema) in alt_schemas.iter().enumerate() {
            let prefix = if name.is_empty() { "alternative-" } else { "-" };
            let sub_name = format!("{name}{prefix}{i}");
            rules.push(self.visit(schema, &sub_name));
        }
        rules.join(" | ")
    }

    fn visit_pattern(&mut self, pattern: &str, name: &str) -> String {
        if !pattern.starts_with('^') || !pattern.ends_with('$') {
            self.errors
                .push("Pattern must start with '^' and end with '$'".to_string());
            return String::new();
        }
        let sub_pattern = &pattern[1..pattern.len() - 1];
        let mut sub_rule_ids: HashMap<String, String> = HashMap::new();

        let chars: Vec<char> = sub_pattern.chars().collect();
        let mut i = 0;
        let length = chars.len();

        #[derive(Clone)]
        struct LiteralOrRule {
            s: String,
            is_literal: bool,
        }

        fn to_rule(item: &LiteralOrRule) -> String {
            if item.is_literal {
                format!("\"{}\"", item.s)
            } else {
                item.s.clone()
            }
        }

        fn parse_pattern(
            chars: &[char],
            i: &mut usize,
            length: usize,
            converter: &mut SchemaConverter,
            sub_rule_ids: &mut HashMap<String, String>,
            name: &str,
        ) -> LiteralOrRule {
            let start = *i;
            let mut seq: Vec<LiteralOrRule> = Vec::new();

            let get_dot = |conv: &mut SchemaConverter| {
                let rule = if conv.dotall {
                    "[\\U00000000-\\U0010FFFF]"
                } else {
                    "[^\\x0A\\x0D]"
                };
                conv.add_rule("dot", rule)
            };

            let join_seq = |seq: Vec<LiteralOrRule>| -> LiteralOrRule {
                let mut ret: Vec<LiteralOrRule> = Vec::new();
                let mut literal = String::new();

                for item in seq {
                    if item.is_literal {
                        literal.push_str(&item.s);
                    } else {
                        if !literal.is_empty() {
                            ret.push(LiteralOrRule {
                                s: literal.clone(),
                                is_literal: true,
                            });
                            literal.clear();
                        }
                        ret.push(item);
                    }
                }
                if !literal.is_empty() {
                    ret.push(LiteralOrRule {
                        s: literal,
                        is_literal: true,
                    });
                }

                let results: Vec<String> = ret.iter().map(to_rule).collect();
                LiteralOrRule {
                    s: results.join(" "),
                    is_literal: false,
                }
            };

            while *i < length {
                let c = chars[*i];
                if c == '.' {
                    let dot_rule = get_dot(converter);
                    seq.push(LiteralOrRule {
                        s: dot_rule,
                        is_literal: false,
                    });
                    *i += 1;
                } else if c == '(' {
                    *i += 1;
                    if *i < length && chars[*i] == '?' {
                        if *i + 1 < length && chars[*i + 1] == ':' {
                            *i += 2;
                        } else {
                            converter
                                .warnings
                                .push("Unsupported pattern syntax".to_string());
                            let mut depth = 1;
                            while *i < length && depth > 0 {
                                if chars[*i] == '\\' && *i + 1 < length {
                                    *i += 2;
                                } else {
                                    if chars[*i] == '(' {
                                        depth += 1;
                                    } else if chars[*i] == ')' {
                                        depth -= 1;
                                    }
                                    *i += 1;
                                }
                            }
                            continue;
                        }
                    }
                    let inner = parse_pattern(chars, i, length, converter, sub_rule_ids, name);
                    seq.push(LiteralOrRule {
                        s: format!("({})", to_rule(&inner)),
                        is_literal: false,
                    });
                } else if c == ')' {
                    *i += 1;
                    if start > 0
                        && chars[start - 1] != '('
                        && (start < 2 || chars[start - 2] != '?' || chars[start - 1] != ':')
                    {
                        converter.errors.push("Unbalanced parentheses".to_string());
                    }
                    return join_seq(seq);
                } else if c == '[' {
                    let mut square = String::new();
                    square.push(c);
                    *i += 1;
                    while *i < length && chars[*i] != ']' {
                        if chars[*i] == '\\' && *i + 1 < length {
                            square.push(chars[*i]);
                            square.push(chars[*i + 1]);
                            *i += 2;
                        } else {
                            square.push(chars[*i]);
                            *i += 1;
                        }
                    }
                    if *i >= length {
                        converter
                            .errors
                            .push("Unbalanced square brackets".to_string());
                    }
                    square.push(']');
                    *i += 1;
                    seq.push(LiteralOrRule {
                        s: square,
                        is_literal: false,
                    });
                } else if c == '|' {
                    seq.push(LiteralOrRule {
                        s: "|".to_string(),
                        is_literal: false,
                    });
                    *i += 1;
                } else if c == '*' || c == '+' || c == '?' {
                    if let Some(last) = seq.last_mut() {
                        last.s = format!("{}{c}", to_rule(last));
                        last.is_literal = false;
                    }
                    *i += 1;
                } else if c == '{' {
                    let mut curly = String::new();
                    curly.push(c);
                    *i += 1;
                    while *i < length && chars[*i] != '}' {
                        curly.push(chars[*i]);
                        *i += 1;
                    }
                    if *i >= length {
                        converter
                            .errors
                            .push("Unbalanced curly brackets".to_string());
                    }
                    curly.push('}');
                    *i += 1;

                    let inner_curly = &curly[1..curly.len() - 1];
                    let nums: Vec<&str> = inner_curly.split(',').collect();
                    let mut min_times = 0usize;
                    let mut max_times: Option<usize> = None;

                    if nums.len() == 1 {
                        if let Ok(v) = nums[0].parse::<usize>() {
                            min_times = v;
                            max_times = Some(v);
                        } else {
                            converter
                                .errors
                                .push("Invalid number in curly brackets".to_string());
                        }
                    } else if nums.len() == 2 {
                        if !nums[0].is_empty() {
                            if let Ok(v) = nums[0].parse::<usize>() {
                                min_times = v;
                            } else {
                                converter
                                    .errors
                                    .push("Invalid number in curly brackets".to_string());
                            }
                        }
                        if !nums[1].is_empty() {
                            if let Ok(v) = nums[1].parse::<usize>() {
                                max_times = Some(v);
                            } else {
                                converter
                                    .errors
                                    .push("Invalid number in curly brackets".to_string());
                            }
                        }
                    } else {
                        converter
                            .errors
                            .push("Wrong number of values in curly brackets".to_string());
                    }

                    if let Some(last) = seq.last_mut() {
                        let sub_is_literal = last.is_literal;
                        let mut sub = last.s.clone();

                        if !sub_is_literal {
                            let sub_id = if let Some(id) = sub_rule_ids.get(&sub) {
                                id.clone()
                            } else {
                                let new_id = converter
                                    .add_rule(&format!("{name}-{}", sub_rule_ids.len()), &sub);
                                sub_rule_ids.insert(sub.clone(), new_id.clone());
                                new_id
                            };
                            sub = sub_id;
                        }
                        let item_target = if sub_is_literal {
                            format!("\"{sub}\"")
                        } else {
                            sub
                        };
                        last.s = build_repetition(&item_target, min_times, max_times, "");
                        last.is_literal = false;
                    }
                } else {
                    let mut literal = String::new();
                    let non_literals: HashSet<char> =
                        ['|', '.', '(', ')', '[', ']', '{', '}', '*', '+', '?']
                            .iter()
                            .copied()
                            .collect();
                    let escaped_in_regex: HashSet<char> = [
                        '^', '$', '.', '[', ']', '(', ')', '|', '{', '}', '*', '+', '?',
                    ]
                    .iter()
                    .copied()
                    .collect();

                    while *i < length {
                        if chars[*i] == '\\' && *i < length - 1 {
                            let next = chars[*i + 1];
                            if escaped_in_regex.contains(&next) {
                                *i += 1;
                                literal.push(chars[*i]);
                                *i += 1;
                            } else {
                                literal.push(chars[*i]);
                                literal.push(chars[*i + 1]);
                                *i += 2;
                            }
                        } else if chars[*i] == '"' {
                            literal.push_str("\\\"");
                            *i += 1;
                        } else if !non_literals.contains(&chars[*i])
                            && (*i == length - 1
                                || literal.is_empty()
                                || chars[*i + 1] == '.'
                                || !non_literals.contains(&chars[*i + 1]))
                        {
                            literal.push(chars[*i]);
                            *i += 1;
                        } else {
                            break;
                        }
                    }
                    if !literal.is_empty() {
                        seq.push(LiteralOrRule {
                            s: literal,
                            is_literal: true,
                        });
                    }
                }
            }
            join_seq(seq)
        }

        let result = parse_pattern(&chars, &mut i, length, self, &mut sub_rule_ids, name);
        let rule_content = format!("\"\\\"\" ({}) \"\\\"\" space", to_rule(&result));
        self.add_rule(name, &rule_content)
    }

    fn not_strings(&mut self, strings: &[String]) -> String {
        #[derive(Default)]
        struct TrieNode {
            children: BTreeMap<char, TrieNode>,
            is_end_of_string: bool,
        }
        impl TrieNode {
            fn new() -> Self {
                Self {
                    children: BTreeMap::new(),
                    is_end_of_string: false,
                }
            }
            fn insert(&mut self, s: &str) {
                let mut node = self;
                for c in s.chars() {
                    node = node.children.entry(c).or_default();
                }
                node.is_end_of_string = true;
            }
        }

        let mut trie = TrieNode::new();
        for s in strings {
            trie.insert(s);
        }

        let char_rule_name = self.add_named_primitive("char", "char");

        let mut out = String::new();
        out.push_str("[\"] ( ");

        fn visit_trie(node: &TrieNode, out: &mut String, char_rule_name: &str) {
            let mut rejects = String::new();
            let mut first = true;

            for (&c, child) in &node.children {
                rejects.push(c);
                if first {
                    first = false;
                } else {
                    out.push_str(" | ");
                }
                out.push('[');
                out.push(c);
                out.push(']');

                if !child.children.is_empty() {
                    out.push_str(" (");
                    visit_trie(child, out, char_rule_name);
                    out.push(')');
                } else if child.is_end_of_string {
                    out.push(' ');
                    out.push_str(char_rule_name);
                    out.push('+');
                }
            }

            if !node.children.is_empty() {
                if !first {
                    out.push_str(" | ");
                }
                out.push_str("[^\"");
                out.push_str(&rejects);
                out.push(']');
                out.push(' ');
                out.push_str(char_rule_name);
                out.push('*');
            }
        }

        visit_trie(&trie, &mut out, &char_rule_name);
        out.push_str(" )");
        if !trie.is_end_of_string {
            out.push('?');
        }
        out.push_str(" [\"] space");
        out
    }

    fn resolve_ref(&mut self, ref_str: &str) -> String {
        let ref_fragment = if let Some(idx) = ref_str.find('#') {
            &ref_str[idx + 1..]
        } else {
            ref_str
        };
        let ref_name_clean = INVALID_RULE_CHARS_RE
            .replace_all(ref_fragment, "-")
            .to_string();
        let ref_name = format!("ref{ref_name_clean}");

        if !self.rules.contains_key(&ref_name) && !self.refs_being_resolved.contains(ref_str) {
            self.refs_being_resolved.insert(ref_str.to_string());
            if let Some(resolved) = self.refs.get(ref_str).cloned() {
                let generated = self.visit(&resolved, &ref_name);
                self.refs_being_resolved.remove(ref_str);
                return generated;
            }
            self.refs_being_resolved.remove(ref_str);
        }
        ref_name
    }

    fn resolve_refs(&mut self, schema: &mut Value, url: &str) {
        fn visit_refs(n: &mut Value, schema: &Value, url: &str, conv: &mut SchemaConverter) {
            match n {
                Value::Array(arr) => {
                    for item in arr {
                        visit_refs(item, schema, url, conv);
                    }
                }
                Value::Object(obj) => {
                    if let Some(Value::String(ref_str)) = obj.get("$ref") {
                        let ref_val = ref_str.clone();
                        if !conv.refs.contains_key(&ref_val) {
                            if let Some(pointer) = ref_val.strip_prefix('#') {
                                let absolute_ref = format!("{url}{ref_val}");
                                obj.insert("$ref".to_string(), Value::String(absolute_ref.clone()));

                                let mut target = schema;
                                let tokens: Vec<&str> = pointer.split('/').collect();
                                let mut ok = true;
                                for token in tokens.iter().skip(1) {
                                    if let Some(next) = target.get(token) {
                                        target = next;
                                    } else if let Ok(idx) = token.parse::<usize>() {
                                        if let Some(next) = target.get(idx) {
                                            target = next;
                                        } else {
                                            conv.errors.push(format!(
                                                "Error resolving ref {ref_val}: {token} not found"
                                            ));
                                            ok = false;
                                            break;
                                        }
                                    } else {
                                        conv.errors.push(format!(
                                            "Error resolving ref {ref_val}: {token} not found"
                                        ));
                                        ok = false;
                                        break;
                                    }
                                }
                                if ok {
                                    conv.refs.insert(absolute_ref, target.clone());
                                }
                            } else {
                                conv.errors.push(format!("Unsupported ref: {ref_val}"));
                            }
                        }
                    } else {
                        for (_, v) in obj.iter_mut() {
                            visit_refs(v, schema, url, conv);
                        }
                    }
                }
                _ => {}
            }
        }

        let schema_clone = schema.clone();
        visit_refs(schema, &schema_clone, url, self);
    }

    fn build_object_rule(
        &mut self,
        properties: &[(String, Value)],
        required: &HashSet<String>,
        name: &str,
        additional_properties: &Value,
    ) -> String {
        let mut required_props = Vec::new();
        let mut optional_props = Vec::new();
        let mut prop_kv_rule_names = HashMap::new();
        let mut prop_names = Vec::new();

        for (prop_name, prop_schema) in properties {
            let prefix = if name.is_empty() { "" } else { "-" };
            let prop_rule_name = self.visit(prop_schema, &format!("{name}{prefix}{prop_name}"));

            let kv_content = format!(
                "{} space \":\" space {prop_rule_name}",
                format_literal(&Value::String(prop_name.clone()).to_string())
            );
            let kv_rule_name = self.add_rule(&format!("{name}{prefix}{prop_name}-kv"), &kv_content);
            prop_kv_rule_names.insert(prop_name.clone(), kv_rule_name);

            if required.contains(prop_name) {
                required_props.push(prop_name.clone());
            } else {
                optional_props.push(prop_name.clone());
            }
            prop_names.push(prop_name.clone());
        }

        let is_add_props =
            additional_properties.as_bool().unwrap_or(false) || additional_properties.is_object();
        if is_add_props {
            let prefix = if name.is_empty() { "" } else { "-" };
            let sub_name = format!("{name}{prefix}additional");

            let value_rule = if additional_properties.is_object() {
                self.visit(additional_properties, &format!("{sub_name}-value"))
            } else {
                self.add_named_primitive("value", "value")
            };

            let key_rule = if prop_names.is_empty() {
                self.add_named_primitive("string", "string")
            } else {
                let not_str = self.not_strings(&prop_names);
                self.add_rule(&format!("{sub_name}-k"), &not_str)
            };

            let kv_rule = self.add_rule(
                &format!("{sub_name}-kv"),
                &format!("{key_rule} \":\" space {value_rule}"),
            );
            prop_kv_rule_names.insert("*".to_string(), kv_rule);
            optional_props.push("*".to_string());
        }

        let mut rule = "\"{\" space ".to_string();
        for (i, req_p) in required_props.iter().enumerate() {
            if i > 0 {
                rule.push_str(" \",\" space ");
            }
            rule.push_str(&prop_kv_rule_names[req_p]);
        }

        if !optional_props.is_empty() {
            rule.push_str(" (");
            if !required_props.is_empty() {
                rule.push_str(" \",\" space ( ");
            }

            fn get_recursive_refs(
                conv: &mut SchemaConverter,
                ks: &[String],
                first_is_optional: bool,
                prop_kv_rule_names: &HashMap<String, String>,
                name: &str,
            ) -> String {
                if ks.is_empty() {
                    return String::new();
                }
                let k = &ks[0];
                let kv_rule_name = &prop_kv_rule_names[k];
                let comma_ref = format!("( \",\" space {kv_rule_name} )");
                let quantifier = if k == "*" { "*" } else { "?" };
                let repeat_quantifier = if k == "*" {
                    format!(" {comma_ref}*")
                } else {
                    String::new()
                };

                let mut res = if first_is_optional {
                    format!("{comma_ref}{quantifier}")
                } else {
                    format!("{kv_rule_name}{repeat_quantifier}")
                };

                if ks.len() > 1 {
                    let prefix = if name.is_empty() { "" } else { "-" };
                    let rest = get_recursive_refs(conv, &ks[1..], true, prop_kv_rule_names, name);
                    let rest_rule_name = conv.add_rule(&format!("{name}{prefix}{k}-rest"), &rest);
                    res.push(' ');
                    res.push_str(&rest_rule_name);
                }
                res
            }

            for (i, _) in optional_props.iter().enumerate() {
                if i > 0 {
                    rule.push_str(" | ");
                }
                let rec = get_recursive_refs(
                    self,
                    &optional_props[i..],
                    false,
                    &prop_kv_rule_names,
                    name,
                );
                rule.push_str(&rec);
            }
            if !required_props.is_empty() {
                rule.push_str(" )");
            }
            rule.push_str(" )?");
        }

        rule.push_str(" \"}\" space");
        rule
    }

    fn visit(&mut self, schema: &Value, name: &str) -> String {
        let schema_obj = schema.as_object();
        let schema_type = schema_obj.and_then(|o| o.get("type"));
        let schema_format = schema_obj
            .and_then(|o| o.get("format"))
            .and_then(Value::as_str)
            .unwrap_or("");

        let rule_name = if is_reserved_name(name) {
            format!("{name}-")
        } else if name.is_empty() {
            "root".to_string()
        } else {
            name.to_string()
        };

        if let Some(ref_val) = schema_obj
            .and_then(|o| o.get("$ref"))
            .and_then(Value::as_str)
        {
            let ref_rule = self.resolve_ref(ref_val);
            return self.add_rule(&rule_name, &ref_rule);
        }

        if let Some(alts) = schema_obj
            .and_then(|o| o.get("oneOf").or_else(|| o.get("anyOf")))
            .and_then(Value::as_array)
        {
            let union_rule = self.generate_union_rule(name, alts);
            return self.add_rule(&rule_name, &union_rule);
        }

        if let Some(types) = schema_type.and_then(Value::as_array) {
            let mut schema_types = Vec::new();
            for t in types {
                let mut schema_copy = schema.clone();
                if let Some(obj) = schema_copy.as_object_mut() {
                    obj.insert("type".to_string(), t.clone());
                }
                schema_types.push(schema_copy);
            }
            let union_rule = self.generate_union_rule(name, &schema_types);
            return self.add_rule(&rule_name, &union_rule);
        }

        if let Some(c) = schema_obj.and_then(|o| o.get("const")) {
            let const_rule = format!("{} space", generate_constant_rule(c));
            return self.add_rule(&rule_name, &const_rule);
        }

        if let Some(enums) = schema_obj
            .and_then(|o| o.get("enum"))
            .and_then(Value::as_array)
        {
            let enum_values: Vec<String> = enums.iter().map(generate_constant_rule).collect();
            let enum_rule = format!("({}) space", enum_values.join(" | "));
            return self.add_rule(&rule_name, &enum_rule);
        }

        let is_null_or_obj_type =
            schema_type.is_none() || schema_type.and_then(Value::as_str) == Some("object");

        if is_null_or_obj_type
            && (schema_obj.is_some_and(|o| o.contains_key("properties"))
                || schema_obj
                    .is_some_and(|o| o.get("additionalProperties").is_some_and(|ap| ap != true)))
        {
            let mut required = HashSet::new();
            if let Some(req_arr) = schema_obj
                .and_then(|o| o.get("required"))
                .and_then(Value::as_array)
            {
                for item in req_arr {
                    if let Some(s) = item.as_str() {
                        required.insert(s.to_string());
                    }
                }
            }
            let mut properties = Vec::new();
            if let Some(props_obj) = schema_obj
                .and_then(|o| o.get("properties"))
                .and_then(Value::as_object)
            {
                for (k, v) in props_obj {
                    properties.push((k.clone(), v.clone()));
                }
            }
            let add_props = schema_obj
                .and_then(|o| o.get("additionalProperties"))
                .cloned()
                .unwrap_or(Value::Null);

            let obj_rule = self.build_object_rule(&properties, &required, name, &add_props);
            return self.add_rule(&rule_name, &obj_rule);
        }

        if (schema_type.is_none()
            || schema_type.and_then(Value::as_str) == Some("object")
            || schema_type.and_then(Value::as_str) == Some("string"))
            && let Some(all_of) = schema_obj
                .and_then(|o| o.get("allOf"))
                .and_then(Value::as_array)
        {
            let mut required = HashSet::new();
            let mut properties = Vec::new();
            let mut enum_values: BTreeMap<String, usize> = BTreeMap::new();
            let hybrid_name = name.to_string();

            let mut add_component =
                |comp_schema: &Value, is_req: bool, conv: &mut SchemaConverter| {
                    if let Some(r) = comp_schema.get("$ref").and_then(Value::as_str)
                        && let Some(target) = conv.refs.get(r).cloned()
                        && let Some(props) = target.get("properties").and_then(Value::as_object)
                    {
                        for (k, v) in props {
                            properties.push((k.clone(), v.clone()));
                            if is_req {
                                required.insert(k.clone());
                            }
                        }
                    } else if let Some(props) =
                        comp_schema.get("properties").and_then(Value::as_object)
                    {
                        for (k, v) in props {
                            properties.push((k.clone(), v.clone()));
                            if is_req {
                                required.insert(k.clone());
                            }
                        }
                    } else if let Some(enums) = comp_schema.get("enum").and_then(Value::as_array) {
                        for v in enums {
                            let rule = generate_constant_rule(v);
                            *enum_values.entry(rule).or_insert(0) += 1;
                        }
                    }
                };

            for t in all_of {
                if let Some(any_of) = t.get("anyOf").and_then(Value::as_array) {
                    for tt in any_of {
                        add_component(tt, false, self);
                    }
                } else {
                    add_component(t, true, self);
                }
            }

            if !enum_values.is_empty() {
                let mut enum_intersection = Vec::new();
                for (k, count) in &enum_values {
                    if *count == all_of.len() {
                        enum_intersection.push(k.clone());
                    }
                }
                if !enum_intersection.is_empty() {
                    let rule_content = format!("({}) space", enum_intersection.join(" | "));
                    return self.add_rule(&rule_name, &rule_content);
                }
            }
            let obj_rule =
                self.build_object_rule(&properties, &required, &hybrid_name, &Value::Null);
            return self.add_rule(&rule_name, &obj_rule);
        }

        let is_null_or_arr_type =
            schema_type.is_none() || schema_type.and_then(Value::as_str) == Some("array");

        if is_null_or_arr_type
            && let Some(items) =
                schema_obj.and_then(|o| o.get("items").or_else(|| o.get("prefixItems")))
        {
            if let Some(items_arr) = items.as_array() {
                let mut rule = "\"[\" space ".to_string();
                for (i, item_schema) in items_arr.iter().enumerate() {
                    if i > 0 {
                        rule.push_str(" \",\" space ");
                    }
                    let prefix = if name.is_empty() { "" } else { "-" };
                    let sub_rule_name =
                        self.visit(item_schema, &format!("{name}{prefix}tuple-{i}"));
                    rule.push_str(&sub_rule_name);
                }
                rule.push_str(" \"]\" space");
                return self.add_rule(&rule_name, &rule);
            }

            let prefix = if name.is_empty() { "" } else { "-" };
            let item_rule_name = self.visit(items, &format!("{name}{prefix}item"));
            let min_items = schema_obj
                .and_then(|o| o.get("minItems"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let max_items = schema_obj
                .and_then(|o| o.get("maxItems"))
                .and_then(Value::as_u64)
                .map(|m| m as usize);

            let rep = build_repetition(&item_rule_name, min_items, max_items, "\",\" space");
            let rule_content = format!("\"[\" space {rep} \"]\" space");
            return self.add_rule(&rule_name, &rule_content);
        }

        let is_null_or_str_type =
            schema_type.is_none() || schema_type.and_then(Value::as_str) == Some("string");

        if is_null_or_str_type
            && let Some(pattern) = schema_obj
                .and_then(|o| o.get("pattern"))
                .and_then(Value::as_str)
        {
            return self.visit_pattern(pattern, &rule_name);
        }

        if is_null_or_str_type && schema_format.starts_with("uuid") {
            let prim_target = if rule_name == "root" {
                "root"
            } else {
                schema_format
            };
            return self.add_named_primitive(prim_target, "uuid");
        }

        let format_key = format!("{schema_format}-string");
        if is_null_or_str_type
            && let Some(fmt_rule_def) = STRING_FORMAT_RULES.get(format_key.as_str())
        {
            let fmt_rule = BuiltinRule {
                content: fmt_rule_def.content,
                deps: fmt_rule_def.deps,
            };
            let prim_name = self.add_primitive(&format_key, &fmt_rule);
            return self.add_rule(&rule_name, &prim_name);
        }

        if schema_type.and_then(Value::as_str) == Some("string")
            && schema_obj
                .is_some_and(|o| o.contains_key("minLength") || o.contains_key("maxLength"))
        {
            let char_rule = self.add_named_primitive("char", "char");
            let min_len = schema_obj
                .and_then(|o| o.get("minLength"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let max_len = schema_obj
                .and_then(|o| o.get("maxLength"))
                .and_then(Value::as_u64)
                .map(|m| m as usize);

            let rep = build_repetition(&char_rule, min_len, max_len, "");
            let rule_content = format!("\"\\\"\" {rep} \"\\\"\" space");
            return self.add_rule(&rule_name, &rule_content);
        }

        if schema_type.and_then(Value::as_str) == Some("integer")
            && schema_obj.is_some_and(|o| {
                o.contains_key("minimum")
                    || o.contains_key("exclusiveMinimum")
                    || o.contains_key("maximum")
                    || o.contains_key("exclusiveMaximum")
            })
        {
            let mut min_val = None;
            let mut max_val = None;

            if let Some(min_i) = schema_obj
                .and_then(|o| o.get("minimum"))
                .and_then(Value::as_i64)
            {
                min_val = Some(min_i);
            } else if let Some(emin_i) = schema_obj
                .and_then(|o| o.get("exclusiveMinimum"))
                .and_then(Value::as_i64)
            {
                min_val = Some(emin_i + 1);
            }

            if let Some(max_i) = schema_obj
                .and_then(|o| o.get("maximum"))
                .and_then(Value::as_i64)
            {
                max_val = Some(max_i);
            } else if let Some(emax_i) = schema_obj
                .and_then(|o| o.get("exclusiveMaximum"))
                .and_then(Value::as_i64)
            {
                max_val = Some(emax_i - 1);
            }

            let mut out = String::new();
            out.push('(');
            build_min_max_int(min_val, max_val, &mut out, 16, true);
            out.push_str(") space");
            return self.add_rule(&rule_name, &out);
        }

        if schema.as_object().is_some_and(Map::is_empty)
            || schema_type.and_then(Value::as_str) == Some("object")
        {
            let prim_name = self.add_named_primitive("object", "object");
            return self.add_rule(&rule_name, &prim_name);
        }

        if schema_type.is_none() && schema.is_object() {
            let prim_name = self.add_named_primitive("value", "value");
            return self.add_rule(&rule_name, &prim_name);
        }

        if let Some(type_str) = schema_type.and_then(Value::as_str)
            && PRIMITIVE_RULES.contains_key(type_str)
        {
            let prim_target = if rule_name == "root" {
                "root"
            } else {
                type_str
            };
            return self.add_named_primitive(prim_target, type_str);
        }

        self.errors.push(format!("Unrecognized schema: {schema}"));
        String::new()
    }

    fn check_errors(&self) -> Result<(), GrammarError> {
        if !self.errors.is_empty() {
            return Err(GrammarError::Parse(self.errors.join("\n")));
        }
        if !self.warnings.is_empty() {
            tracing::warn!(
                component = "GrammarConverter",
                "JSON schema conversion warning: {}",
                self.warnings.join("; ")
            );
        }
        Ok(())
    }

    fn format_grammar(&self) -> String {
        let mut ss = String::new();
        // GBNF output must lead with the root rule.
        if let Some(root_rule) = self.rules.get("root") {
            ss.push_str("root ::= ");
            ss.push_str(root_rule);
            ss.push('\n');
        }
        for (k, v) in &self.rules {
            if k != "root" {
                ss.push_str(k);
                ss.push_str(" ::= ");
                ss.push_str(v);
                ss.push('\n');
            }
        }
        ss
    }
}

/// Convert a JSON Schema document to a GBNF grammar string.
pub(crate) fn json_schema_to_grammar(schema_json: &str) -> Result<String, LlmProviderError> {
    let mut schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| LlmProviderError::LocalLlm(format!("invalid JSON schema: {e}")))?;

    let mut converter = SchemaConverter::new(false);
    converter.resolve_refs(&mut schema, "");
    converter.visit(&schema, "");
    converter
        .check_errors()
        .map_err(|e| LlmProviderError::LocalLlm(e.to_string()))?;

    Ok(converter.format_grammar())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_object_schema_to_gbnf() {
        let schema =
            r#"{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root ::= "), "{grammar}");
        assert!(
            grammar.contains("boolean") || grammar.contains("\"ok\""),
            "{grammar}"
        );
    }

    #[test]
    fn converts_enum_to_gbnf() {
        let schema = r#"{"type":"string","enum":["red","green","blue"]}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root ::= "), "{grammar}");
        assert!(grammar.contains("\"\\\"red\\\"\""), "{grammar}");
        assert!(grammar.contains("\"\\\"green\\\"\""), "{grammar}");
        assert!(grammar.contains("\"\\\"blue\\\"\""), "{grammar}");
    }

    #[test]
    fn converts_array_to_gbnf() {
        let schema = r#"{"type":"array","items":{"type":"string"}}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root ::= "), "{grammar}");
        assert!(grammar.contains("\"[\" space"), "{grammar}");
        assert!(grammar.contains("\"]\" space"), "{grammar}");
    }

    #[test]
    fn converts_integer_range_to_gbnf() {
        let schema = r#"{"type":"integer","minimum":1,"maximum":10}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root ::= "), "{grammar}");
    }

    #[test]
    fn converts_pattern_to_gbnf() {
        let schema = r#"{"type":"string","pattern":"^[a-z]+$"}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root ::= "), "{grammar}");
        assert!(grammar.contains("[a-z]"), "{grammar}");
    }

    /// Guards the reject-character-class builder in `not_strings`: the
    /// `[^"...]` bracket expression must cover every declared property name
    /// as a single expression — a malformed GBNF terminal like `[^"]ab]`
    /// (closing the bracket one character early) makes
    /// `LlamaSampler::grammar` panic on invalid GBNF (see
    /// `llama_cpp/generate.rs`), a guaranteed break for any object schema
    /// that declares named properties and allows additional ones.
    #[test]
    fn additional_properties_key_rejection_is_a_single_bracket_expression() {
        let schema = r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a"],"additionalProperties":true}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("[^\"ab]"), "{grammar}");
        assert!(!grammar.contains("]a]"), "{grammar}");
        assert!(!grammar.contains("]b]"), "{grammar}");
    }

    /// Same invariant via a second call site: single-property +
    /// object-valued `additionalProperties` (rather than
    /// `additionalProperties: true`) reaching the same `not_strings` path.
    #[test]
    fn additional_properties_schema_key_rejection_is_a_single_bracket_expression() {
        let schema = r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":{"type":"integer"}}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("[^\"a]"), "{grammar}");
        assert!(!grammar.contains("]a]"), "{grammar}");
    }
}
