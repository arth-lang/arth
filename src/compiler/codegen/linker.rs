//! Linker Integration for Native Compilation
//!
//! This module handles the final step of native compilation: linking LLVM IR
//! into executable binaries. It supports two backends:
//!
//! 1. **Clang** - Uses clang to compile IR directly to executable
//! 2. **LLVM Toolchain** - Uses llc + platform linker (ld.lld, ld64.lld, link.exe)
//!
//! # Usage
//!
//! ```ignore
//! let config = LinkerConfig {
//!     llvm_ir_path: PathBuf::from("output.ll"),
//!     output_path: PathBuf::from("output"),
//!     arth_rt_lib_path: PathBuf::from("/usr/local/lib/arth"),
//!     target_triple: "x86_64-apple-darwin".to_string(),
//!     optimization_level: 2,
//!     backend: LinkerBackend::Clang,
//! };
//!
//! compile_and_link(&config)?;
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

// =============================================================================
// Types and Configuration
// =============================================================================

/// The linker backend to use for compilation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkerBackend {
    /// Use clang to compile IR directly to executable.
    /// This is the simplest and most reliable option.
    Clang,

    /// Use llc to compile to object file, then platform linker.
    /// Requires: llc, and one of: ld.lld (Linux), ld64.lld (macOS), link.exe (Windows)
    LlvmDirect,
}

/// Configuration for the linker.
#[derive(Clone, Debug)]
pub struct LinkerConfig {
    /// Path to the LLVM IR file (.ll).
    pub llvm_ir_path: PathBuf,

    /// Path to the output executable.
    pub output_path: PathBuf,

    /// Path to the directory containing libarth_rt.
    pub arth_rt_lib_path: PathBuf,

    /// Target triple (e.g., "x86_64-apple-darwin", "x86_64-unknown-linux-gnu").
    pub target_triple: String,

    /// Optimization level (0-3).
    pub optimization_level: u8,

    /// Which linker backend to use.
    pub backend: LinkerBackend,

    /// Whether to emit debug information.
    pub debug_info: bool,

    /// Whether to link statically.
    pub static_link: bool,
}

impl Default for LinkerConfig {
    fn default() -> Self {
        Self {
            llvm_ir_path: PathBuf::new(),
            output_path: PathBuf::new(),
            arth_rt_lib_path: find_arth_rt_lib_path(),
            target_triple: detect_target_triple(),
            optimization_level: 2,
            backend: LinkerBackend::Clang,
            debug_info: false,
            static_link: false,
        }
    }
}

/// Errors that can occur during linking.
#[derive(Debug)]
pub enum LinkerError {
    /// The required tool was not found.
    ToolNotFound(String),

    /// The tool execution failed.
    ToolFailed {
        tool: String,
        status: i32,
        stderr: String,
    },

    /// IO error.
    Io(std::io::Error),

    /// The LLVM IR file was not found.
    IrFileNotFound(PathBuf),

    /// The arth-rt library was not found.
    RuntimeNotFound(PathBuf),

    /// Unsupported platform.
    UnsupportedPlatform(String),

    /// The tool version is too old.
    ToolVersionTooOld {
        tool: String,
        found: u32,
        minimum: u32,
    },
}

impl std::fmt::Display for LinkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkerError::ToolNotFound(tool) => {
                write!(
                    f,
                    "Required tool not found: {}. Install clang or LLVM toolchain.",
                    tool
                )
            }
            LinkerError::ToolFailed {
                tool,
                status,
                stderr,
            } => {
                write!(f, "{} failed with exit code {}:\n{}", tool, status, stderr)
            }
            LinkerError::Io(e) => write!(f, "IO error: {}", e),
            LinkerError::IrFileNotFound(path) => {
                write!(f, "LLVM IR file not found: {}", path.display())
            }
            LinkerError::RuntimeNotFound(path) => {
                write!(
                    f,
                    "arth-rt library not found at: {}. Build with: cargo build -p arth-rt --release",
                    path.display()
                )
            }
            LinkerError::UnsupportedPlatform(platform) => {
                write!(f, "Unsupported platform: {}", platform)
            }
            LinkerError::ToolVersionTooOld {
                tool,
                found,
                minimum,
            } => {
                write!(
                    f,
                    "{} version {} is too old. Minimum required: {}. Please upgrade.",
                    tool, found, minimum
                )
            }
        }
    }
}

