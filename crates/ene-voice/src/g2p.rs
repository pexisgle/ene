//! Grapheme-to-phoneme conversion and Kokoro phoneme tokenization (C4).
//!
//! The Kokoro ONNX model does not consume raw text or UTF-8 bytes; its
//! `input_ids` tensor is a flat phoneme vocabulary (`VOCAB_V10`, ~100 IPA-based
//! symbols). Feeding raw bytes produces garbage audio. This module converts
//! input text into that phoneme vocabulary:
//!
//! - **English**: a lightweight rule-based grapheme-to-phoneme pass producing
//!   IPA-ish phonemes. (A production deployment would front this with
//!   `espeak-ng`; the rules here are intentionally dependency-free.)
//! - **Japanese**: a kana → phoneme lookup table (hiragana/katakana → IPA).
//!
//! Unknown characters are dropped, matching the reference Kokoro tokenizer.
/// A single Kokoro vocabulary entry: phoneme character → token id.
const VOCAB: &[(char, u8)] = &[
    (';', 1),
    (':', 2),
    (',', 3),
    ('.', 4),
    ('!', 5),
    ('?', 6),
    ('—', 9),
    ('…', 10),
    ('"', 11),
    ('(', 12),
    (')', 13),
    ('“', 14),
    ('”', 15),
    (' ', 16),
    ('\u{0303}', 17),
    ('ʣ', 18),
    ('ʥ', 19),
    ('ʦ', 20),
    ('ʨ', 21),
    ('ᵝ', 22),
    ('\u{AB67}', 23),
    ('A', 24),
    ('I', 25),
    ('O', 31),
    ('Q', 33),
    ('S', 35),
    ('T', 36),
    ('W', 39),
    ('Y', 41),
    ('ᵊ', 42),
    ('a', 43),
    ('b', 44),
    ('c', 45),
    ('d', 46),
    ('e', 47),
    ('f', 48),
    ('h', 50),
    ('i', 51),
    ('j', 52),
    ('k', 53),
    ('l', 54),
    ('m', 55),
    ('n', 56),
    ('o', 57),
    ('p', 58),
    ('q', 59),
    ('r', 60),
    ('s', 61),
    ('t', 62),
    ('u', 63),
    ('v', 64),
    ('w', 65),
    ('x', 66),
    ('y', 67),
    ('z', 68),
    ('ɑ', 69),
    ('ɐ', 70),
    ('ɒ', 71),
    ('æ', 72),
    ('β', 75),
    ('ɔ', 76),
    ('ɕ', 77),
    ('ç', 78),
    ('ɖ', 80),
    ('ð', 81),
    ('ʤ', 82),
    ('ə', 83),
    ('ɚ', 85),
    ('ɛ', 86),
    ('ɜ', 87),
    ('ɟ', 90),
    ('ɡ', 92),
    ('ɥ', 99),
    ('ɨ', 101),
    ('ɪ', 102),
    ('ʝ', 103),
    ('ɯ', 110),
    ('ɰ', 111),
    ('ŋ', 112),
    ('ɳ', 113),
    ('ɲ', 114),
    ('ɴ', 115),
    ('ø', 116),
    ('ɸ', 118),
    ('θ', 119),
    ('œ', 120),
    ('ɹ', 123),
    ('ɾ', 125),
    ('ɻ', 126),
    ('ʁ', 128),
    ('ɽ', 129),
    ('ʂ', 130),
    ('ʃ', 131),
    ('ʈ', 132),
    ('ʧ', 133),
    ('ʊ', 135),
    ('ʋ', 136),
    ('ʌ', 138),
    ('ɣ', 139),
    ('ɤ', 140),
    ('χ', 142),
    ('ʎ', 143),
    ('ʒ', 147),
    ('ʔ', 148),
    ('ˈ', 156),
    ('ˌ', 157),
    ('ː', 158),
    ('ʰ', 162),
    ('ʲ', 164),
    ('↓', 169),
    ('→', 171),
    ('↗', 172),
    ('↘', 173),
    ('ᵻ', 177),
];

