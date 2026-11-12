use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rustyline::{
    Completer, Helper, Highlighter, Hinter,
    completion::{Completer, FilenameCompleter, Pair},
    highlight::Highlighter,
    hint::Hinter,
    validate::{ValidationContext, ValidationResult, Validator},
};

use crate::style::*;

/// Commands whose first argument is a filesystem path. These delegate to
/// rustyline's [`FilenameCompleter`], so completion reflects the live
/// filesystem instead of a static candidate list.
const PATH_COMMANDS: &[&str] = &["/add", "/image", "/debug"];

/// Commands whose argument is completed from the dynamic [`CompletionSources`].
const SESSION_COMMANDS: &[&str] = &["/session"];
const SKILL_COMMANDS: &[&str] = &["/skill", "/use", "/unload"];
const MODEL_COMMANDS: &[&str] = &["/model"];

/// Cap on dynamic candidates offered at once (sessions are the long pole).
const MAX_DYNAMIC_CANDIDATES: usize = 20;

/// Dynamic completion sources, refreshed by the host application before each
/// read: session IDs/names and skills change during a run, model names are
/// fetched once at startup.
#[derive(Debug, Default, Clone)]
pub struct CompletionSources {
    /// Session ID prefixes and names, offered for `/session`.
    pub sessions: Vec<String>,
    /// Skill names, offered for `/skill`, `/use`, `/unload`.
    pub skills: Vec<String>,
    /// Model names, offered for `/model`.
    pub models: Vec<String>,
}

/// Shared handle to the dynamic sources: the completer/hinter hold a clone,
/// the host application refreshes it before each `readline`.
pub type SharedCompletionSources = Arc<RwLock<CompletionSources>>;

fn read_sources(sources: &SharedCompletionSources) -> CompletionSources {
    sources
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Resolve the dynamic candidate pool for `cmd` given the text typed after it.
///
/// Returns `(line_prefix, pool, typed_filter)`: candidates render as
/// `line_prefix + candidate` and are filtered by `typed_filter`. Handles the
/// literal sub-words `/session delete …` and `/skill use …`.
fn dynamic_pool<'a>(
    cmd: &str,
    rest: &'a str,
    sources: &'a CompletionSources,
) -> Option<(String, &'a [String], String)> {
    let lower = rest.to_lowercase();
    if SESSION_COMMANDS.contains(&cmd) {
        if lower.starts_with("delete ") {
            let filter = rest.get("delete ".len()..).unwrap_or("").to_lowercase();
            return Some((format!("{cmd} delete "), &sources.sessions, filter));
        }
        return Some((format!("{cmd} "), &sources.sessions, lower));
    }
    if SKILL_COMMANDS.contains(&cmd) {
        if cmd == "/skill" && lower.starts_with("use ") {
            let filter = rest.get("use ".len()..).unwrap_or("").to_lowercase();
            return Some((format!("{cmd} use "), &sources.skills, filter));
        }
        return Some((format!("{cmd} "), &sources.skills, lower));
    }
    if MODEL_COMMANDS.contains(&cmd) {
        return Some((format!("{cmd} "), &sources.models, lower));
    }
    None
}

fn to_pairs(full_lines: Vec<String>) -> Vec<Pair> {
    full_lines
        .into_iter()
        .map(|full| Pair {
            display: full.clone(),
            replacement: full,
        })
        .collect()
}

#[derive(Completer, Helper, Highlighter, Hinter)]
pub struct CommandHelper {
    #[rustyline(Completer)]
    completer: CommandCompleter,
    #[rustyline(Hinter)]
    hinter: CommandHinter,
    #[rustyline(Highlighter)]
    highlighter: CommandHighlighter,
}

impl Validator for CommandHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        let input = ctx.input();

        // Check if input ends with backslash (continuation character)
        let trimmed = input.trim_end();
        if trimmed.ends_with('\\') {
            // Incomplete - needs more input
            return Ok(ValidationResult::Incomplete);
        }

        // Check for unclosed code fences (```)
        let fence_count = input.matches("```").count();
        if fence_count % 2 == 1 {
            // Unclosed code fence - needs more input
            return Ok(ValidationResult::Incomplete);
        }

        // Check for unclosed backtick code spans
        let backtick_count = input.matches('`').count();
        if backtick_count % 2 == 1 {
            // Unclosed backtick - needs more input
            return Ok(ValidationResult::Incomplete);
        }

        // Input is complete
        Ok(ValidationResult::Valid(None))
    }
}

