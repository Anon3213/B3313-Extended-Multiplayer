//! Records the commit the panel is built from, for crash reports to name.

use std::path::Path;
use std::process::Command;

fn main() {
    // The commit changes when HEAD moves to another branch or its branch gets a new commit.
    for path in ["../.git/HEAD", "../.git/refs/heads", "../.git/packed-refs"] {
        // Cargo runs this again on every build for a path that does not exist.
        if Path::new(path).exists() {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    let commit = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|commit| commit.trim().to_owned())
        .unwrap_or_default();
    println!("cargo::rustc-env=B3313_PANEL_COMMIT={commit}");
}
