use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::Result;
use include_dir::Dir;

pub fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let cursor = Cursor::new(data);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest)?;
    Ok(())
}

pub fn extract_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for file in dir.files() {
        let file_name = file.path().file_name().unwrap_or_default();
        fs::write(dest.join(file_name), file.contents())?;
    }
    for sub in dir.dirs() {
        let sub_name = sub.path().file_name().unwrap_or_default();
        extract_embedded_dir(sub, &dest.join(sub_name))?;
    }
    Ok(())
}


pub fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
