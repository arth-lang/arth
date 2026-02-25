use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::config::parse_manifest;
use crate::compiler::source::SourceFile;

/// Default maximum source file size (10 MB). Configurable via ARTH_MAX_SOURCE_SIZE.
const DEFAULT_MAX_SOURCE_SIZE: u64 = 10 * 1024 * 1024;

fn max_source_size() -> u64 {
    std::env::var("ARTH_MAX_SOURCE_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_SOURCE_SIZE)
}

pub(crate) fn collect_source_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if root.is_file() {
        if root.extension() == Some(OsStr::new("arth")) {
            files.push(root.to_path_buf());
        }
        return Ok(files);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension() == Some(OsStr::new("arth")) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn load_sources(root: &Path) -> std::io::Result<Vec<SourceFile>> {
    let paths = collect_source_paths(root)?;
    let limit = max_source_size();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        // Check file size before reading to reject oversized files early
        match fs::metadata(&p) {
            Ok(meta) if meta.len() > limit => {
                eprintln!(
                    "error: source file too large: {} ({} bytes, maximum {} bytes)",
                    p.display(),
                    meta.len(),
                    limit
                );
                continue;
            }
            Err(e) => {
                eprintln!("error: failed to read {}: {}", p.display(), e);
                continue;
            }
            _ => {}
        }
        match SourceFile::load_from_path(&p) {
            Ok(sf) => out.push(sf),
            Err(e) => {
                eprintln!("error: failed to read {}: {}", p.display(), e);
            }
        }
    }
    Ok(out)
}

pub(crate) fn read_entry_from_arth_toml(root: &Path) -> Option<PathBuf> {
    let cfg_dir = if root.is_dir() {
        root.to_path_buf()
    } else {
        root.parent()?.to_path_buf()
    };
    let cfg_path = cfg_dir.join("arth.toml");
    let manifest = parse_manifest(&cfg_path).ok()?;
    let rel = manifest.package.entry.as_ref()?;
    Some(cfg_dir.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_entry_from_manifest_entry_field() {
        let tmp_dir = std::env::temp_dir().join("arth_fs_entry_test");
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let manifest_path = tmp_dir.join("arth.toml");
        let manifest_txt = r#"
[package]
name = "arth-sample"
version = "0.1.0"
edition = "2025"
entry = "src/demo/MathDemo.arth"
        "#;
        fs::write(&manifest_path, manifest_txt).expect("write manifest");

        let entry = read_entry_from_arth_toml(&tmp_dir).expect("entry path");
        assert!(
            entry.ends_with("src/demo/MathDemo.arth"),
            "entry path should join cfg_dir and entry field"
        );
    }
}
