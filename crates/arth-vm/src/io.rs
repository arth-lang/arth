use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use crate::{Program, decode_program, encode_program, run_program};

pub fn write_abc_file(path: &Path, p: &Program) -> std::io::Result<()> {
    let bytes = encode_program(p);
    let mut f = fs::File::create(path)?;
    f.write_all(&bytes)
}

pub fn run_abc_file(path: &Path) -> Result<i32, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let p = decode_program(&buf)?;
    Ok(run_program(&p))
}