impl Default for CommandHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandHelper {
    /// Create a `CommandHelper` with no command data (no completions or hints).
    pub fn new() -> Self {
        Self::with_sources(
            Vec::new(),
            HashMap::new(),
            Arc::new(RwLock::new(CompletionSources::default())),
        )
    }

    /// Create a `CommandHelper` populated with command names and subcommand
    /// completions, typically sourced from the binary's `CommandRegistry`.
    ///
    /// - `command_names`: all slash-command names (primary + aliases), e.g. `"/help"`, `"/quit"`.
    /// - `subcommands`: mapping from command name to its argument completions,
    ///   e.g. `"/mode" → ["agent", "casual", "planning"]`.
    pub fn with_commands(
        command_names: Vec<String>,
        subcommands: HashMap<String, Vec<String>>,
    ) -> Self {
        Self::with_sources(
            command_names,
            subcommands,
            Arc::new(RwLock::new(CompletionSources::default())),
        )
    }

    /// Like [`CommandHelper::with_commands`], but the completer and hinter also
    /// read dynamic candidates (sessions, skills, models) from `sources`.
    /// The host refreshes the shared sources before each `readline`; path
    /// arguments (`/add`, `/image`, `/debug`) always complete live from disk.
    pub fn with_sources(
        command_names: Vec<String>,
        subcommands: HashMap<String, Vec<String>>,
        sources: SharedCompletionSources,
    ) -> Self {
        Self {
            completer: CommandCompleter {
                command_names: command_names.clone(),
                subcommands: subcommands.clone(),
                sources: Arc::clone(&sources),
                paths: FilenameCompleter::new(),
            },
            hinter: CommandHinter {
                command_names,
                subcommands,
                sources,
            },
            highlighter: CommandHighlighter,
        }
    }
}

pub struct CommandCompleter {
    command_names: Vec<String>,
    subcommands: HashMap<String, Vec<String>>,
    sources: SharedCompletionSources,
    paths: FilenameCompleter,
}

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        if !line.starts_with('/') || pos == 0 {
            return Ok((0, vec![]));
        }

        let prefix = &line[..pos];

        // Check if we're completing a subcommand argument
        if let Some(space_pos) = prefix.find(' ') {
            let cmd = prefix[..space_pos].to_lowercase();
            let rest = prefix[space_pos + 1..].trim_start();
            let sub_prefix = rest.to_lowercase();

            // Path arguments delegate to the filename completer, which returns
            // the correct word-start offset itself.
            if PATH_COMMANDS.contains(&cmd.as_str()) {
                return self.paths.complete(line, pos, ctx);
            }

            // Dynamic candidates (sessions / skills / models).
            let sources = read_sources(&self.sources);
            if let Some((base, pool, filter)) = dynamic_pool(&cmd, rest, &sources) {
                let matches: Vec<String> = pool
                    .iter()
                    .filter(|s| s.to_lowercase().starts_with(&filter))
                    .take(MAX_DYNAMIC_CANDIDATES)
                    .map(|s| format!("{base}{s}"))
                    .collect();
                if !matches.is_empty() {
                    return Ok((0, to_pairs(matches)));
                }
            }

            // Static subcommand completions
            let subs = self
                .subcommands
                .get(&cmd)
                .map(|s| s.as_slice())
                .unwrap_or(&[]);
            let matches: Vec<String> = subs
                .iter()
                .filter(|s| s.starts_with(&sub_prefix))
                .take(5)
                .map(|s| format!("{} {}", cmd, s))
                .collect();
            return Ok((0, to_pairs(matches)));
        }

        // Top-level command completion — all matches are returned; the default
        // circular completion rotates through them on repeated Tab.
        let cmd_prefix = prefix.to_lowercase();
        let matches: Vec<String> = self
            .command_names
            .iter()
            .filter(|name| name.to_lowercase().starts_with(&cmd_prefix))
            .cloned()
            .collect();

        Ok((0, to_pairs(matches)))
    }
}

pub struct CommandHinter {
    command_names: Vec<String>,
    subcommands: HashMap<String, Vec<String>>,
    sources: SharedCompletionSources,
}

