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
    if !path.exists() {
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