/// Look up the Kokoro token id for a phoneme character.
fn vocab_id(ch: char) -> Option<u8> {
    VOCAB.iter().find(|(c, _)| *c == ch).map(|(_, id)| *id)
}

/// Convert a phoneme string into Kokoro `input_ids`, wrapped with BOS/EOS
/// token `0` (matching the reference `get_token_ids`). Unknown characters are
/// dropped.
#[must_use]
pub fn tokenize(phonemes: &str) -> Vec<i64> {
    let mut tokens = Vec::with_capacity(phonemes.chars().count() + 2);
    tokens.push(0);
    for ch in phonemes.chars() {
        if let Some(id) = vocab_id(ch) {
            tokens.push(i64::from(id));
        }
    }
    tokens.push(0);
    tokens
}

/// Convert input `text` directly into Kokoro `input_ids` for `language`
/// (`"ja"` → Japanese kana mapping, anything else → English rules).
#[must_use]
pub fn text_to_tokens(text: &str, language: Option<&str>) -> Vec<i64> {
    let phonemes = match language.map(str::to_lowercase) {
        Some(lang) if lang.starts_with("ja") => japanese_to_phonemes(text),
        _ => english_to_phonemes(text),
    };
    tokenize(&phonemes)
}

/// Rule-based English grapheme-to-phoneme producing IPA-ish phonemes.
///
/// This is deliberately approximate: it maps common letter patterns to the
/// closest Kokoro vocabulary symbols so that English text yields plausible
/// (if not perfect) pronunciation. It is not a full G2P engine.
#[must_use]
#[expect(
    clippy::match_same_arms,
    reason = "distinct letters intentionally map to the same phoneme (e.g. c/k/q -> k)"
)]
pub fn english_to_phonemes(text: &str) -> String {
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // Two-letter digraphs first.
        let digraph = match (c, next) {
            ('t', Some('h')) => Some("θ"),
            ('s', Some('h')) => Some("ʃ"),
            ('c', Some('h')) => Some("ʧ"),
            ('p', Some('h')) => Some("f"),
            ('n', Some('g')) => Some("ŋ"),
            ('w', Some('h')) => Some("w"),
            ('q', Some('u')) => Some("kw"),
            _ => None,
        };
        if let Some(ph) = digraph {
            out.push_str(ph);
            i += 2;
            continue;
        }
        let phone = match c {
            'a' => "æ",
            'e' => "ɛ",
            'i' => "ɪ",
            'o' => "ɔ",
            'u' => "ʌ",
            'y' => "j",
            'b' => "b",
            'c' => "k",
            'd' => "d",
            'f' => "f",
            'g' => "ɡ",
            'h' => "h",
            'j' => "ʤ",
            'k' => "k",
            'l' => "l",
            'm' => "m",
            'n' => "n",
            'p' => "p",
            'q' => "k",
            'r' => "ɹ",
            's' => "s",
            't' => "t",
            'v' => "v",
            'w' => "w",
            'x' => "ks",
            'z' => "z",
            ' ' => " ",
            ',' => ",",
            '.' => ".",
            '!' => "!",
            '?' => "?",
            ';' => ";",
            ':' => ":",
            _ => "",
        };
        out.push_str(phone);
        i += 1;
    }
    out
}

/// Map Japanese kana (hiragana/katakana) to IPA-ish phonemes via a lookup
/// table. Romaji and other scripts fall back to the English rules.
#[must_use]
pub fn japanese_to_phonemes(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if let Some(ph) = kana_to_phoneme(ch) {
            out.push_str(ph);
        } else if ch.is_ascii() {
            out.push_str(&english_to_phonemes(&ch.to_string()));
        }
        // Non-kana, non-ASCII characters are dropped.
    }
    out
}

