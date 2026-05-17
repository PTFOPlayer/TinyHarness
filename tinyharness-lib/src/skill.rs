use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Maximum size for a skill content (characters).
/// Skills exceeding this are truncated with a notice.
const SKILL_MAX_CHARS: usize = 10_000;

/// A skill discovered from a SKILL.md file.
///
/// Skills are reusable instruction sets that can be invoked by the user
/// (via `/skill <name>`) or automatically by the model (via the `invoke_skill`
/// signal tool). They live in `~/.tinyharness/skills/<name>/SKILL.md` (personal)
/// or `.tinyharness/skills/<name>/SKILL.md` (project-local).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// The skill name (from frontmatter `name` field or directory name).
    pub name: String,
    /// A short description of when and how to use this skill.
    pub description: String,
    /// Optional hint shown to the user about what argument to pass (e.g. "file path to review").
    pub argument_hint: Option<String>,
    /// Optional compatibility string (e.g. "rust", "python", "any").
    pub compatibility: Option<String>,
    /// If true, the model may NOT auto-invoke this skill; only manual `/skill` invocation is allowed.
    pub disable_model_invocation: bool,
    /// Optional license identifier (e.g. "MIT", "Apache-2.0").
    pub license: Option<String>,
    /// Optional arbitrary metadata as key-value pairs.
    pub metadata: Option<HashMap<String, String>>,
    /// If true, the skill can be invoked by the user via `/skill <name>`.
    pub user_invocable: bool,
    /// The full markdown content of the SKILL.md file (after frontmatter).
    pub content: String,
    /// The filesystem path where this skill was found.
    pub path: PathBuf,
    /// Whether this is a personal (home) or project-local skill.
    pub source: SkillSource,
}

/// Where a skill was discovered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// `~/.tinyharness/skills/<name>/SKILL.md`
    Personal,
    /// `.tinyharness/skills/<name>/SKILL.md` (in project directory)
    Project,
}

/// Parsed frontmatter fields from a SKILL.md file.
///
/// Supported fields (compatible with VS Code agent skills):
/// - `name` — skill name (defaults to directory name)
/// - `description` — short description
/// - `argument-hint` — hint about what argument to pass
/// - `compatibility` — compatibility info (e.g. "rust", "python")
/// - `disable-model-invocation` — if true, model cannot auto-invoke
/// - `license` — license identifier
/// - `metadata` — arbitrary key-value pairs (indented block)
/// - `user-invocable` — if true, user can invoke via `/skill <name>`
struct ParsedFrontmatter {
    name: Option<String>,
    description: Option<String>,
    argument_hint: Option<String>,
    compatibility: Option<String>,
    disable_model_invocation: bool,
    license: Option<String>,
    metadata: Option<HashMap<String, String>>,
    user_invocable: bool,
}

/// Parse simple YAML-like frontmatter (key: value pairs with optional
/// indented metadata block). This avoids pulling in serde_yaml for what
/// is essentially flat `key: value` data.
fn parse_frontmatter(text: &str) -> ParsedFrontmatter {
    let mut name = None;
    let mut description = None;
    let mut argument_hint = None;
    let mut compatibility = None;
    let mut disable_model_invocation = false;
    let mut license = None;
    let mut user_invocable = true;
    let mut metadata: Option<HashMap<String, String>> = None;

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Skip blank lines
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Check for indented line — metadata sub-properties
        if line.starts_with("  ") || line.starts_with("\t") {
            // This is a continuation of a metadata block
            if let Some(ref mut map) = metadata {
                let trimmed = line.trim();
                if let Some((k, v)) = trimmed.split_once(':') {
                    map.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
                }
            }
            i += 1;
            continue;
        }

        // Top-level key: value
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "name" => name = Some(value.trim_matches('"').to_string()),
                "description" => description = Some(value.trim_matches('"').to_string()),
                "argument-hint" => argument_hint = Some(value.trim_matches('"').to_string()),
                "compatibility" => compatibility = Some(value.trim_matches('"').to_string()),
                "disable-model-invocation" => disable_model_invocation = value == "true",
                "license" => license = Some(value.trim_matches('"').to_string()),
                "user-invocable" => user_invocable = value == "true" || value.is_empty(),
                "metadata" => {
                    // Start a metadata block; value after "metadata:" is ignored
                    metadata.get_or_insert_with(HashMap::new);
                }
                _ => {} // ignore unknown keys
            }
        }

        i += 1;
    }

    ParsedFrontmatter {
        name,
        description,
        argument_hint,
        compatibility,
        disable_model_invocation,
        license,
        metadata,
        user_invocable,
    }
}

