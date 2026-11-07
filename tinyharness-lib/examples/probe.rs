//! Prints how `settings.json` fragments parse into `SandboxConfig`, including
//! the rejection cases. Run with `cargo run --example probe -p tinyharness-lib`.
//!
//! Three behaviours are worth looking at in the output:
//!
//! 1. Omitting `sandbox` entirely gives the **auto** default (sandboxing is on
//!    unless the user opts out); an explicit `"sandbox": null` is a hard parse
//!    error, so "turn it off" is spelled `{"backend": "off"}`, not `null`.
//! 2. `backend` is required whenever a block is present — a backend-less block
//!    is rejected rather than silently treated as `off`.
//! 3. An unknown key inside the block (here the removed `network` setting) is
//!    silently ignored by serde, so a stale `"network": true` has no effect.

fn main() {
    for json in [
        // Absent → disabled default.
        r#"{}"#,
        // Explicit null → rejected (not the same as absent).
        r#"{"sandbox": null}"#,
        // backend present → accepted.
        r#"{"sandbox": {"backend": "off"}}"#,
        r#"{"sandbox": {"backend": "bwrap", "writable_paths": ["/tmp/x"]}}"#,
        // Backend-less block → rejected, not silently "off".
        r#"{"sandbox": {}}"#,
        r#"{"sandbox": {"writable_paths": ["/tmp/x"]}}"#,
        // Stale `network` key: ignored, so this fails only because backend is
        // missing — and this one parses with the key having no effect.
        r#"{"sandbox": {"network": true}}"#,
        r#"{"sandbox": {"backend": "bwrap", "network": true}}"#,
        // Wrong type → rejected.
        r#"{"sandbox": {"backend": true}}"#,
    ] {
        let r: Result<tinyharness_lib::config::Settings, _> = serde_json::from_str(json);
        match r {
            Ok(s) => println!(
                "OK    {json} -> backend={:?} writable={:?} disabled={}",
                s.sandbox.backend,
                s.sandbox.writable_paths,
                s.sandbox.is_disabled()
            ),
            Err(e) => println!("REJECT {json} -> {e}"),
        }
    }
}
