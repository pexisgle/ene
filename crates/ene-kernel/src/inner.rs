use ene_session::InnerAspect;

const INNER_OPEN: &str = "<inner";
const INNER_CLOSE: &str = "</inner>";

/// Incremental surface/inner split for streamed model text.
#[derive(Debug)]
pub struct StreamingSurfaceInnerParser {
    pending: String,
    inside_inner: bool,
    inner_aspect: InnerAspect,
}

/// One parsed emission from [`StreamingSurfaceInnerParser`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamParseDelta {
    Surface(String),
    Inner {
        aspect: InnerAspect,
        text: String,
    },
}

impl StreamingSurfaceInnerParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            inside_inner: false,
            inner_aspect: InnerAspect::Thought,
        }
    }

    pub fn push(&mut self, chunk: &str) -> Vec<StreamParseDelta> {
        if chunk.is_empty() {
            return Vec::new();
        }
        self.pending.push_str(chunk);
        let mut out = Vec::new();
        loop {
            if self.inside_inner {
                if let Some(close_idx) = self.pending.find(INNER_CLOSE) {
                    let body = self.pending[..close_idx].trim();
                    if !body.is_empty() {
                        out.push(StreamParseDelta::Inner {
                            aspect: self.inner_aspect,
                            text: body.to_owned(),
                        });
                    }
                    self.pending = self
                        .pending
                        .split_off(close_idx + INNER_CLOSE.len());
                    self.inside_inner = false;
                    continue;
                }
                let hold_back = INNER_CLOSE.len().saturating_sub(1);
                if self.pending.len() > hold_back {
                    let split_at = self.pending.len() - hold_back;
                    let body = self.pending[..split_at].to_owned();
                    self.pending = self.pending.split_off(split_at);
                    if !body.is_empty() {
                        out.push(StreamParseDelta::Inner {
                            aspect: self.inner_aspect,
                            text: body,
                        });
                    }
                }
                break;
            }

            let Some(start) = self.pending.find(INNER_OPEN) else {
                emit_surface_prefix(&mut self.pending, &mut out);
                break;
            };
            if start > 0 {
                out.push(StreamParseDelta::Surface(self.pending[..start].to_owned()));
                self.pending = self.pending.split_off(start);
            }
            let Some(tag_end) = self.pending.find('>') else {
                if !could_start_inner_tag(&self.pending) {
                    out.push(StreamParseDelta::Surface(std::mem::take(&mut self.pending)));
                }
                break;
            };
            let header = &self.pending[..tag_end];
            self.inner_aspect = parse_aspect_attr(header).unwrap_or(InnerAspect::Thought);
            self.pending = self.pending.split_off(tag_end + 1);
            self.inside_inner = true;
        }
        out
    }

    pub fn flush(&mut self) -> Vec<StreamParseDelta> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        if self.inside_inner {
            let body = std::mem::take(&mut self.pending).trim().to_owned();
            self.inside_inner = false;
            if body.is_empty() {
                return Vec::new();
            }
            return vec![StreamParseDelta::Inner {
                aspect: self.inner_aspect,
                text: body,
            }];
        }
        let mut out = Vec::new();
        emit_surface_prefix(&mut self.pending, &mut out);
        out
    }
}

fn emit_surface_prefix(pending: &mut String, out: &mut Vec<StreamParseDelta>) {
    let hold_back = longest_inner_open_prefix_len(pending);
    if pending.len() <= hold_back {
        return;
    }
    let split_at = pending.len() - hold_back;
    let surface = pending[..split_at].to_owned();
    *pending = pending.split_off(split_at);
    if !surface.is_empty() {
        out.push(StreamParseDelta::Surface(surface));
    }
}

fn could_start_inner_tag(pending: &str) -> bool {
    INNER_OPEN.starts_with(pending) || pending.starts_with(INNER_OPEN)
}

fn longest_inner_open_prefix_len(text: &str) -> usize {
    (1..=INNER_OPEN.len())
        .rev()
        .find(|&len| text.ends_with(&INNER_OPEN[..len]))
        .unwrap_or(0)
}

