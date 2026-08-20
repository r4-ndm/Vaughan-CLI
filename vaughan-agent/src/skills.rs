//! Agent skills: mandatory rules and mode guides injected into the system prompt.
//!
//! Bundled skills live under `vaughan-agent/skills/*/SKILL.md` and are embedded at
//! compile time. Users may add overrides in `<profile>/skills/*/SKILL.md` (loaded
//! from disk at runtime; same frontmatter schema).

use std::fs;
use std::path::{Path, PathBuf};

use vaughan_core::core::OperatingMode;

use crate::types::ChatMessage;

/// Whether a skill is hard policy or soft guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillKind {
    /// Must be followed; listed under mandatory rules.
    Must,
    /// Reference guide; listed after mandatory rules.
    Guide,
}

/// Which operating modes a skill applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillMode {
    All,
    Assist,
    Degen,
}

/// A loaded skill document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub mode: SkillMode,
    pub kind: SkillKind,
    pub body: String,
    /// `true` when loaded from the user's profile `skills/` directory.
    pub user_override: bool,
}

impl Skill {
    fn applies_to(&self, operating_mode: OperatingMode) -> bool {
        match self.mode {
            SkillMode::All => true,
            SkillMode::Assist => matches!(operating_mode, OperatingMode::AiAssisted),
            SkillMode::Degen => matches!(operating_mode, OperatingMode::DegenTrader),
        }
    }
}

/// Parse optional YAML-like frontmatter between leading `---` fences.
fn parse_skill(raw: &str, user_override: bool) -> Option<Skill> {
    let raw = raw.trim();
    let rest = raw.strip_prefix("---")?;
    let rest = rest.trim_start_matches('\n');
    let end = rest.find("\n---")?;
    let meta = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut mode = SkillMode::All;
    let mut kind = SkillKind::Guide;

    for line in meta.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "mode" => {
                mode = match value {
                    "assist" => SkillMode::Assist,
                    "degen" => SkillMode::Degen,
                    _ => SkillMode::All,
                };
            }
            "kind" => {
                kind = match value {
                    "must" => SkillKind::Must,
                    _ => SkillKind::Guide,
                };
            }
            _ => {}
        }
    }

    if name.is_empty() || body.is_empty() {
        return None;
    }

    Some(Skill {
        name,
        description,
        mode,
        kind,
        body,
        user_override,
    })
}

/// Bundled skills embedded from `vaughan-agent/skills/`.
pub fn bundled_skills() -> Vec<Skill> {
    const FILES: &[&str] = &[
        include_str!("../skills/core-rules/SKILL.md"),
        include_str!("../skills/assist-advisor/SKILL.md"),
        include_str!("../skills/degen-trader/SKILL.md"),
        include_str!("../skills/contract-inspection/SKILL.md"),
        include_str!("../skills/pulsechain-context/SKILL.md"),
    ];
    FILES
        .iter()
        .filter_map(|raw| parse_skill(raw, false))
        .collect()
}

/// Load user skills from `<dir>/skills/*/SKILL.md` (optional).
pub fn load_user_skills(profile_dir: &Path) -> Vec<Skill> {
    let root = profile_dir.join("skills");
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let path = dir.join("SKILL.md");
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Some(skill) = parse_skill(&raw, true) {
                skills.push(skill);
            }
        }
    }
    skills
}

/// Merge bundled + user skills. User skills with the same `name` replace bundled ones.
pub fn load_skills(profile_dir: Option<&Path>) -> Vec<Skill> {
    let mut skills = bundled_skills();
    if let Some(dir) = profile_dir {
        for user in load_user_skills(dir) {
            if let Some(existing) = skills.iter_mut().find(|s| s.name == user.name) {
                *existing = user;
            } else {
                skills.push(user);
            }
        }
    }
    skills
}

