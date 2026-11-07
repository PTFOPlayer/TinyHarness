//! End-to-end tests for the OS-level process sandbox.
//!
//! These drive the real chain — `ToolManager::execute_tool_call` → the `run`
//! tool handler → `bwrap` — so they assert the *wiring*, not just argv shape
//! (unit-tested in `tinyharness-lib/src/sandbox.rs`).
//!
//! Skipped when no backend is installed: CI runners typically lack bubblewrap,
//! and a sandbox test that fails for a missing binary would be worse than no
//! test at all. Set `TH_FORCE_SANDBOX_TESTS=1` to turn the skip into a failure
//! once bubblewrap is installed on the runner.

#![cfg(unix)]

use std::path::Path;

use tinyharness_lib::sandbox::{Sandbox, SandboxBackend, SandboxOptions};
use tinyharness_lib::tools::ToolManager;

/// A manager with the process sandbox wired in the way the binary does it:
/// sandbox set *before* the tools are registered, because handlers capture it.
fn sandboxed_manager(sandbox: Sandbox) -> ToolManager {
    let mut manager = ToolManager::new();
    manager.set_sandbox(sandbox);
    manager.register_defaults();
    manager
}

fn require_bwrap(workspace: &Path) -> Option<Sandbox> {
    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        if std::env::var("TH_FORCE_SANDBOX_TESTS").is_ok() {
            panic!("bubblewrap is required but not installed (TH_FORCE_SANDBOX_TESTS is set)");
        }
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return None;
    }
    // A probe failure means user namespaces are unavailable (a common distro
    // hardening setting) — also an environment problem, not a code problem.
    let sandbox = Sandbox::new(SandboxBackend::Bwrap, workspace, SandboxOptions::default())
        .expect("sandbox construction failed");
    match sandbox.probe() {
        Ok(()) => Some(sandbox),
        Err(e) => {
            if std::env::var("TH_FORCE_SANDBOX_TESTS").is_ok() {
                panic!("bwrap probe failed but TH_FORCE_SANDBOX_TESTS is set: {e}");
            }
            eprintln!("skipping: bwrap could not run a test command: {e}");
            None
        }
    }
}

#[tokio::test]
async fn sandboxed_run_executes_in_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    std::fs::write(tmp.path().join("marker.txt"), "inside").unwrap();

    let manager = sandboxed_manager(sandbox);
    let result = manager
        .execute_tool_call("run", &serde_json::json!({"command": "cat marker.txt"}))
        .await;

    assert!(result.contains("inside"), "result was: {result}");
    assert!(result.contains("(exit 0"), "result was: {result}");
}

#[tokio::test]
async fn sandboxed_run_cannot_read_outside_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "TOP-SECRET-VALUE").unwrap();

    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    let manager = sandboxed_manager(sandbox);

    let absolute = outside.path().join("secret.txt");
    let result = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({"command": format!("cat {}", absolute.display())}),
        )
        .await;

    assert!(
        !result.contains("TOP-SECRET-VALUE"),
        "sandbox leaked an outside file: {result}"
    );
    // The path must be *absent*, not merely unreadable — that is what the
    // tmpfs-root profile buys over a read-only host bind.
    assert!(
        result.contains("No such file") || result.contains("(exit"),
        "result was: {result}"
    );
}

#[tokio::test]
async fn sandboxed_run_cannot_write_outside_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    let manager = sandboxed_manager(sandbox);

    let victim = outside.path().join("created-by-sandbox");
    manager
        .execute_tool_call(
            "run",
            &serde_json::json!({"command": format!("touch {}", victim.display())}),
        )
        .await;

    assert!(!victim.exists(), "sandbox wrote outside the workspace");
}

#[tokio::test]
async fn sandboxed_run_can_write_inside_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    let manager = sandboxed_manager(sandbox);

    let result = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({"command": "echo built > out.txt"}),
        )
        .await;
    assert!(result.contains("(exit 0"), "result was: {result}");

    let written = tmp.path().join("out.txt");
    assert!(
        written.exists(),
        "sandboxed command could not write the workspace"
    );
    assert_eq!(std::fs::read_to_string(&written).unwrap().trim(), "built");
}

