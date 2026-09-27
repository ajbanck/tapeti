//! One version number across the repo: this crate's `CARGO_PKG_VERSION`.
//!
//! `CARGO_PKG_VERSION` is what ships: the About dialog, the macOS About panel, the
//! first line of a crash log, and what `scripts/build-desktop.mjs` names a dmg, an
//! msi and an AppImage after. `core/Cargo.toml` and `package.json` carry the same
//! number, but nothing that ships reads them, so only this test catches them
//! drifting.

const APP: &str = env!("CARGO_PKG_VERSION");

/// The `version = "…"` of a Cargo.toml's `[package]`, without a TOML parser: it is
/// the first one, before whatever section comes after `[package]`.
fn cargo_version(toml: &str, rel: &str) -> String {
    for line in toml.lines() {
        if let Some(v) = line.strip_prefix("version = \"").and_then(|r| r.split('"').next()) {
            return v.to_string();
        }
        if line.starts_with('[') && line != "[package]" {
            break;
        }
    }
    panic!("no [package] version in {rel}");
}

#[test]
fn the_core_is_on_the_same_version() {
    assert_eq!(
        cargo_version(include_str!("../../core/Cargo.toml"), "core/Cargo.toml"),
        APP,
        "core/Cargo.toml has drifted from desktop/"
    );
}

#[test]
fn package_json_is_on_the_same_version() {
    // Matches the `"version": "…",` line, the only key npm writes in exactly that shape
    // at one indent level in a file this small.
    let json = include_str!("../../package.json");
    let found = json
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"version\": \"")?.split('"').next())
        .expect("no \"version\" in package.json");
    assert_eq!(found, APP, "package.json has drifted from desktop/Cargo.toml");
}