impl std::error::Error for LinkerError {}

impl From<std::io::Error> for LinkerError {
    fn from(e: std::io::Error) -> Self {
        LinkerError::Io(e)
    }
}

// =============================================================================
// Tool Detection
// =============================================================================

/// Check if a command is available in PATH.
fn which(cmd: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    let path_var = std::env::var_os("PATH")?;

    #[cfg(unix)]
    for dir in std::env::split_paths(&path_var) {
        let full_path = dir.join(cmd);
        if full_path.is_file() {
            return Some(full_path);
        }
    }

    #[cfg(windows)]
    {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let full_path = dir.join(format!("{}.exe", cmd));
            if full_path.is_file() {
                return Some(full_path);
            }
        }
    }

    None
}

/// Minimum required clang major version.
/// Clang 14+ is required for full DWARF exception handling and
/// personality function support used by the Arth native backend.
const MIN_CLANG_VERSION: u32 = 14;

/// Parse the major version number from `clang --version` output.
/// Handles both Apple clang and standard LLVM clang formats:
/// - "Apple clang version 15.0.0 (clang-1500.3.9.4)"
/// - "clang version 17.0.6"
fn parse_clang_major_version(version_output: &str) -> Option<u32> {
    let idx = version_output.find("version ")?;
    let after = &version_output[idx + 8..];
    let major_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    major_str.parse().ok()
}

/// Check if a clang major version meets the minimum requirement.
fn is_clang_version_sufficient(major: u32) -> bool {
    major >= MIN_CLANG_VERSION
}

/// Validate that the installed clang meets version requirements.
/// Returns Ok(()) if sufficient, Err with diagnostic if too old.
fn validate_clang_version() -> Result<(), LinkerError> {
    let output = Command::new("clang")
        .arg("--version")
        .output()
        .map_err(|_| LinkerError::ToolNotFound("clang".to_string()))?;

    let version_text = String::from_utf8_lossy(&output.stdout);
    match parse_clang_major_version(&version_text) {
        Some(major) if is_clang_version_sufficient(major) => Ok(()),
        Some(major) => Err(LinkerError::ToolVersionTooOld {
            tool: "clang".to_string(),
            found: major,
            minimum: MIN_CLANG_VERSION,
        }),
        None => Ok(()), // Can't parse version — proceed optimistically
    }
}

/// Ordered list of candidate linkers for Linux (highest priority first).
///
/// - `ld.lld` — LLVM's lld (fast, wide format support)
/// - `mold`   — modern high-speed linker
/// - `ld.gold` — GNU gold (faster than GNU ld)
/// - `ld`     — GNU ld (universal fallback)
pub const LINUX_LINKER_CANDIDATES: &[&str] = &["ld.lld", "mold", "ld.gold", "ld"];

/// Detect the best available linker backend.
pub fn detect_available_linker() -> Result<LinkerBackend, LinkerError> {
    // Prefer clang as it's simpler
    if which("clang").is_some() {
        validate_clang_version()?;
        return Ok(LinkerBackend::Clang);
    }

    // Check for LLVM toolchain
    if which("llc").is_some() {
        #[cfg(target_os = "linux")]
        if LINUX_LINKER_CANDIDATES.iter().any(|c| which(c).is_some()) {
            return Ok(LinkerBackend::LlvmDirect);
        }

        #[cfg(target_os = "macos")]
        if which("ld64.lld").is_some() || which("ld").is_some() {
            return Ok(LinkerBackend::LlvmDirect);
        }

        #[cfg(target_os = "windows")]
        if which("link").is_some() || which("lld-link").is_some() {
            return Ok(LinkerBackend::LlvmDirect);
        }
    }

    Err(LinkerError::ToolNotFound("clang or llc+lld".to_string()))
}

/// Detect the current host's target triple.
///
/// This is used both by the linker configuration and by the LLVM IR emitter
/// to produce target-aware module metadata (datalayout, triple).
pub fn detect_target_triple() -> String {
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin".to_string();

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin".to_string();

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-unknown-linux-gnu".to_string();

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-unknown-linux-gnu".to_string();

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "x86_64-pc-windows-msvc".to_string();

    #[cfg(not(any(
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    return "unknown-unknown-unknown".to_string();
}