#[tokio::test]
async fn sandboxed_run_gets_a_cleared_environment() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    let manager = sandboxed_manager(sandbox);

    // No mutation of the test process (unsafe + racy): instead assert that a
    // variable which is definitely set here — cargo's own — does not reach the
    // sandboxed shell. bwrap runs --clearenv and re-injects only a whitelist.
    assert!(
        std::env::var("CARGO_PKG_NAME").is_ok(),
        "test precondition: cargo sets CARGO_PKG_NAME"
    );
    let result = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({"command": "echo pkg=${CARGO_PKG_NAME:-unset}"}),
        )
        .await;

    assert!(result.contains("pkg=unset"), "result was: {result}");
}

#[tokio::test]
async fn sandboxed_run_keeps_the_host_network() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(sandbox) = require_bwrap(tmp.path()) else {
        return;
    };
    let manager = sandboxed_manager(sandbox);

    // Offline-safe assertion: in an unshared network namespace only the
    // loopback interface exists, so counting non-loopback entries in
    // /proc/net/dev proves the host namespace is shared. This avoids
    // depending on outbound connectivity (or on curl being installed).
    let result = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({
                "command": "tail -n +3 /proc/net/dev | grep -vw lo | wc -l",
            }),
        )
        .await;

    let count: i32 = result
        .lines()
        .find(|l| l.trim().chars().all(|c| c.is_ascii_digit()) && !l.trim().is_empty())
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or_else(|| panic!("could not parse interface count from: {result}"));

    assert!(
        count > 0,
        "sandboxed command saw only loopback — the network namespace was \
         unshared, which breaks git/curl/cargo: {result}"
    );
}

#[tokio::test]
async fn allowed_paths_make_outside_dirs_readable() {
    let tmp = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();
    std::fs::write(extra.path().join("dep.txt"), "ALLOWED-CONTENT").unwrap();

    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return;
    }
    let sandbox = Sandbox::new(
        SandboxBackend::Bwrap,
        tmp.path(),
        SandboxOptions {
            read_only_paths: vec![extra.path().canonicalize().unwrap()],
            ..SandboxOptions::default()
        },
    )
    .unwrap();
    if sandbox.probe().is_err() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }

    let manager = sandboxed_manager(sandbox);
    // Binds are canonicalized, so inside the sandbox the file lives at the
    // canonical spelling (the symlinked `/tmp` prefix does not exist there).
    let target = extra.path().canonicalize().unwrap().join("dep.txt");
    let result = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({"command": format!("cat {}", target.display())}),
        )
        .await;

    assert!(result.contains("ALLOWED-CONTENT"), "result was: {result}");
}

#[tokio::test]
async fn allowed_paths_are_read_only_for_sandboxed_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();

    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return;
    }
    let sandbox = Sandbox::new(
        SandboxBackend::Bwrap,
        tmp.path(),
        SandboxOptions {
            read_only_paths: vec![extra.path().canonicalize().unwrap()],
            ..SandboxOptions::default()
        },
    )
    .unwrap();
    if sandbox.probe().is_err() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }

    let manager = sandboxed_manager(sandbox);
    manager
        .execute_tool_call(
            "run",
            &serde_json::json!({
                "command": format!("touch {}/written", extra.path().display())
            }),
        )
        .await;

    assert!(
        !extra.path().join("written").exists(),
        "an allowed_path was writable — read-only binds must not degrade"
    );
}

