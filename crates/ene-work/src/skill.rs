use crate::error::WorkError;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub proactive_hint: Option<String>,
    pub emotion_note: Option<String>,
    pub body: String,
}

/// Parse Anthropic-style `SKILL.md` (YAML frontmatter + body).
pub fn parse_skill_md(text: &str) -> Result<SkillMeta, WorkError> {
    let text = text.trim_start_matches('\u{feff}');
    let (front, body) = if let Some(rest) = text.strip_prefix("---") {
        let rest = rest.trim_start_matches(['\n', '\r']);
        rest.split_once("\n---")
            .or_else(|| rest.split_once("\r\n---"))
            .ok_or_else(|| WorkError::Skill("missing frontmatter close".to_owned()))?
    } else {
        return Err(WorkError::Skill("SKILL.md must start with ---".to_owned()));
    };
    let mut name = String::new();
    let mut description = String::new();
    let mut proactive_hint = None;
    let mut emotion_note = None;
    for line in front.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').to_owned();
            match k.trim() {
                "name" => name = v,
                "description" => description = v,
                "ene.proactive_hint" => proactive_hint = Some(v),
                "ene.emotion_note" => emotion_note = Some(v),
                _ => {}
            }
        }
    }
    if name.is_empty() || description.is_empty() {
        return Err(WorkError::Skill(
            "name and description are required".to_owned(),
        ));
    }
    Ok(SkillMeta {
        name,
        description,
        proactive_hint,
        emotion_note,
        body: body.trim().to_owned(),
    })
}

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub meta: SkillMeta,
    pub path: PathBuf,
}

pub fn install_skill_dir(home: &Path, src: &Path) -> Result<InstalledSkill, WorkError> {
    let md = fs::read_to_string(src.join("SKILL.md"))?;
    let meta = parse_skill_md(&md)?;
    let dest = home.join(&meta.name);
    if dest.exists() {
        return Err(WorkError::Skill(format!(
            "already installed: {}",
            meta.name
        )));
    }
    copy_dir(src, &dest)?;
    Ok(InstalledSkill { meta, path: dest })
}

pub fn load_skill(home: &Path, name: &str) -> Result<SkillMeta, WorkError> {
    let path = home.join(name).join("SKILL.md");
    let canonical_home = home.canonicalize()?;
    let Some(file_name) = path.file_name().filter(|file| *file == "SKILL.md") else {
        return Err(WorkError::UnknownSkill(name.to_owned()));
    };
    let parent = path.parent().unwrap_or(home);
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| WorkError::UnknownSkill(name.to_owned()))?;
    if !canonical_parent.starts_with(&canonical_home) || !canonical_parent.join(file_name).exists()
    {
        return Err(WorkError::UnknownSkill(name.to_owned()));
    }
    let md = fs::read_to_string(path)?;
    parse_skill_md(&md)
}

pub fn catalog(home: &Path, enabled: &[String]) -> Result<Vec<(String, String)>, WorkError> {
    if !home.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(home)? {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !enabled.is_empty() && !enabled.iter().any(|n| n == &name) {
            continue;
        }
        if let Ok(meta) = load_skill(home, &name) {
            out.push((meta.name, meta.description));
        }
    }
    Ok(out)
}

/// Catalog as a System Context block (`skills.catalog`). Empty when nothing is installed.
#[must_use]
pub fn skill_catalog_blocks(home: &Path, enabled: &[String]) -> Vec<(String, String)> {
    let Ok(rows) = catalog(home, enabled) else {
        return Vec::new();
    };
    if rows.is_empty() {
        return Vec::new();
    }
    let mut text = String::from(
        "Installed skills (name — description). Call skill.load for a matching body.\n",
    );
    for (name, description) in rows {
        text.push_str("- ");
        text.push_str(&name);
        text.push_str(": ");
        text.push_str(&description);
        text.push('\n');
    }
    vec![("skills.catalog".to_owned(), text)]
}