/// Find the arth-rt library path.
fn find_arth_rt_lib_path() -> PathBuf {
    // Check environment variable first
    if let Ok(path) = std::env::var("ARTH_RT_LIB") {
        return PathBuf::from(path);
    }

    // Check relative to the executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // Development: target/debug or target/release
            let dev_path = exe_dir.join("deps");
            if dev_path.exists() {
                return dev_path;
            }

            // Installed: same directory as executable
            return exe_dir.to_path_buf();
        }
    }

    // Fallback to standard locations
    #[cfg(unix)]
    return PathBuf::from("/usr/local/lib/arth");

    #[cfg(windows)]
    return PathBuf::from("C:\\arth\\lib");
}

// =============================================================================
// Linking Functions
// =============================================================================

/// Compile and link LLVM IR to a native binary.
pub fn compile_and_link(config: &LinkerConfig) -> Result<(), LinkerError> {
    // Validate inputs
    if !config.llvm_ir_path.exists() {
        return Err(LinkerError::IrFileNotFound(config.llvm_ir_path.clone()));
    }

    match config.backend {
        LinkerBackend::Clang => link_with_clang(config),
        LinkerBackend::LlvmDirect => link_with_llvm_tools(config),
    }
}

/// Link using clang (preferred method).
fn link_with_clang(config: &LinkerConfig) -> Result<(), LinkerError> {
    let clang_path =
        which("clang").ok_or_else(|| LinkerError::ToolNotFound("clang".to_string()))?;

    let mut cmd = Command::new(&clang_path);

    // Output file
    cmd.arg("-o").arg(&config.output_path);

    // Input LLVM IR
    cmd.arg(&config.llvm_ir_path);

    // Optimization level
    cmd.arg(format!("-O{}", config.optimization_level));

    // Debug info
    if config.debug_info {
        cmd.arg("-g");
    }

    // Link arth-rt library
    cmd.arg("-L").arg(&config.arth_rt_lib_path);
    cmd.arg("-larth_rt");

    // System libraries (platform-specific)
    #[cfg(target_os = "linux")]
    {
        cmd.arg("-lpthread");
        cmd.arg("-ldl");
        cmd.arg("-lm");
    }

    #[cfg(target_os = "macos")]
    {
        cmd.arg("-lpthread");
        cmd.arg("-lSystem");
        // Framework for crypto operations
        cmd.arg("-framework").arg("Security");
    }

    #[cfg(target_os = "windows")]
    {
        // Windows uses different library names
        cmd.arg("-lkernel32");
        cmd.arg("-luser32");
        cmd.arg("-lws2_32");

        // Force DWARF exception handling on Windows.  Our LLVM IR emits
        // Itanium-style landingpad / personality instructions.  Clang on
        // Windows defaults to SEH, which uses a completely different IR
        // representation (catchswitch/catchpad).  With these flags clang
        // will use the DWARF unwinder instead, keeping our IR valid.
        cmd.arg("-fexceptions");
        cmd.arg("-fdwarf-exceptions");
    }

    // Static linking
    if config.static_link {
        #[cfg(target_os = "linux")]
        cmd.arg("-static");

        #[cfg(target_os = "macos")]
        {
            // macOS doesn't support fully static binaries
            cmd.arg("-static-libstdc++");
        }
    }

    // Execute
    let output = cmd.output()?;

    if !output.status.success() {
        return Err(LinkerError::ToolFailed {
            tool: "clang".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

/// Link using LLVM tools (llc + platform linker).
fn link_with_llvm_tools(config: &LinkerConfig) -> Result<(), LinkerError> {
    let llc_path = which("llc").ok_or_else(|| LinkerError::ToolNotFound("llc".to_string()))?;

    // Step 1: Compile IR to object file using llc
    let obj_path = config.output_path.with_extension("o");

    let mut llc_cmd = Command::new(&llc_path);
    llc_cmd.arg("-filetype=obj");
    llc_cmd.arg(format!("-O{}", config.optimization_level));

    if !config.target_triple.is_empty() {
        llc_cmd.arg("-mtriple").arg(&config.target_triple);
    }

    llc_cmd.arg("-o").arg(&obj_path);
    llc_cmd.arg(&config.llvm_ir_path);

    let llc_output = llc_cmd.output()?;

    if !llc_output.status.success() {
        return Err(LinkerError::ToolFailed {
            tool: "llc".to_string(),
            status: llc_output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&llc_output.stderr).to_string(),
        });
    }

    // Step 2: Link object file to executable using platform linker
    let link_result = link_object_file(&obj_path, config);

    // Clean up object file
    let _ = std::fs::remove_file(&obj_path);

    link_result
}

/// Link an object file to an executable using the platform linker.
fn link_object_file(obj_path: &Path, config: &LinkerConfig) -> Result<(), LinkerError> {
    #[cfg(target_os = "linux")]
    {
        link_with_linker_linux(obj_path, config)
    }

    #[cfg(target_os = "macos")]
    {
        link_with_ld_macos(obj_path, config)
    }

    #[cfg(target_os = "windows")]
    {
        link_with_link_windows(obj_path, config)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(LinkerError::UnsupportedPlatform(
            std::env::consts::OS.to_string(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn link_with_linker_linux(obj_path: &Path, config: &LinkerConfig) -> Result<(), LinkerError> {
    // Detection order follows LINUX_LINKER_CANDIDATES priority.
    let linker_path = LINUX_LINKER_CANDIDATES
        .iter()
        .find_map(|c| which(c))
        .ok_or_else(|| LinkerError::ToolNotFound(LINUX_LINKER_CANDIDATES.join(", ").to_string()))?;

    let linker_name = linker_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("ld")
        .to_string();

    let mut cmd = Command::new(&linker_path);

    cmd.arg("-o").arg(&config.output_path);
    cmd.arg(obj_path);

    // Link arth-rt
    cmd.arg("-L").arg(&config.arth_rt_lib_path);
    cmd.arg("-larth_rt");

    // System libraries
    cmd.arg("-lpthread");
    cmd.arg("-ldl");
    cmd.arg("-lm");
    cmd.arg("-lc");

    // Dynamic linker
    if !config.static_link {
        cmd.arg("-dynamic-linker")
            .arg("/lib64/ld-linux-x86-64.so.2");
    }

    // CRT startup files
    cmd.arg("/usr/lib/x86_64-linux-gnu/crt1.o");
    cmd.arg("/usr/lib/x86_64-linux-gnu/crti.o");
    cmd.arg("/usr/lib/x86_64-linux-gnu/crtn.o");

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(LinkerError::ToolFailed {
            tool: linker_name,
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn link_with_ld_macos(obj_path: &Path, config: &LinkerConfig) -> Result<(), LinkerError> {
    // macOS uses ld64 (system linker)
    let linker_path = which("ld")
        .or_else(|| which("ld64.lld"))
        .ok_or_else(|| LinkerError::ToolNotFound("ld".to_string()))?;

    let mut cmd = Command::new(&linker_path);

    cmd.arg("-o").arg(&config.output_path);
    cmd.arg(obj_path);

    // Link arth-rt
    cmd.arg("-L").arg(&config.arth_rt_lib_path);
    cmd.arg("-larth_rt");

    // System libraries
    cmd.arg("-lSystem");
    cmd.arg("-lpthread");

    // macOS SDK (required for system library resolution)
    if let Ok(sdk_path) = std::process::Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
    {
        if sdk_path.status.success() {
            let sdk = String::from_utf8_lossy(&sdk_path.stdout).trim().to_string();
            cmd.arg("-syslibroot").arg(&sdk);
            cmd.arg("-L").arg(format!("{}/usr/lib", sdk));
        }
    }

    // Architecture
    #[cfg(target_arch = "x86_64")]
    cmd.arg("-arch").arg("x86_64");

    #[cfg(target_arch = "aarch64")]
    cmd.arg("-arch").arg("arm64");

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(LinkerError::ToolFailed {
            tool: "ld".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn link_with_link_windows(obj_path: &Path, config: &LinkerConfig) -> Result<(), LinkerError> {
    // Windows uses link.exe (MSVC) or lld-link
    let linker_path = which("lld-link")
        .or_else(|| which("link"))
        .ok_or_else(|| LinkerError::ToolNotFound("link.exe or lld-link".to_string()))?;

    let mut cmd = Command::new(&linker_path);

    cmd.arg(format!("/OUT:{}", config.output_path.display()));
    cmd.arg(obj_path);

    // Link arth-rt
    cmd.arg(format!("/LIBPATH:{}", config.arth_rt_lib_path.display()));
    cmd.arg("arth_rt.lib");

    // System libraries
    cmd.arg("kernel32.lib");
    cmd.arg("user32.lib");
    cmd.arg("ws2_32.lib");
    cmd.arg("msvcrt.lib");

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(LinkerError::ToolFailed {
            tool: "link".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(())
}

// =============================================================================
// High-Level API
// =============================================================================

/// Write LLVM IR text to a file.
pub fn write_llvm_ir(ir_text: &str, path: &Path) -> Result<(), LinkerError> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(ir_text.as_bytes())?;
    Ok(())
}

/// Compile LLVM IR text to a native binary.
///
/// This is the main entry point for the linker module.
/// `opt_level` controls the LLVM optimization level (0–3); values above 3 are clamped.
pub fn compile_ir_to_native(
    ir_text: &str,
    output_path: &Path,
    arth_rt_path: Option<&Path>,
    opt_level: u8,
    debug: bool,
) -> Result<(), LinkerError> {
    // Detect available linker
    let backend = detect_available_linker()?;

    // Create temporary file for IR
    let ir_path = output_path.with_extension("ll");
    write_llvm_ir(ir_text, &ir_path)?;

    // Configure linker
    let config = LinkerConfig {
        llvm_ir_path: ir_path.clone(),
        output_path: output_path.to_path_buf(),
        arth_rt_lib_path: arth_rt_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(find_arth_rt_lib_path),
        target_triple: detect_target_triple(),
        optimization_level: opt_level.min(3),
        backend,
        debug_info: debug,
        static_link: false,
    };

    // Compile and link
    let result = compile_and_link(&config);

    // Clean up IR file
    let _ = std::fs::remove_file(&ir_path);

    result
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_target_triple() {
        let triple = detect_target_triple();
        assert!(!triple.is_empty());
        assert!(triple.contains('-'));
    }

    #[test]
    fn test_which_clang() {
        // This test may fail on systems without clang installed
        let result = which("clang");
        if result.is_some() {
            println!("Found clang at: {:?}", result.unwrap());
        }
    }

    #[test]
    fn test_detect_available_linker() {
        // This test may fail on systems without a linker installed
        match detect_available_linker() {
            Ok(backend) => println!("Detected linker backend: {:?}", backend),
            Err(e) => println!("No linker found: {}", e),
        }
    }

    #[test]
    fn test_linker_config_default() {
        let config = LinkerConfig::default();
        assert_eq!(config.optimization_level, 2);
        assert_eq!(config.backend, LinkerBackend::Clang);
        assert!(!config.debug_info);
        assert!(!config.static_link);
    }

    #[test]
    fn test_parse_clang_version_apple() {
        assert_eq!(
            parse_clang_major_version("Apple clang version 15.0.0 (clang-1500.3.9.4)"),
            Some(15)
        );
    }

    #[test]
    fn test_parse_clang_version_llvm() {
        assert_eq!(parse_clang_major_version("clang version 17.0.6"), Some(17));
    }

    #[test]
    fn test_parse_clang_version_old() {
        assert_eq!(parse_clang_major_version("clang version 10.0.0"), Some(10));
    }

    #[test]
    fn test_parse_clang_version_not_clang() {
        assert_eq!(parse_clang_major_version("gcc (GCC) 12.0"), None);
    }

    #[test]
    fn test_clang_version_sufficient() {
        assert!(is_clang_version_sufficient(15));
        assert!(is_clang_version_sufficient(14));
        assert!(!is_clang_version_sufficient(13));
        assert!(!is_clang_version_sufficient(10));
    }

    #[test]
    fn test_linux_linker_candidates_order() {
        // ld.lld (LLVM lld) should be highest priority, ld (GNU ld) lowest.
        let candidates = LINUX_LINKER_CANDIDATES;
        assert_eq!(candidates[0], "ld.lld", "lld should be first choice");
        assert_eq!(candidates[1], "mold", "mold should be second choice");
        assert_eq!(candidates[2], "ld.gold", "gold should be third choice");
        assert_eq!(
            *candidates.last().unwrap(),
            "ld",
            "GNU ld should be fallback"
        );
    }

    #[test]
    fn test_linux_linker_candidates_not_empty() {
        // Validate constant has entries — length check avoids const_is_empty lint.
        assert!(
            LINUX_LINKER_CANDIDATES.len() > 0,
            "must have at least one Linux linker candidate"
        );
    }

    #[test]
    fn test_linux_linker_candidates_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for c in LINUX_LINKER_CANDIDATES {
            assert!(seen.insert(c), "duplicate linker candidate: {c}");
        }
    }
}
