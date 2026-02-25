use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use arth::compiler::hir::{HirDecl, HirFile};
use arth_vm::{Program, decode_library_detailed, encode_program};
use serde::{Deserialize, Serialize};

pub const TS_GUEST_MANIFEST_VERSION: u32 = 1;
pub const TS_GUEST_SCHEMA_VERSION: &str = "ts-guest-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsGuestExport {
    /// Local name of the export
    pub name: String,
    /// Kind of export (func, struct, enum, interface, provider)
    pub kind: String,
    /// Fully-qualified name (Module.name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    /// Arity for functions (None for non-functions)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arity: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TsGuestImportKind {
    HostCapability,
    LocalModule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsGuestImportItem {
    pub local: String,
    pub canonical: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsGuestImport {
    pub source: String,
    pub kind: TsGuestImportKind,
    pub items: Vec<TsGuestImportItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsGuestManifest {
    pub format_version: u32,
    pub schema_version: String,
    pub language: String,
    pub module_name: String,
    pub package: Option<String>,
    pub entry: Option<String>,
    pub exports: Vec<TsGuestExport>,
    pub imports: Vec<TsGuestImport>,
    /// TS surface capabilities (e.g., "log", "time", "math")
    pub capabilities: Vec<String>,
    /// VM host capabilities required for execution (e.g., "io", "net", "time")
    pub host_capabilities: Vec<String>,
    /// Relative path to the compiled VM bytecode file for this guest.
    pub bytecode: String,
}

fn parse_ts_import_note(message: &str) -> Option<(String, String, String)> {
    // Expected shapes:
    // ts-import:named local=foo source=arth:log canonical=arth.log.Logger
    // ts-import:default local=Foo source=arth:log canonical=arth.log
    // ts-import:namespace local=log source=arth:log canonical=arth.log
    let rest = message.strip_prefix("ts-import:")?;
    let mut parts = rest.split_whitespace();
    let _kind = parts.next()?; // named/default/namespace (currently unused)
    let mut local = None;
    let mut source = None;
    let mut canonical = None;
    for part in parts {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "local" => local = Some(v.to_string()),
                "source" => source = Some(v.to_string()),
                "canonical" => canonical = Some(v.to_string()),
                _ => {}
            }
        }
    }
    let local = local?;
    let source = source?;
    let canonical = canonical?;
    Some((local, source, canonical))
}

/// Map a TS surface module to the required VM host capabilities.
///
/// Returns a list of host capability names ("io", "net", "time") that the
/// VM must have enabled to run code using this module.
fn ts_import_to_host_capabilities(source: &str) -> Vec<&'static str> {
    match source {
        "arth:log" => vec!["io"],     // log.emit uses HostIo (console write)
        "arth:time" => vec!["time"],  // time.* uses HostTime
        "arth:fs" => vec!["io"],      // file system uses HostIo
        "arth:console" => vec!["io"], // console I/O uses HostIo
        "arth:http" => vec!["net"],   // HTTP uses HostNet
        "arth:ws" => vec!["net"],     // WebSocket uses HostNet
        "arth:sse" => vec!["net"],    // SSE uses HostNet
        "arth:net" => vec!["net"],    // generic networking uses HostNet
        // Pure modules - no host capabilities required
        "arth:math" => vec![],
        "arth:rand" => vec![],
        "arth:array" => vec![],
        "arth:map" => vec![],
        "arth:option" => vec![],
        "arth:result" => vec![],
        "arth:string" => vec![],
        "arth:json" => vec![],
        _ => vec![], // Unknown modules default to no capabilities
    }
}

fn build_imports_and_capabilities(hir: &HirFile) -> (Vec<TsGuestImport>, Vec<String>, Vec<String>) {
    let mut by_source: BTreeMap<String, TsGuestImport> = BTreeMap::new();
    let mut caps: BTreeSet<String> = BTreeSet::new();
    let mut host_caps: BTreeSet<String> = BTreeSet::new();

    for note in &hir.notes {
        if let Some((local, source, canonical)) = parse_ts_import_note(&note.message) {
            let kind = if let Some(cap) = source.strip_prefix("arth:") {
                if !cap.is_empty() {
                    caps.insert(cap.to_string());
                }
                // Map TS surface import to VM host capabilities
                for hc in ts_import_to_host_capabilities(&source) {
                    host_caps.insert(hc.to_string());
                }
                TsGuestImportKind::HostCapability
            } else {
                TsGuestImportKind::LocalModule
            };

            let entry = by_source
                .entry(source.clone())
                .or_insert_with(|| TsGuestImport {
                    source: source.clone(),
                    kind,
                    items: Vec::new(),
                });
            entry.items.push(TsGuestImportItem { local, canonical });
        }
    }

    let imports = by_source.into_values().collect();
    let capabilities = caps.into_iter().collect();
    let host_capabilities = host_caps.into_iter().collect();
    (imports, capabilities, host_capabilities)
}