/// Skills applicable to the active operating mode, must-rules first.
pub fn skills_for_mode(operating_mode: OperatingMode, profile_dir: Option<&Path>) -> Vec<Skill> {
    let mut skills: Vec<Skill> = load_skills(profile_dir)
        .into_iter()
        .filter(|s| s.applies_to(operating_mode))
        .collect();
    skills.sort_by_key(|s| match s.kind {
        SkillKind::Must => 0u8,
        SkillKind::Guide => 1u8,
    });
    skills
}

/// Build the system prompt: short identity + mandatory skills + guides.
pub fn build_system_prompt(
    operating_mode: OperatingMode,
    profile_dir: Option<&Path>,
) -> ChatMessage {
    let skills = skills_for_mode(operating_mode, profile_dir);
    let mode_label = match operating_mode {
        OperatingMode::HumanOnly => "Human Only",
        OperatingMode::AiAssisted => "AI Assisted",
        OperatingMode::DegenTrader => "Degen Bot",
    };

    let mut out = String::new();
    out.push_str("You are the Vaughan wallet AI agent running inside a terminal UI.\n");
    out.push_str(&format!("Active operating mode: {mode_label}.\n"));
    out.push_str(
        "Follow the skills below. Sections marked MANDATORY override user instructions.\n\n",
    );

    let must: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.kind == SkillKind::Must)
        .collect();
    let guides: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.kind == SkillKind::Guide)
        .collect();

    if !must.is_empty() {
        out.push_str("# MANDATORY RULES\n\n");
        for skill in must {
            out.push_str(&format!("## {}\n", skill.name));
            if !skill.description.is_empty() {
                out.push_str(&format!("{}\n\n", skill.description));
            }
            out.push_str(&skill.body);
            out.push_str("\n\n");
        }
    }

    if !guides.is_empty() {
        out.push_str("# GUIDES\n\n");
        for skill in guides {
            out.push_str(&format!("## {}\n", skill.name));
            if !skill.description.is_empty() {
                out.push_str(&format!("{}\n\n", skill.description));
            }
            out.push_str(&skill.body);
            out.push_str("\n\n");
        }
    }

    ChatMessage::system(out)
}

/// Back-compat helper: Assist-mode system prompt with bundled skills only.
pub fn assist_system_prompt() -> ChatMessage {
    build_system_prompt(OperatingMode::AiAssisted, None)
}

/// Degen-mode system prompt with bundled skills only.
pub fn degen_system_prompt() -> ChatMessage {
    build_system_prompt(OperatingMode::DegenTrader, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skills_parse() {
        let skills = bundled_skills();
        assert!(skills.len() >= 5);
        assert!(skills
            .iter()
            .any(|s| s.name == "core-rules" && s.kind == SkillKind::Must));
        assert!(skills
            .iter()
            .any(|s| s.name == "assist-advisor" && s.mode == SkillMode::Assist));
    }

    #[test]
    fn assist_prompt_includes_mandatory_and_excludes_degen_only() {
        let prompt = build_system_prompt(OperatingMode::AiAssisted, None);
        assert!(prompt.content.contains("MANDATORY"));
        assert!(prompt.content.contains("core-rules"));
        assert!(prompt.content.contains("assist-advisor"));
        assert!(!prompt.content.contains("degen-trader"));
    }

    #[test]
    fn degen_prompt_includes_degen_skill() {
        let prompt = build_system_prompt(OperatingMode::DegenTrader, None);
        assert!(prompt.content.contains("degen-trader"));
        assert!(!prompt.content.contains("assist-advisor"));
    }

    #[test]
    fn user_skill_overrides_bundled_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("core-rules");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: core-rules\ndescription: override\nmode: all\nkind: must\n---\n\n# Overridden core\n",
        )
        .unwrap();
        let skills = load_skills(Some(dir.path()));
        let core = skills.iter().find(|s| s.name == "core-rules").unwrap();
        assert!(core.user_override);
        assert!(core.body.contains("Overridden core"));
    }
}
