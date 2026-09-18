//! One version number across the repo, and this crate's is it.
//!
//! `CARGO_PKG_VERSION` is what the app reports — the About dialog, the macOS
//! About panel, the first line of a crash log — and since stage 6 it is also what
//! `scripts/build-desktop.mjs` names a dmg, an msi and an AppImage after. The two
//! other places a version is written down are not read by anything that ships, so
//! nothing would notice them drifting except a person reading a file and believing
//! it. This notices.

const APP: &str = env!("CARGO_PKG_VERSION");

fn repo(rel: &str) -> String {
    let path = format!("{}/../{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The `version = "…"` of a Cargo.toml's `[package]`, without a TOML parser: it is
/// the first one, before whatever section comes after `[package]`.
fn cargo_version(rel: &str) -> String {
    for line in repo(rel).lines() {
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
    assert_eq!(cargo_version("core/Cargo.toml"), APP, "core/Cargo.toml has drifted from desktop/");
}

#[test]
fn package_json_is_on_the_same_version() {
    // `"version": "0.3.3",` — the only key spelled that way at one indent level in
    // a file this small, and npm writes it in exactly this shape.
    let json = repo("package.json");
    let found = json
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"version\": \"")?.split('"').next())
        .expect("no \"version\" in package.json");
    assert_eq!(found, APP, "package.json has drifted from desktop/Cargo.toml");
}