fn build_exports(hir: &HirFile) -> (Vec<TsGuestExport>, Option<String>, String) {
    let mut exports = Vec::new();
    let mut entry: Option<String> = None;
    let mut module_name = "Main".to_string();

    for decl in &hir.decls {
        match decl {
            HirDecl::Module(m) => {
                if exports.is_empty() {
                    module_name = m.name.clone();
                }
                if m.is_exported {
                    for f in &m.funcs {
                        let name = f.sig.name.clone();
                        let arity = f.sig.params.len() as u8;
                        if entry.is_none() && name == "main" {
                            entry = Some(name.clone());
                        }
                        exports.push(TsGuestExport {
                            qualified_name: Some(format!("{}.{}", m.name, name)),
                            name,
                            kind: "func".to_string(),
                            arity: Some(arity),
                        });
                    }
                }
            }
            HirDecl::Struct(s) => {
                exports.push(TsGuestExport {
                    qualified_name: Some(format!("{}.{}", module_name, s.name)),
                    name: s.name.clone(),
                    kind: "struct".to_string(),
                    arity: None,
                });
            }
            HirDecl::Enum(e) => {
                exports.push(TsGuestExport {
                    qualified_name: Some(format!("{}.{}", module_name, e.name)),
                    name: e.name.clone(),
                    kind: "enum".to_string(),
                    arity: None,
                });
            }
            HirDecl::Interface(i) => {
                exports.push(TsGuestExport {
                    qualified_name: Some(format!("{}.{}", module_name, i.name)),
                    name: i.name.clone(),
                    kind: "interface".to_string(),
                    arity: None,
                });
            }
            HirDecl::Provider(p) => {
                exports.push(TsGuestExport {
                    qualified_name: Some(format!("{}.{}", module_name, p.name)),
                    name: p.name.clone(),
                    kind: "provider".to_string(),
                    arity: None,
                });
            }
            _ => {}
        }
    }

    (exports, entry, module_name)
}

fn build_ts_guest_manifest_internal(hir: &HirFile, bytecode_rel_path: String) -> TsGuestManifest {
    let (imports, capabilities, host_capabilities) = build_imports_and_capabilities(hir);
    let (exports, entry, module_name) = build_exports(hir);
    let package = hir.package.as_ref().map(|p| p.to_string());

    TsGuestManifest {
        format_version: TS_GUEST_MANIFEST_VERSION,
        schema_version: TS_GUEST_SCHEMA_VERSION.to_string(),
        language: "ts".to_string(),
        module_name,
        package,
        entry,
        exports,
        imports,
        capabilities,
        host_capabilities,
        bytecode: bytecode_rel_path,
    }
}

pub fn write_ts_guest_package(
    hir: &HirFile,
    program: &Program,
    out_dir: &Path,
    base_name: &str,
) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(out_dir)?;

    let bytecode_name = format!("{base_name}.abc");
    let manifest_name = format!("{base_name}.tsguest.json");

    let bytecode_path = out_dir.join(&bytecode_name);
    let manifest_path = out_dir.join(&manifest_name);

    let bytes = encode_program(program);
    std::fs::write(&bytecode_path, bytes)?;

    let manifest = build_ts_guest_manifest_internal(hir, bytecode_name);
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, json)?;

    Ok((manifest_path, bytecode_path))
}

pub fn load_ts_guest_package(manifest_path: &Path) -> std::io::Result<(TsGuestManifest, Program)> {
    let text = std::fs::read_to_string(manifest_path)?;
    let manifest: TsGuestManifest = serde_json::from_str(&text)?;
    if manifest.format_version != TS_GUEST_MANIFEST_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unsupported TS guest manifest version {}; expected {}",
                manifest.format_version, TS_GUEST_MANIFEST_VERSION
            ),
        ));
    }

    let bytecode_path = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&manifest.bytecode);
    let bytes = std::fs::read(&bytecode_path)?;
    let (program, _exports) = decode_library_detailed(&bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to decode VM library: {e}"),
        )
    })?;

    Ok((manifest, program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_import_to_host_capabilities_io() {
        // Modules that require IO capability
        assert_eq!(ts_import_to_host_capabilities("arth:log"), vec!["io"]);
        assert_eq!(ts_import_to_host_capabilities("arth:fs"), vec!["io"]);
        assert_eq!(ts_import_to_host_capabilities("arth:console"), vec!["io"]);
    }

    #[test]
    fn test_ts_import_to_host_capabilities_net() {
        // Modules that require Net capability
        assert_eq!(ts_import_to_host_capabilities("arth:http"), vec!["net"]);
        assert_eq!(ts_import_to_host_capabilities("arth:ws"), vec!["net"]);
        assert_eq!(ts_import_to_host_capabilities("arth:sse"), vec!["net"]);
        assert_eq!(ts_import_to_host_capabilities("arth:net"), vec!["net"]);
    }

    #[test]
    fn test_ts_import_to_host_capabilities_time() {
        // Modules that require Time capability
        assert_eq!(ts_import_to_host_capabilities("arth:time"), vec!["time"]);
    }

    #[test]
    fn test_ts_import_to_host_capabilities_pure() {
        // Pure modules - no host capabilities required
        assert!(ts_import_to_host_capabilities("arth:math").is_empty());
        assert!(ts_import_to_host_capabilities("arth:rand").is_empty());
        assert!(ts_import_to_host_capabilities("arth:array").is_empty());
        assert!(ts_import_to_host_capabilities("arth:map").is_empty());
        assert!(ts_import_to_host_capabilities("arth:option").is_empty());
        assert!(ts_import_to_host_capabilities("arth:result").is_empty());
        assert!(ts_import_to_host_capabilities("arth:string").is_empty());
        assert!(ts_import_to_host_capabilities("arth:json").is_empty());
    }

    #[test]
    fn test_ts_import_to_host_capabilities_unknown() {
        // Unknown modules default to no capabilities
        assert!(ts_import_to_host_capabilities("arth:unknown").is_empty());
        assert!(ts_import_to_host_capabilities("other:module").is_empty());
        assert!(ts_import_to_host_capabilities("./local").is_empty());
    }

    #[test]
    fn test_schema_version() {
        // Verify the schema version is updated for host_capabilities
        assert_eq!(TS_GUEST_SCHEMA_VERSION, "ts-guest-v1");
    }
}
