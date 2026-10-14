//! Proves `tinyharness-lib` can be driven entirely through an injected
//! [`Paths`] — no code path may reach for the ambient `$HOME`, `$XDG_*`, or
//! process CWD when a `Paths` is supplied.
//!
//! Each test builds a disposable root with `tempfile`, points `Paths` at it,
//! and asserts that files land inside that root. The real user configuration
//! is never touched, and no test mutates process-global environment variables.

use std::path::Path;
use tinyharness_lib::paths::Paths;

/// A realistic three-root layout (config / data / project) with an optional
/// legacy home, kept distinct so personal- and project-scoped discovery can
/// be told apart. `Paths::with_home` collapses the legacy and project roots
/// into one directory, which is fine for isolation but not for precedence
/// tests — hence this helper.
fn layout(tmp: &Path) -> Paths {
    Paths::new(
        tmp.join("cfg/tinyharness"),
        tmp.join("data/tinyharness"),
        tmp.join("proj"),
    )
    .with_legacy_home(tmp.join("home"))
}

#[test]
fn settings_round_trip_through_injected_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    let settings = tinyharness_lib::Settings {
        context_limit: Some(123_456),
        questions_enabled: false,
        ..Default::default()
    };

    tinyharness_lib::save_settings_to(&paths, &settings).unwrap();

    // The file must live under the injected root, never the real home dir.
    let written = paths.settings_file();
    assert!(
        written.starts_with(tmp.path()),
        "settings escaped the injected root: {}",
        written.display()
    );
    assert!(written.is_file());

    let loaded = tinyharness_lib::load_settings_from(&paths);
    assert_eq!(loaded.context_limit, Some(123_456));
    assert!(!loaded.questions_enabled);

    // A store built from the same Paths agrees with the free functions.
    let store = tinyharness_lib::SettingsStore::from_paths(&paths);
    assert_eq!(store.path(), written.as_path());
    assert_eq!(store.load().unwrap().context_limit, Some(123_456));
}

#[test]
fn prompts_are_seeded_into_injected_dir_without_touching_home() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    let dir = tinyharness_lib::ensure_prompts_initialized_for(&paths);
    assert_eq!(dir, paths.prompts_dir());
    assert!(dir.starts_with(tmp.path()));

    for name in [
        "header.md",
        "casual.md",
        "planning.md",
        "agent.md",
        "research.md",
    ] {
        let file = dir.join(name);
        assert!(file.is_file(), "missing seeded prompt {name}");
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .chars()
                .any(|c| !c.is_whitespace()),
            "seeded prompt {name} should be non-empty"
        );
    }

    // A custom prompt in the injected dir is picked up by the mode loader.
    std::fs::write(dir.join("casual.md"), "INJECTED PROMPT").unwrap();
    let prompt = tinyharness_lib::AgentMode::Casual.load_system_prompt(&dir);
    assert_eq!(prompt, "INJECTED PROMPT");

    // Idempotent: seeding again must not clobber the user's edit.
    tinyharness_lib::ensure_prompts_initialized_in(&dir);
    let prompt = tinyharness_lib::AgentMode::Casual.load_system_prompt(&dir);
    assert_eq!(prompt, "INJECTED PROMPT");
}

#[test]
fn merged_settings_read_project_config_from_injected_root() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    // Global settings in the injected root.
    let global = tinyharness_lib::Settings {
        questions_enabled: true,
        context_limit: Some(1000),
        ..Default::default()
    };
    tinyharness_lib::save_settings_to(&paths, &global).unwrap();

    // No project config yet → global values win.
    let (_, project, merged) = tinyharness_lib::load_merged_settings_from(&paths);
    assert!(project.is_none());
    assert_eq!(merged.context_limit, Some(1000));
    assert!(merged.questions_enabled);

    // Drop a `.tinyharness/config.json` at the injected project root.
    let project_dir = paths.project_config_root();
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"questions_enabled": false, "context_limit": 2000}"#,
    )
    .unwrap();

    let (_, project, merged) = tinyharness_lib::load_merged_settings_from(&paths);
    assert!(project.is_some(), "project config should be discovered");
    assert_eq!(merged.context_limit, Some(2000));
    assert!(!merged.questions_enabled);
    assert_eq!(
        merged.questions_enabled_source,
        tinyharness_lib::SettingSource::Project
    );
}

#[test]
fn sessions_store_uses_injected_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    let store = tinyharness_lib::SessionStore::from_paths(&paths);
    store.ensure_dir().unwrap();

    let dir = paths.sessions_dir();
    assert!(dir.is_dir());
    assert!(
        dir.starts_with(tmp.path()),
        "sessions escaped the injected root: {}",
        dir.display()
    );
    assert_eq!(&dir, store.dir());

    let session = store.create(
        tmp.path().to_str().unwrap(),
        tinyharness_lib::AgentMode::Agent,
        "ollama",
        Some("test-model".to_string()),
    );
    assert!(session.path().starts_with(&dir));
    assert!(session.path().is_file());

    // Reloading from the same Paths finds the session, proving no dependence
    // on the real session directory.
    let reopened = tinyharness_lib::SessionStore::from_paths(&paths);
    let (reloaded, _) = reopened.load(session.id()).expect("session should reload");
    assert_eq!(reloaded.meta().model.as_deref(), Some("test-model"));

    // The default store must NOT be involved.
    assert!(
        !tinyharness_lib::Paths::default()
            .sessions_dir()
            .join(format!("{}.jsonl", session.id()))
            .exists()
    );
}

