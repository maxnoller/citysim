//! Build script: a fingerprint of everything that decides what a demo city
//! (`CITYSIM_DEMO`) looks like once it is built — the simulation's sources,
//! the game's scenario code and the locked dependency versions. The demo
//! cache (`dev/scenario.rs`) keys its saved worlds by it, so a simulation
//! change can never load a stale city.

use std::path::{Path, PathBuf};

/// FNV-1a, 64 bit: stable across Rust versions and machines (unlike
/// `DefaultHasher`), so worktrees with the same sources share cache files.
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn main() {
    let game = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = game.join("../..");
    let sim_src = root.join("crates/sim/src");
    let mut files = Vec::new();
    files_under(&sim_src, &mut files);
    for f in [
        "crates/sim/Cargo.toml",
        "Cargo.lock",
        "crates/game/src/dev/scenario.rs",
        "crates/game/src/dev/shapes.rs",
    ] {
        files.push(root.join(f));
    }
    files.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for f in &files {
        // Relative names, so the same sources in two worktrees agree.
        let name = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        fnv(&mut hash, name.as_bytes());
        fnv(&mut hash, &std::fs::read(f).unwrap_or_default());
    }
    println!("cargo:rustc-env=CITYSIM_SIM_FINGERPRINT={hash:016x}");
    // A directory is scanned for any change below it.
    println!("cargo:rerun-if-changed={}", sim_src.display());
    for f in files.iter().filter(|f| !f.starts_with(&sim_src)) {
        println!("cargo:rerun-if-changed={}", f.display());
    }
}