/// Parse a SKILL.md file into a `Skill`.
///
/// The file format is:
/// ```markdown
/// ---
/// name: my-skill
/// description: A short description of what this skill does
/// argument-hint: file path to review
/// compatibility: rust
/// disable-model-invocation: false
/// license: MIT
/// metadata:
///   version: "1.0"
/// user-invocable: true
/// ---
///
/// # My Skill
///
/// Instructions for the skill go here...
/// ```
///
/// The frontmatter is optional. If missing, the skill name is derived from
/// the directory name and description defaults to an empty string.
pub fn parse_skill_md(content: &str, path: &Path, source: SkillSource) -> Result<Skill, String> {
    let (frontmatter, body) = extract_frontmatter(content);

    let fm = frontmatter.map(|s| parse_frontmatter(&s));

    // Name: frontmatter > directory name
    let name = fm
        .as_ref()
        .and_then(|f| f.name.clone())
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
        })
        .ok_or_else(|| format!("Cannot determine skill name for {}", path.display()))?;

    let description = fm
        .as_ref()
        .and_then(|f| f.description.clone())
        .unwrap_or_default();
    let argument_hint = fm.as_ref().and_then(|f| f.argument_hint.clone());
    let compatibility = fm.as_ref().and_then(|f| f.compatibility.clone());
    let disable_model_invocation = fm
        .as_ref()
        .map(|f| f.disable_model_invocation)
        .unwrap_or(false);
    let license = fm.as_ref().and_then(|f| f.license.clone());
    let metadata = fm.as_ref().and_then(|f| f.metadata.clone());
    let user_invocable = fm.as_ref().map(|f| f.user_invocable).unwrap_or(true);

    let content = truncate_skill_content(body.trim(), &name);

    Ok(Skill {
        name,
        description,
        argument_hint,
        compatibility,
        disable_model_invocation,
        license,
        metadata,
        user_invocable,
        content,
        path: path.to_path_buf(),
        source,
    })
}

/// Extract YAML frontmatter from a markdown file.
/// Returns (Some(frontmatter_str), body_str) if frontmatter exists,
/// or (None, full_content) if no frontmatter is found.
fn extract_frontmatter(content: &str) -> (Option<String>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }

    // Find the closing ---
    let after_first = &trimmed[3..]; // skip opening ---
    // Skip whitespace after opening ---
    let after_first = after_first.trim_start();

    if let Some(end_pos) = after_first.find("\n---") {
        let yaml_content = after_first[..end_pos].to_string();
        // Body starts after the closing ---
        let body_start = end_pos + 4; // skip "\n---"
        let body = after_first[body_start..].to_string();
        (Some(yaml_content), body)
    } else if let Some(end_pos) = after_first.find("---") {
        let yaml_content = after_first[..end_pos].to_string();
        let body_start = end_pos + 3; // skip "---"
        let body = after_first[body_start..].to_string();
        (Some(yaml_content), body)
    } else {
        (None, content.to_string())
    }
}

/// Truncate skill content that exceeds the maximum size.
fn truncate_skill_content(content: &str, name: &str) -> String {
    if content.len() <= SKILL_MAX_CHARS {
        return content.to_string();
    }

    let head_ratio = 0.70;
    let head_end = (SKILL_MAX_CHARS as f64 * head_ratio) as usize;
    let tail_size = SKILL_MAX_CHARS - head_end;

    let head = &content[..content.floor_char_boundary(head_end)];
    let tail_start = content.len().saturating_sub(tail_size);
    let tail = &content[content.floor_char_boundary(tail_start)..];

    format!(
        "{head}\n\n[...truncated skill '{name}': showing first {head_end} + last {tail_size} chars. Use the read tool to view the full file.]\n\n{tail}"
    )
}

/// Personal skills directory: `~/.tinyharness/skills/`
pub fn personal_skills_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".tinyharness/skills")
}

/// Project-local skills directory: `.tinyharness/skills/` (relative to CWD)
pub fn project_skills_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".tinyharness/skills")
}