#[test]
fn workspace_context_collects_from_injected_project_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    let project = paths.project_dir();
    std::fs::create_dir_all(project).unwrap();
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"injected\"\n",
    )
    .unwrap();
    std::fs::write(project.join("TINYHARNESS.md"), "# Injected rules").unwrap();

    let ctx = tinyharness_lib::WorkspaceContext::collect_in(&paths);

    assert_eq!(ctx.root, project);
    assert_eq!(ctx.project_name, "injected");
    assert_eq!(ctx.project_type, "Rust");
    // Instruction file discovered from the injected project, not the CWD.
    let (file_name, content) = ctx.project_md.expect("TINYHARNESS.md should load");
    assert_eq!(file_name, "TINYHARNESS.md");
    assert!(content.contains("Injected rules"));
}

#[test]
fn custom_tools_merge_from_injected_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    let tool_json = |name: &str| {
        format!(
            r#"{{"custom_tools":[{{"name":"{name}","description":"d","category":"readonly","parameters":{{"type":"object","properties":{{}}}},"command":"echo {name}"}}]}}"#
        )
    };

    // Global tool
    let global_file = paths.custom_tools_file();
    std::fs::create_dir_all(global_file.parent().unwrap()).unwrap();
    std::fs::write(&global_file, tool_json("global-tool")).unwrap();

    // Project tool, discovered by walking up from the injected project dir.
    let project_cfg = paths.project_config_root();
    std::fs::create_dir_all(&project_cfg).unwrap();
    std::fs::write(
        project_cfg.join("custom_tools.json"),
        tool_json("project-tool"),
    )
    .unwrap();

    let manager = tinyharness_lib::CustomToolManager::load_from(&paths);
    let names: Vec<&str> = manager
        .custom_tools()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(names.contains(&"global-tool"), "got {names:?}");
    assert!(names.contains(&"project-tool"), "got {names:?}");
}

#[test]
fn skills_discover_from_injected_xdg_legacy_and_project_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = layout(tmp.path());

    fn write_skill(dir: &Path, name: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\n{body}\n"),
        )
        .unwrap();
    }

    // Three distinct personal/project roots, all defining `shared`.
    let xdg = paths.config_dir().join("skills");
    write_skill(&xdg, "shared", "FROM XDG");
    write_skill(&xdg, "xdg-only", "FROM XDG ONLY");

    let legacy = tmp.path().join("home/.tinyharness/skills");
    write_skill(&legacy, "shared", "FROM LEGACY");
    write_skill(&legacy, "legacy-only", "FROM LEGACY ONLY");

    let project = paths.project_skills_dir();
    write_skill(&project, "shared", "FROM PROJECT");

    let skills = tinyharness_lib::discover_skills_in(&paths);
    let find = |name: &str| skills.iter().find(|s| s.name == name);

    // Project overrides both personal sources.
    let shared = find("shared").expect("shared skill should exist");
    assert_eq!(shared.source, tinyharness_lib::SkillSource::Project);
    assert!(shared.content.contains("FROM PROJECT"));

    // Both personal locations remain reachable for names they own alone.
    assert!(
        find("xdg-only").is_some(),
        "XDG personal dir must be scanned"
    );
    assert!(
        find("legacy-only").is_some(),
        "legacy ~/.tinyharness/skills must remain discoverable for existing installs"
    );

    // With no project override, XDG wins over legacy.
    let paths_no_project = layout(tmp.path()).with_project_dir(tmp.path().join("empty-proj"));
    let skills = tinyharness_lib::discover_skills_in(&paths_no_project);
    let shared = skills
        .iter()
        .find(|s| s.name == "shared")
        .expect("shared skill should exist");
    assert!(shared.content.contains("FROM XDG"));
    assert_eq!(shared.source, tinyharness_lib::SkillSource::Personal);

    // Registry entry point honours the same injection.
    let registry = tinyharness_lib::SkillRegistry::discover_in(&paths);
    assert_eq!(registry.skills.len(), 3);
}

#[test]
fn xdg_config_home_is_honoured_for_every_subsystem() {
    let tmp = tempfile::tempdir().unwrap();
    let config_root = tmp.path().join("cfg");
    let data_root = tmp.path().join("data");

    let paths = Paths::new(
        config_root.join("tinyharness"),
        data_root.join("tinyharness"),
        tmp.path().to_path_buf(),
    );

    // Settings and prompts land under the injected config root …
    let global = tinyharness_lib::Settings {
        context_limit: Some(7),
        ..Default::default()
    };
    tinyharness_lib::save_settings_to(&paths, &global).unwrap();
    assert!(paths.settings_file().starts_with(&config_root));
    assert!(tinyharness_lib::ensure_prompts_initialized_for(&paths).starts_with(&config_root));

    // … sessions under the injected data root …
    let store = tinyharness_lib::SessionStore::from_paths(&paths);
    store.ensure_dir().unwrap();
    assert!(paths.sessions_dir().starts_with(&data_root));

    // … and the legacy skills dir is not consulted when there is no home.
    assert_eq!(
        paths.personal_skills_dirs(),
        vec![config_root.join("tinyharness").join("skills")]
    );
}

#[test]
fn image_paths_resolve_against_injected_base_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("shots");
    std::fs::create_dir_all(&base).unwrap();
    // Minimal PNG header is enough — the loader keys off the extension.
    std::fs::write(base.join("pixel.png"), [0x89, b'P', b'N', b'G']).unwrap();

    let img = tinyharness_lib::ImageAttachment::load_from_str_in(&base, "pixel.png")
        .expect("relative image should resolve against base_dir");
    assert_eq!(img.path, base.join("pixel.png"));

    // Absolute paths ignore the base dir.
    let abs = tinyharness_lib::ImageAttachment::load_from_str_in(
        Path::new("/nowhere"),
        base.join("pixel.png").to_str().unwrap(),
    )
    .expect("absolute image path should load");
    assert_eq!(abs.path, base.join("pixel.png"));
}