#[must_use]
pub const fn model_visible_for(aspect: InnerAspect) -> bool {
    !matches!(aspect, InnerAspect::Emotion)
}

#[must_use]
pub fn split_surface_and_inner(raw: &str) -> (String, Vec<(InnerAspect, String)>) {
    let mut speech = String::new();
    let mut inner = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<inner") {
        speech.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            speech.push_str(after);
            break;
        };
        let header = &after[..tag_end];
        let aspect = parse_aspect_attr(header).unwrap_or(InnerAspect::Thought);
        let body_start = tag_end + 1;
        if let Some(close_rel) = after[body_start..].find("</inner>") {
            let body = after[body_start..body_start + close_rel].trim();
            if !body.is_empty() {
                inner.push((aspect, body.to_owned()));
            }
            rest = &after[body_start + close_rel + "</inner>".len()..];
        } else {
            speech.push_str(after);
            rest = "";
            break;
        }
    }
    if !rest.is_empty() && !rest.starts_with("<inner") {
        speech.push_str(rest);
    }
    (collapse_ws(&speech), inner)
}

fn parse_aspect_attr(header: &str) -> Option<InnerAspect> {
    let key = "aspect=\"";
    let idx = header.find(key)?;
    let rest = &header[idx + key.len()..];
    let end = rest.find('"')?;
    match rest[..end].trim() {
        "thought" => Some(InnerAspect::Thought),
        "emotion" => Some(InnerAspect::Emotion),
        "action_intent" => Some(InnerAspect::ActionIntent),
        _ => None,
    }
}

fn collapse_ws(text: &str) -> String {
    let trimmed = text.trim();
    let mut out = String::new();
    let mut prev_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            out.push(ch);
        }
    }
    out
}

#[must_use]
pub fn derive_thought_from_thinking(
    inner: &[(InnerAspect, String)],
    thinking: Option<&str>,
    enabled: bool,
) -> Option<(InnerAspect, String)> {
    if !enabled {
        return None;
    }
    if inner
        .iter()
        .any(|(aspect, _)| *aspect == InnerAspect::Thought)
    {
        return None;
    }
    let thinking = thinking.map(str::trim).filter(|text| !text.is_empty())?;
    Some((InnerAspect::Thought, thinking.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{
        StreamParseDelta, StreamingSurfaceInnerParser, split_surface_and_inner,
    };
    use ene_session::InnerAspect;

    #[test]
    fn streaming_parser_matches_batch_splitter() {
        let raw = r#"Hello <inner aspect="thought">secret</inner> world"#;
        let mut parser = StreamingSurfaceInnerParser::new();
        let mut streamed = Vec::new();
        for chunk in ["Hello ", "<inner aspect=\"thought\">sec", "ret</inner> world"] {
            streamed.extend(parser.push(chunk));
        }
        streamed.extend(parser.flush());
        let (speech, inner) = split_surface_and_inner(raw);
        let surface: String = streamed
            .iter()
            .filter_map(|delta| match delta {
                StreamParseDelta::Surface(text) => Some(text.as_str()),
                StreamParseDelta::Inner { .. } => None,
            })
            .collect();
        assert_eq!(
            surface.split_whitespace().collect::<String>(),
            speech.split_whitespace().collect::<String>()
        );
        assert_eq!(
            inner,
            vec![(InnerAspect::Thought, "secret".to_owned())]
        );
    }

    #[test]
    fn streaming_parser_hides_split_inner_open_tag() {
        let mut parser = StreamingSurfaceInnerParser::new();
        let mut out = parser.push("before <in");
        out.extend(parser.push("ner aspect=\"thought\">x</inner> after"));
        out.extend(parser.flush());
        assert!(!out.iter().any(|delta| matches!(
            delta,
            StreamParseDelta::Surface(text) if text.contains("<inner")
        )));
        assert!(out.iter().any(|delta| matches!(
            delta,
            StreamParseDelta::Inner { text, .. } if text == "x"
        )));
    }
}