/// Hiragana/katakana → phoneme lookup. Returns the IPA-ish phoneme string for
/// a single kana, or `None` if the character is not a mapped kana.
#[expect(
    clippy::match_same_arms,
    reason = "distinct kana intentionally map to the same phoneme (e.g. じ/ぢ -> ʤi)"
)]
fn kana_to_phoneme(ch: char) -> Option<&'static str> {
    let phone = match ch {
        // Vowels
        'あ' | 'ア' => "a",
        'い' | 'イ' => "i",
        'う' | 'ウ' => "ɯ",
        'え' | 'エ' => "e",
        'お' | 'オ' => "o",
        // K-row
        'か' | 'カ' => "ka",
        'き' | 'キ' => "ki",
        'く' | 'ク' => "kɯ",
        'け' | 'ケ' => "ke",
        'こ' | 'コ' => "ko",
        // S-row
        'さ' | 'サ' => "sa",
        'し' | 'シ' => "ɕi",
        'す' | 'ス' => "sɯ",
        'せ' | 'セ' => "se",
        'そ' | 'ソ' => "so",
        // T-row
        'た' | 'タ' => "ta",
        'ち' | 'チ' => "ʧi",
        'つ' | 'ツ' => "ʦɯ",
        'て' | 'テ' => "te",
        'と' | 'ト' => "to",
        // N-row
        'な' | 'ナ' => "na",
        'に' | 'ニ' => "ɲi",
        'ぬ' | 'ヌ' => "nɯ",
        'ね' | 'ネ' => "ne",
        'の' | 'ノ' => "no",
        // H-row
        'は' | 'ハ' => "ha",
        'ひ' | 'ヒ' => "çi",
        'ふ' | 'フ' => "ɸɯ",
        'へ' | 'ヘ' => "he",
        'ほ' | 'ホ' => "ho",
        // M-row
        'ま' | 'マ' => "ma",
        'み' | 'ミ' => "mi",
        'む' | 'ム' => "mɯ",
        'め' | 'メ' => "me",
        'も' | 'モ' => "mo",
        // Y-row
        'や' | 'ヤ' => "ja",
        'ゆ' | 'ユ' => "jɯ",
        'よ' | 'ヨ' => "jo",
        // R-row
        'ら' | 'ラ' => "ɾa",
        'り' | 'リ' => "ɾi",
        'る' | 'ル' => "ɾɯ",
        'れ' | 'レ' => "ɾe",
        'ろ' | 'ロ' => "ɾo",
        // W-row
        'わ' | 'ワ' => "wa",
        'を' | 'ヲ' => "o",
        // N
        'ん' | 'ン' => "ɴ",
        // Voiced K/G
        'が' | 'ガ' => "ɡa",
        'ぎ' | 'ギ' => "ɡi",
        'ぐ' | 'グ' => "ɡɯ",
        'げ' | 'ゲ' => "ɡe",
        'ご' | 'ゴ' => "ɡo",
        // Voiced S/Z
        'ざ' | 'ザ' => "za",
        'じ' | 'ジ' => "ʤi",
        'ず' | 'ズ' => "zɯ",
        'ぜ' | 'ゼ' => "ze",
        'ぞ' | 'ゾ' => "zo",
        // Voiced T/D
        'だ' | 'ダ' => "da",
        'ぢ' | 'ヂ' => "ʤi",
        'づ' | 'ヅ' => "zɯ",
        'で' | 'デ' => "de",
        'ど' | 'ド' => "do",
        // Voiced H/B
        'ば' | 'バ' => "ba",
        'び' | 'ビ' => "bi",
        'ぶ' | 'ブ' => "bɯ",
        'べ' | 'ベ' => "be",
        'ぼ' | 'ボ' => "bo",
        // P-row
        'ぱ' | 'パ' => "pa",
        'ぴ' | 'ピ' => "pi",
        'ぷ' | 'プ' => "pɯ",
        'ぺ' | 'ペ' => "pe",
        'ぽ' | 'ポ' => "po",
        // Small vowels (map to the same phoneme as their full-size form)
        'ぁ' | 'ァ' => "a",
        'ぃ' | 'ィ' => "i",
        'ぅ' | 'ゥ' => "ɯ",
        'ぇ' | 'ェ' => "e",
        'ぉ' | 'ォ' => "o",
        // Yoon (contracted sounds, e.g. きゃ -> kja)
        'ゃ' | 'ャ' => "ja",
        'ゅ' | 'ュ' => "jɯ",
        'ょ' | 'ョ' => "jo",
        // Gemination: sokuon marks a glottal stop / consonant doubling (VOCAB id 148)
        'っ' | 'ッ' => "ʔ",
        // Long vowel mark (chōonpu, VOCAB id 158)
        'ー' => "ː",
        // Punctuation
        '、' => ",",
        '。' => ".",
        '！' => "!",
        '？' => "?",
        ' ' => " ",
        _ => return None,
    };
    Some(phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_wraps_with_bos_eos() {
        let tokens = tokenize("a");
        assert_eq!(tokens.first().copied(), Some(0));
        assert_eq!(tokens.last().copied(), Some(0));
        assert!(tokens.len() >= 3);
    }

    #[test]
    fn tokenize_drops_unknown_chars() {
        // '1' is not in VOCAB_V10; only BOS/EOS remain.
        let tokens = tokenize("1");
        assert_eq!(tokens, vec![0, 0]);
    }

    #[test]
    fn tokenize_maps_known_phonemes() {
        // 'a' -> 43, space -> 16.
        let tokens = tokenize("a ");
        assert_eq!(tokens, vec![0, 43, 16, 0]);
    }

    #[test]
    fn english_rules_basic() {
        let ph = english_to_phonemes("hi");
        assert!(ph.contains('h'));
        // 'i' maps to the IPA near-close vowel 'ɪ'.
        assert!(ph.contains('ɪ'));
    }

    #[test]
    fn english_th_digraph() {
        let ph = english_to_phonemes("th");
        assert_eq!(ph, "θ");
    }

    #[test]
    fn japanese_kana_mapping() {
        let ph = japanese_to_phonemes("あい");
        assert_eq!(ph, "ai");
    }

    #[test]
    fn japanese_katakana_mapping() {
        let ph = japanese_to_phonemes("カキ");
        assert_eq!(ph, "kaki");
    }

    #[test]
    fn japanese_yoon_and_gemination() {
        // Yoon kana map per-character (ゃ -> ja), so きゃ -> ki + ja.
        assert_eq!(japanese_to_phonemes("きゃ"), "kija");
        assert_eq!(japanese_to_phonemes("しゅ"), "ɕijɯ");
        // Gemination (sokuon) maps to a glottal stop.
        assert_eq!(japanese_to_phonemes("っか"), "ʔka");
        assert_eq!(japanese_to_phonemes("ッカ"), "ʔka");
        // Long vowel mark (chōonpu) maps to the length marker.
        assert_eq!(japanese_to_phonemes("カー"), "kaː");
    }

    #[test]
    fn japanese_yoon_gemination_tokenize() {
        // 'ʔ' -> 148 and 'ː' -> 158 exist in VOCAB, so they survive tokenization.
        let tokens = text_to_tokens("っー", Some("ja"));
        assert!(
            tokens.contains(&148),
            "glottal stop token missing: {tokens:?}"
        );
        assert!(
            tokens.contains(&158),
            "long vowel token missing: {tokens:?}"
        );
    }

    #[test]
    fn text_to_tokens_selects_language() {
        let ja = text_to_tokens("あ", Some("ja"));
        let en = text_to_tokens("あ", Some("en"));
        // Japanese path maps 'あ' -> 'a' (token 43); English path drops it.
        assert!(ja.contains(&43));
        assert!(!en.contains(&43));
    }

    #[test]
    fn empty_text_yields_bos_eos_only() {
        assert_eq!(text_to_tokens("", None), vec![0, 0]);
    }
}