impl Hinter for CommandHinter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if !line.starts_with('/') || pos == 0 || line.len() != pos {
            return None;
        }

        // Check if we're hinting a subcommand
        if let Some(space_pos) = line.find(' ') {
            let cmd = line[..space_pos].to_lowercase();
            let rest = line[space_pos + 1..].trim_start();
            let sub_prefix = rest.to_lowercase();

            // No filesystem hints for path arguments — Tab completion handles
            // those live; a hint would go stale immediately.
            if PATH_COMMANDS.contains(&cmd.as_str()) {
                return None;
            }

            // Dynamic candidates: hint the remainder of a full-line candidate.
            let sources = read_sources(&self.sources);
            if let Some((base, pool, filter)) = dynamic_pool(&cmd, rest, &sources) {
                let matches: Vec<String> = pool
                    .iter()
                    .filter(|s| s.to_lowercase().starts_with(&filter))
                    .take(5)
                    .map(|s| format!("{base}{s}"))
                    .collect();
                if matches.len() == 1 {
                    // The candidate starts with the current line, so the hint
                    // is whatever comes after the cursor.
                    let hint = matches[0].get(pos..).unwrap_or("").to_string();
                    return if hint.is_empty() { None } else { Some(hint) };
                }
                if matches.len() > 1 {
                    let args: Vec<&str> = matches
                        .iter()
                        .filter_map(|m| m.get(base.len()..))
                        .map(|s| s.trim())
                        .collect();
                    return Some(format!("  ({})", args.join(" | ")));
                }
                // No dynamic matches — fall through to static subcommands.
            }

            let subs = self
                .subcommands
                .get(&cmd)
                .map(|s| s.as_slice())
                .unwrap_or(&[]);

            if !subs.is_empty() {
                let matches: Vec<&String> = subs
                    .iter()
                    .filter(|s| s.starts_with(&sub_prefix))
                    .take(5)
                    .collect();

                if matches.is_empty() {
                    return None;
                }

                if matches.len() == 1 && sub_prefix.is_empty() {
                    // Single exact match and no prefix typed yet — show the subcommand
                    return Some(format!(" {}", matches[0]));
                }

                if matches.len() == 1 && sub_prefix == *matches[0] {
                    // Exact match completed — no hint needed
                    return None;
                }

                // Multiple matches or partial match — show options
                let suggestions: Vec<&str> = matches.iter().map(|s| s.as_str()).collect();
                return Some(format!("  ({})", suggestions.join(" | ")));
            }
        }

        // Top-level command hinting
        let prefix = line.to_lowercase();
        let matches: Vec<&str> = self
            .command_names
            .iter()
            .filter(|name| name.to_lowercase().starts_with(&prefix))
            .take(3)
            .map(|s| s.as_str())
            .collect();

        if matches.is_empty() {
            return None;
        }

        if matches.len() == 1 {
            let hint = matches[0][pos..].to_string();
            if !hint.is_empty() {
                return Some(hint);
            }
        }

        let suggestions = matches.join("  ");
        Some(format!("  ({})", suggestions))
    }
}

pub struct CommandHighlighter;