/// Discover all skills from both personal and project directories.
///
/// Scans `<dir>/*/SKILL.md` where `*` is the skill name.
/// Project skills take precedence over personal skills with the same name.
pub fn discover_skills() -> Vec<Skill> {
    let mut skills = Vec::new();

    // Discover personal skills
    let personal_dir = personal_skills_dir();
    if let Ok(entries) = discover_skills_from_dir(&personal_dir, SkillSource::Personal) {
        skills.extend(entries);
    }

    // Discover project skills (these override personal skills with the same name)
    let project_dir = project_skills_dir();
    if let Ok(entries) = discover_skills_from_dir(&project_dir, SkillSource::Project) {
        // Merge: project skills override personal skills with the same name
        for skill in entries {
            if let Some(pos) = skills.iter().position(|s| s.name == skill.name) {
                skills[pos] = skill;
            } else {
                skills.push(skill);
            }
        }
    }

    // Sort alphabetically by name
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Discover skills from a specific directory.
fn discover_skills_from_dir(dir: &Path, source: SkillSource) -> Result<Vec<Skill>, std::io::Error> {
    let mut skills = Vec::new();

    if !dir.is_dir() {
        return Ok(skills);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        let content = match std::fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to read skill file {}: {}",
                    skill_file.display(),
                    e
                );
                continue;
            }
        };

        match parse_skill_md(&content, &skill_file, source) {
            Ok(skill) => skills.push(skill),
            Err(e) => {
                eprintln!(
                    "Warning: Failed to parse skill file {}: {}",
                    skill_file.display(),
                    e
                );
                continue;
            }
        }
    }

    Ok(skills)
}

/// A registry of discovered skills, used by the agent loop and commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRegistry {
    pub skills: Vec<Skill>,
}

impl SkillRegistry {
    /// Discover and build a registry of all available skills.
    pub fn discover() -> Self {
        SkillRegistry {
            skills: discover_skills(),
        }
    }

