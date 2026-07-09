use std::fs;
use std::path::Path;

/// Stage `../agents` into OUT_DIR for `include_dir!`, skipping build artifacts.
///
/// The CLI embeds the agents directory as scaffold templates (`nasiko new`)
/// and skills. Building an agent in-place (`cargo build` inside oss/agents/*)
/// leaves a multi-GB `target/` dir there; embedding it produces a binary too
/// large to load (its segments collide with the dyld shared-cache region on
/// macOS). Copying with `target/` filtered out keeps the embed template-only.
fn copy_filtered(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let ty = entry.file_type()?;
        let to = dst.join(&name);
        if ty.is_dir() {
            copy_filtered(&entry.path(), &to)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = std::env::var("OUT_DIR").unwrap();
    let src = Path::new(&manifest).join("..").join("agents");
    let dst = Path::new(&out).join("agents");

    println!("cargo:rerun-if-changed={}", src.display());

    let _ = fs::remove_dir_all(&dst);
    copy_filtered(&src, &dst).expect("failed to stage agent templates for embedding");
}
