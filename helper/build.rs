// Derives HELPER_BUILD_ID from the helper's own sources, so a helper that was not built from the
// same sources as the server's copy of it can be told apart even when nobody bumps the protocol
// version. `scripts/copy-helper.mjs` records the id of the helper it installs; the server compares
// it with what the running helper reports.
use std::fs;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("Cargo.toml"), root.join("Cargo.lock"), root.join("build.rs")];
    collect(&root.join("src"), &mut files);
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in &files {
        let Ok(content) = fs::read(file) else { continue };
        // Names are relative so the same sources give the same id in any checkout location.
        let name = file.strip_prefix(&root).unwrap_or(file).to_string_lossy().replace('\\', "/");
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, &[0]);
        // Line endings do not change what is built, so they do not change the id.
        let normalized: Vec<u8> = content.iter().copied().filter(|b| *b != b'\r').collect();
        fnv1a(&mut hash, &normalized);
        fnv1a(&mut hash, &[0]);
    }
    println!("cargo:rustc-env=HELPER_BUILD_ID={hash:016x}");
    for name in ["src", "Cargo.toml", "Cargo.lock", "build.rs"] {
        println!("cargo:rerun-if-changed={name}");
    }
}
