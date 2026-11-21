//! Records the commit the panel is built from, for crash reports to name, and gives the
//! program its icon.

use std::path::Path;
use std::process::Command;

/// Hands the linker the program's icon, which `scripts/make_icons.py` draws and packs as the
/// linker takes it. GPUI gives the panel's window the same icon, from the program. Only
/// Microsoft's linker takes such a file, so a build for anything else has no icon.
fn link_icon() {
    let icon = Path::new("../icons/panel.res");
    println!("cargo::rerun-if-changed={}", icon.display());
    let is = |name: &str, wanted: &str| std::env::var(name).is_ok_and(|value| value == wanted);
    if is("CARGO_CFG_TARGET_OS", "windows") && is("CARGO_CFG_TARGET_ENV", "msvc") {
        // The linker runs elsewhere, so it is told the whole path.
        let folder = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let icon = Path::new(&folder).join(icon);
        println!("cargo::rustc-link-arg-bins={}", icon.display());
    }
}

fn main() {
    link_icon();
    // The commit changes when HEAD moves to another branch or its branch gets a new commit.
    for path in ["../.git/HEAD", "../.git/refs/heads", "../.git/packed-refs"] {
        // Cargo runs this again on every build for a path that does not exist.
        if Path::new(path).exists() {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    // A release is built from a copy of the sources that git knows nothing of, so
    // `scripts/release.py` names the commit itself.
    println!("cargo::rerun-if-env-changed=B3313_COMMIT");
    let commit = std::env::var("B3313_COMMIT").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short=10", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
    });
    let commit = commit
        .map(|commit| commit.trim().to_owned())
        .unwrap_or_default();
    println!("cargo::rustc-env=B3313_PANEL_COMMIT={commit}");
}