    /// Look up a skill by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        let name_lower = name.to_lowercase();
        self.skills
            .iter()
            .find(|s| s.name.to_lowercase() == name_lower)
    }

    /// Get all skills that can be auto-invoked by the model.
    pub fn auto_invocable_skills(&self) -> Vec<&Skill> {
        self.skills
            .iter()
            .filter(|s| !s.disable_model_invocation)
            .collect()
    }

    /// Format the skill index for injection into the system prompt.
    ///
    /// Returns a string listing all available skills with names and descriptions,
    /// and compatibility/argument hints for auto-invocable skills.
    pub fn format_index_for_prompt(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        lines.push("## Available Skills\n".to_string());
        lines.push("You can invoke a skill using the `invoke_skill` tool with the skill name. Skills provide specialized instructions for specific tasks.\n".to_string());

        for skill in &self.skills {
            let mut parts = vec![format!("**{}**", skill.name)];

            if !skill.description.is_empty() {
                parts.push(format!(": {}", skill.description));
            }

            if let Some(hint) = &skill.argument_hint {
                parts.push(format!(" (arg: {})", hint));
            }

            if let Some(compat) = &skill.compatibility {
                parts.push(format!(" [{}]", compat));
            }

            if skill.disable_model_invocation {
                parts.push(" _(manual invocation only)_".to_string());
            }

            lines.push(format!("- {}", parts.join("")));
        }

        lines.push("\nUse `invoke_skill` to activate a skill. The skill's full instructions will be injected into the conversation.".to_string());
        lines.join("\n")
    }

    /// Format the full content of a skill for injection into the system prompt
    /// when the skill is active.
    pub fn format_skill_content(&self, skill: &Skill) -> String {
        format!(
            "## Active Skill: {}\n\n{}\n\n---\nSkill instructions:\n{}",
            skill.name, skill.description, skill.content
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_skill_md_with_frontmatter() {
        let content = r#"---
name: test-skill
description: A test skill for unit tests
argument-hint: file path to test
compatibility: rust
disable-model-invocation: false
license: MIT
metadata:
  version: "1.0"
user-invocable: true
---

# Test Skill

This is the body of the test skill.
"#;
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("test-skill").join("SKILL.md");
        fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        fs::write(&skill_path, content).unwrap();

        let skill = parse_skill_md(content, &skill_path, SkillSource::Personal).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill for unit tests");
        assert_eq!(skill.argument_hint, Some("file path to test".to_string()));
        assert_eq!(skill.compatibility, Some("rust".to_string()));
        assert!(!skill.disable_model_invocation);
        assert_eq!(skill.license, Some("MIT".to_string()));
        assert!(skill.user_invocable);
        assert!(skill.content.contains("# Test Skill"));
        assert!(skill.content.contains("body of the test skill"));
    }

    #[test]
    fn test_parse_skill_md_without_frontmatter() {
        let content = "# My Skill\n\nThis is a skill without frontmatter.";
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("my-skill").join("SKILL.md");
        fs::create_dir_all(skill_path.parent().unwrap()).unwrap();

        let skill = parse_skill_md(content, &skill_path, SkillSource::Project).unwrap();
        assert_eq!(skill.name, "my-skill"); // Falls back to directory name
        assert_eq!(skill.description, ""); // Defaults to empty
        assert!(skill.argument_hint.is_none());
        assert!(skill.compatibility.is_none());
        assert!(!skill.disable_model_invocation);
        assert!(skill.user_invocable); // Defaults to true
    }

    #[test]
    fn test_parse_skill_md_minimal_frontmatter() {
        let content = "---\nname: minimal\n---\n\nJust the name, no description.";
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("minimal").join("SKILL.md");
        fs::create_dir_all(skill_path.parent().unwrap()).unwrap();

        let skill = parse_skill_md(content, &skill_path, SkillSource::Personal).unwrap();
        assert_eq!(skill.name, "minimal");
        assert_eq!(skill.description, "");
    }

    #[test]
    fn test_extract_frontmatter() {
        let content = "---\nname: test\n---\nBody text";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_some());
        assert!(body.contains("Body text"));
    }

    #[test]
    fn test_extract_frontmatter_none() {
        let content = "No frontmatter here, just text.";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_frontmatter_with_metadata() {
        let fm = parse_frontmatter("metadata:\n  version: \"1.0\"\n  author: test");
        assert!(fm.metadata.is_some());
        let meta = fm.metadata.unwrap();
        assert_eq!(meta.get("version").unwrap(), "1.0");
        assert_eq!(meta.get("author").unwrap(), "test");
    }

    #[test]
    fn test_parse_frontmatter_boolean_fields() {
        let fm = parse_frontmatter("disable-model-invocation: true\nuser-invocable: false");
        assert!(fm.disable_model_invocation);
        assert!(!fm.user_invocable);
    }

    #[test]
    fn test_parse_frontmatter_defaults() {
        let fm = parse_frontmatter("name: hello");
        assert_eq!(fm.name, Some("hello".to_string()));
        assert!(!fm.disable_model_invocation); // defaults to false
        assert!(fm.user_invocable); // defaults to true
        assert!(fm.description.is_none());
        assert!(fm.argument_hint.is_none());
        assert!(fm.metadata.is_none());
    }

    #[test]
    fn test_skill_registry_format_index() {
        let skill = Skill {
            name: "rust-dev".to_string(),
            description: "Rust development best practices".to_string(),
            argument_hint: Some("Rust file or module".to_string()),
            compatibility: Some("rust".to_string()),
            disable_model_invocation: false,
            license: None,
            metadata: None,
            user_invocable: true,
            content: "Instructions...".to_string(),
            path: PathBuf::from("/test/SKILL.md"),
            source: SkillSource::Personal,
        };

        let registry = SkillRegistry {
            skills: vec![skill],
        };

        let index = registry.format_index_for_prompt();
        assert!(index.contains("rust-dev"));
        assert!(index.contains("invoke_skill"));
        assert!(index.contains("arg:"));
        assert!(index.contains("[rust]"));
    }

    #[test]
    fn test_skill_registry_format_disabled() {
        let skill = Skill {
            name: "manual-only".to_string(),
            description: "Manual invocation only".to_string(),
            argument_hint: None,
            compatibility: None,
            disable_model_invocation: true,
            license: None,
            metadata: None,
            user_invocable: true,
            content: "Instructions...".to_string(),
            path: PathBuf::from("/test/SKILL.md"),
            source: SkillSource::Personal,
        };

        let registry = SkillRegistry {
            skills: vec![skill],
        };

        let index = registry.format_index_for_prompt();
        assert!(index.contains("manual invocation only"));
    }
}