impl Highlighter for CommandHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        if line.starts_with('/') {
            std::borrow::Cow::Owned(format!("{}{}{}", TITLE_COLOR, line, RESET))
        } else {
            std::borrow::Cow::Borrowed(line)
        }
    }

    fn highlight_hint<'l>(&self, hint: &'l str) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Owned(format!("{}{}{}", GRAY, hint, RESET))
    }

    fn highlight_candidate<'l>(
        &self,
        candidate: &'l str,
        _completion: rustyline::CompletionType,
    ) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Owned(format!("{}{}{}", TITLE_COLOR, candidate, RESET))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::history::DefaultHistory;

    fn make_helper(sources: CompletionSources) -> CommandHelper {
        let mut subcommands = HashMap::new();
        subcommands.insert(
            "/mode".to_string(),
            vec![
                "agent".to_string(),
                "casual".to_string(),
                "planning".to_string(),
            ],
        );
        subcommands.insert("/session".to_string(), vec!["delete".to_string()]);
        CommandHelper::with_sources(
            vec![
                "/help".to_string(),
                "/mode".to_string(),
                "/model".to_string(),
                "/session".to_string(),
                "/sessions".to_string(),
                "/settings".to_string(),
                "/showthink".to_string(),
                "/skill".to_string(),
                "/skills".to_string(),
                "/add".to_string(),
                "/image".to_string(),
            ],
            subcommands,
            Arc::new(RwLock::new(sources)),
        )
    }

    fn complete(helper: &CommandHelper, line: &str) -> Vec<String> {
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        helper
            .completer
            .complete(line, line.len(), &ctx)
            .unwrap()
            .1
            .into_iter()
            .map(|p| p.replacement)
            .collect()
    }

    fn hint(helper: &CommandHelper, line: &str) -> Option<String> {
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        helper.hinter.hint(line, line.len(), &ctx)
    }

    fn sources() -> CompletionSources {
        CompletionSources {
            sessions: vec!["abc123def456".to_string(), "my-project".to_string()],
            skills: vec!["rust-dev".to_string(), "review".to_string()],
            models: vec!["qwen3:8b".to_string(), "llama3".to_string()],
        }
    }

    #[test]
    fn top_level_completion_returns_all_matches() {
        let helper = make_helper(CompletionSources::default());
        // "/s" matches more than the old cap of 3 commands
        let matches = complete(&helper, "/s");
        assert!(matches.len() > 3, "expected all matches, got {matches:?}");
        assert!(matches.contains(&"/session".to_string()));
        assert!(matches.contains(&"/settings".to_string()));
    }

    #[test]
    fn non_command_input_completes_nothing() {
        let helper = make_helper(sources());
        assert!(complete(&helper, "hello").is_empty());
    }

    #[test]
    fn session_command_completes_ids_and_names() {
        let helper = make_helper(sources());
        let matches = complete(&helper, "/session ");
        assert_eq!(
            matches,
            vec!["/session abc123def456", "/session my-project"]
        );
        let filtered = complete(&helper, "/session my");
        assert_eq!(filtered, vec!["/session my-project"]);
    }

    #[test]
    fn session_delete_completes_sessions_after_keyword() {
        let helper = make_helper(sources());
        let matches = complete(&helper, "/session delete ");
        assert_eq!(
            matches,
            vec!["/session delete abc123def456", "/session delete my-project"]
        );
        // The static "delete" subcommand still completes on a bare prefix
        let matches = complete(&helper, "/session d");
        assert!(matches.contains(&"/session delete".to_string()));
    }

    #[test]
    fn skill_commands_complete_skill_names() {
        let helper = make_helper(sources());
        assert_eq!(complete(&helper, "/use rus"), vec!["/use rust-dev"]);
        assert_eq!(
            complete(&helper, "/unload "),
            vec!["/unload rust-dev", "/unload review"]
        );
        assert_eq!(
            complete(&helper, "/skill use rus"),
            vec!["/skill use rust-dev"]
        );
    }

    #[test]
    fn model_command_completes_model_names() {
        let helper = make_helper(sources());
        assert_eq!(complete(&helper, "/model q"), vec!["/model qwen3:8b"]);
    }

    #[test]
    fn static_subcommands_still_complete() {
        let helper = make_helper(sources());
        assert_eq!(complete(&helper, "/mode a"), vec!["/mode agent"]);
        assert_eq!(
            complete(&helper, "/mode "),
            vec!["/mode agent", "/mode casual", "/mode planning"]
        );
    }

    #[test]
    fn path_command_delegates_to_filesystem() {
        let dir = std::env::temp_dir().join(format!("th-input-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "x").unwrap();
        let helper = make_helper(sources());
        let line = format!("/add {}/he", dir.display());
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, candidates) = helper.completer.complete(&line, line.len(), &ctx).unwrap();
        let full: Vec<String> = candidates
            .iter()
            .map(|p| format!("{}{}", &line[..start], p.replacement))
            .collect();
        assert!(
            full.iter().any(|c| c.ends_with("hello.txt")),
            "expected a candidate completing to hello.txt, got {full:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hint_shows_remainder_of_unique_dynamic_match() {
        let helper = make_helper(sources());
        assert_eq!(hint(&helper, "/session my"), Some("-project".to_string()));
        // Multiple matches → option list
        let h = hint(&helper, "/skill ").unwrap();
        assert!(h.contains("rust-dev") && h.contains("review"), "got {h:?}");
    }

    #[test]
    fn hint_none_for_path_commands() {
        let helper = make_helper(sources());
        assert_eq!(hint(&helper, "/add src/"), None);
    }
}