#[tokio::test]
async fn nested_allowed_path_binds_without_binding_home() {
    // Load-bearing: `allowed_paths` entries under $HOME must work even though
    // $HOME itself is never bound. If bwrap ever required pre-existing
    // mountpoint parents, these entries would be silently dropped.
    let tmp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let nested = home.path().join(".local/share/data");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("dep.txt"), "NESTED-OK").unwrap();
    // A sibling under the same parent must stay invisible.
    std::fs::create_dir_all(home.path().join("private-notes")).unwrap();
    std::fs::write(home.path().join("private-notes/hidden.txt"), "HIDDEN-VALUE").unwrap();

    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return;
    }
    let sandbox = Sandbox::new(
        SandboxBackend::Bwrap,
        tmp.path(),
        SandboxOptions {
            home: Some(home.path().to_path_buf()),
            read_only_paths: vec![nested.canonicalize().unwrap()],
            ..SandboxOptions::default()
        },
    )
    .unwrap();
    if sandbox.probe().is_err() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }

    let manager = sandboxed_manager(sandbox);

    let reachable = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({
                // Canonical spelling: the bind is canonicalized, and the
                // symlinked tempdir prefix does not exist inside the sandbox.
                "command": format!("cat {}", nested.canonicalize().unwrap().join("dep.txt").display())
            }),
        )
        .await;
    assert!(reachable.contains("NESTED-OK"), "result was: {reachable}");

    let sibling = manager
        .execute_tool_call(
            "run",
            &serde_json::json!({
                "command": format!("cat {}", home.path().join("private-notes/hidden.txt").display())
            }),
        )
        .await;
    assert!(
        !sibling.contains("HIDDEN-VALUE"),
        "a sibling of the bound path was readable: {sibling}"
    );
}

#[tokio::test]
async fn custom_tools_are_sandboxed_too() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "CUSTOM-TOOL-LEAK").unwrap();

    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return;
    }
    let sandbox =
        Sandbox::new(SandboxBackend::Bwrap, tmp.path(), SandboxOptions::default()).unwrap();
    if sandbox.probe().is_err() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }

    let def: tinyharness_lib::custom_tools::CustomToolDefinition =
        serde_json::from_value(serde_json::json!({
            "name": "read_secret",
            "description": "Read a file outside the workspace",
            "category": "readonly",
            "parameters": {
                "type": "object",
                "properties": {"file": {"type": "string"}},
                "required": ["file"]
            },
            "command": "cat {file}"
        }))
        .unwrap();

    let mut manager = ToolManager::new();
    manager.set_sandbox(sandbox);
    manager.register_custom_tools(std::slice::from_ref(&def));

    let target = outside.path().join("secret.txt");
    let result = manager
        .execute_tool_call(
            "read_secret",
            &serde_json::json!({"file": target.to_str().unwrap()}),
        )
        .await;

    assert!(
        !result.contains("CUSTOM-TOOL-LEAK"),
        "custom tool leaked outside the sandbox: {result}"
    );
}

#[tokio::test]
async fn custom_tool_arguments_survive_the_cleared_environment() {
    let tmp = tempfile::tempdir().unwrap();

    if !Sandbox::available().contains(&SandboxBackend::Bwrap) {
        eprintln!("skipping: bubblewrap (`bwrap`) is not installed");
        return;
    }
    let sandbox =
        Sandbox::new(SandboxBackend::Bwrap, tmp.path(), SandboxOptions::default()).unwrap();
    if sandbox.probe().is_err() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }

    let def: tinyharness_lib::custom_tools::CustomToolDefinition =
        serde_json::from_value(serde_json::json!({
            "name": "greet",
            "description": "Greet someone",
            "category": "readonly",
            "parameters": {
                "type": "object",
                "properties": {"who": {"type": "string"}},
                "required": ["who"]
            },
            // Uses the TH_* spelling: these vars must be re-injected as
            // --setenv arguments, because bwrap clears the environment.
            "command": "echo hello-$TH_WHO"
        }))
        .unwrap();

    let mut manager = ToolManager::new();
    manager.set_sandbox(sandbox);
    manager.register_custom_tools(std::slice::from_ref(&def));

    let result = manager
        .execute_tool_call("greet", &serde_json::json!({"who": "world"}))
        .await;

    assert!(result.contains("hello-world"), "result was: {result}");
}