/// Skill bodies whose catalog text matches `query` (`skills.active`).
#[must_use]
pub fn skill_active_blocks(home: &Path, enabled: &[String], query: &str) -> Vec<(String, String)> {
    let Ok(matched) = match_skills(home, enabled, query) else {
        return Vec::new();
    };
    if matched.is_empty() {
        return Vec::new();
    }
    let mut text = String::from("Active skills for this request:\n");
    for meta in matched {
        text.push_str("\n## ");
        text.push_str(&meta.name);
        text.push('\n');
        if let Some(note) = &meta.emotion_note {
            text.push_str("Tone: ");
            text.push_str(note);
            text.push('\n');
        }
        text.push_str(&meta.body);
        text.push('\n');
    }
    vec![("skills.active".to_owned(), text)]
}

/// `ene.proactive_hint` values from enabled skills (empty allowlist = all installed).
#[must_use]
pub fn skill_proactive_hints(home: &Path, enabled: &[String]) -> Vec<String> {
    let Ok(rows) = catalog(home, enabled) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, _) in rows {
        if let Ok(meta) = load_skill(home, &name)
            && let Some(hint) = meta.proactive_hint.filter(|text| !text.trim().is_empty())
        {
            out.push(hint);
        }
    }
    out
}

/// `ene.emotion_note` values from skills whose catalog text matches `query`.
#[must_use]
pub fn skill_emotion_notes(home: &Path, enabled: &[String], query: &str) -> Vec<String> {
    let Ok(matched) = match_skills(home, enabled, query) else {
        return Vec::new();
    };
    matched
        .into_iter()
        .filter_map(|meta| meta.emotion_note.filter(|text| !text.trim().is_empty()))
        .collect()
}

/// Catalog plus matching bodies, for job briefings.
#[must_use]
pub fn skill_context_lines(home: &Path, enabled: &[String], query: &str) -> Vec<(String, String)> {
    let mut out = skill_catalog_blocks(home, enabled);
    out.extend(skill_active_blocks(home, enabled, query));
    out
}

pub fn match_skills(
    home: &Path,
    enabled: &[String],
    query: &str,
) -> Result<Vec<SkillMeta>, WorkError> {
    let mut out = Vec::new();
    for (name, _) in catalog(home, enabled)? {
        let meta = load_skill(home, &name)?;
        if skill_matches(&meta, query) {
            out.push(meta);
        }
    }
    Ok(out)
}

#[must_use]
pub fn skill_matches(meta: &SkillMeta, query: &str) -> bool {
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return false;
    }
    let name = meta.name.to_lowercase();
    let desc = meta.description.to_lowercase();
    if !name.is_empty() && (q.contains(&name) || name.contains(q.trim())) {
        return true;
    }
    for token in tokens(&name).chain(tokens(&desc)) {
        if token.chars().count() >= 2 && q.contains(&token) {
            return true;
        }
    }
    for token in tokens(&q) {
        if token.chars().count() >= 2 && (name.contains(&token) || desc.contains(&token)) {
            return true;
        }
    }
    let bookmarkish = ["しおり", "bookmark", "itinerary", "まとめて"];
    let skill_is_bookmark = ["bookmark", "travel", "しおり", "itinerary"]
        .iter()
        .any(|key| name.contains(key) || desc.contains(key));
    skill_is_bookmark && bookmarkish.iter().any(|key| q.contains(key))
}

pub fn read_skill_file(home: &Path, name: &str, rel: &str) -> Result<String, WorkError> {
    let root = ene_registry::confine_tool_path(home, Path::new(name), false)
        .map_err(|err| WorkError::Skill(err.to_string()))?;
    if !root.is_dir() {
        return Err(WorkError::UnknownSkill(name.to_owned()));
    }
    let confined = ene_registry::confine_tool_path(&root, Path::new(rel), false)
        .map_err(|err| WorkError::Skill(err.to_string()))?;
    fs::read_to_string(confined).map_err(WorkError::from)
}

fn tokens(text: &str) -> impl Iterator<Item = String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
}

fn copy_dir(src: &Path, dest: &Path) -> Result<(), WorkError> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
