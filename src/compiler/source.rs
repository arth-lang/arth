use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    // Byte offsets
    pub start: usize,
    pub end: usize,
    // Line/column (1-based) for nicer diagnostics
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Span {
    #[allow(dead_code)]
    pub fn new(start: usize, end: usize) -> Self {
        // Fallback constructor when line/column is not known; kept for compatibility
        Self {
            start,
            end,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

impl SourceFile {
    pub fn load_from_path(path: &Path) -> std::io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            text,
        })
    }
}
