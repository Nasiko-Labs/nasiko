use std::path::{Path, PathBuf};
use std::{env, fs};

const SKIP_DIRS: &[&str] = &[
    "target",
    ".venv",
    "__pycache__",
    "node_modules",
    ".git",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
];

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let agents_src = PathBuf::from(&manifest_dir).join("../agents");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let dest = out_dir.join("agents");

    if dest.exists() {
        fs::remove_dir_all(&dest).unwrap();
    }
    fs::create_dir_all(&dest).unwrap();

    if agents_src.is_dir() {
        copy_filtered(&agents_src, &dest);
    }

    println!("cargo::rerun-if-changed=../agents");
}

fn copy_filtered(src: &Path, dest: &Path) {
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if path.is_dir() {
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            let child_dest = dest.join(&name);
            fs::create_dir_all(&child_dest).unwrap();
            copy_filtered(&path, &child_dest);
        } else if path.is_file() {
            fs::copy(&path, dest.join(&name)).unwrap();
        }
    }
}
