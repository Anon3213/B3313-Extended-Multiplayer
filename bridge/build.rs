//! Gives the program its icon.

use std::path::Path;

/// Hands the linker the program's icon, which `scripts/make_icons.py` draws and packs as the
/// linker takes it. Only Microsoft's linker takes such a file, so a build for anything else
/// has no icon.
fn main() {
    let icon = Path::new("../icons/bridge.res");
    println!("cargo::rerun-if-changed={}", icon.display());
    let is = |name: &str, wanted: &str| std::env::var(name).is_ok_and(|value| value == wanted);
    if is("CARGO_CFG_TARGET_OS", "windows") && is("CARGO_CFG_TARGET_ENV", "msvc") {
        // The linker runs elsewhere, so it is told the whole path.
        let folder = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let icon = Path::new(&folder).join(icon);
        println!("cargo::rustc-link-arg-bins={}", icon.display());
    }
}
