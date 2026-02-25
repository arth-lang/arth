use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Parsed `arth.toml` manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
    #[serde(default)]
    pub profile: ProfileConfig,
}

/// Build profile configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileConfig {
    /// Development profile settings
    #[serde(default)]
    pub dev: BuildProfile,
    /// Release profile settings
    #[serde(default)]
    pub release: BuildProfile,
}

/// Settings for a specific build profile.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BuildProfile {
    /// Optimization level (0-3)
    #[serde(rename = "opt-level")]
    pub opt_level: Option<u8>,
    /// Whether to include debug info
    pub debug: Option<bool>,
    /// Whether to enable incremental compilation
    pub incremental: Option<bool>,
}

/// `[package]` section of `arth.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
    #[serde(default)]
    pub entry: Option<String>,
}

/// A dependency entry under `[dependencies]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    /// Simple form: `"group:artifact" = "version-range"`.
    Simple(String),
    /// Table form: `"group:artifact" = { version = "...", features = [...], optional = true }`.
    Detailed(DetailedDependency),
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailedDependency {
    pub version: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    ParseToml(toml::de::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "IO error: {}", e),
            ManifestError::ParseToml(e) => write!(f, "TOML parse error: {}", e),
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<std::io::Error> for ManifestError {
    fn from(e: std::io::Error) -> Self {
        ManifestError::Io(e)
    }
}

impl From<toml::de::Error> for ManifestError {
    fn from(e: toml::de::Error) -> Self {
        ManifestError::ParseToml(e)
    }
}

pub fn parse_manifest(path: &Path) -> Result<Manifest, ManifestError> {
    let text = fs::read_to_string(path)?;
    parse_manifest_str(&text)
}

pub fn parse_manifest_str(text: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest = toml::from_str(text)?;
    Ok(manifest)
}

/// Parsed `arth.lock.json` lockfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    pub package: LockPackage,
    pub dependencies: BTreeMap<String, LockDependency>,
    /// Source file fingerprints for reproducibility (package -> hash)
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_fingerprints: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "LockMetadata::is_default")]
    pub metadata: LockMetadata,
}

/// Package information in lockfile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
}

/// Locked dependency entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockDependency {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// Lockfile metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockMetadata {
    #[serde(
        rename = "lockfileVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lockfile_version: Option<i64>,
    #[serde(
        rename = "resolvedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resolved_at: Option<String>,
    #[serde(
        rename = "arthVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub arth_version: Option<String>,
}

impl LockMetadata {
    fn is_default(&self) -> bool {
        self.lockfile_version.is_none() && self.resolved_at.is_none() && self.arth_version.is_none()
    }
}

/// Error type for lockfile operations
#[derive(Debug)]
pub enum LockfileError {
    /// IO error reading/writing lockfile
    Io(std::io::Error),
    /// JSON parse error
    ParseJson(serde_json::Error),
    /// JSON serialization error
    SerializeJson(serde_json::Error),
}

impl std::fmt::Display for LockfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockfileError::Io(e) => write!(f, "IO error: {}", e),
            LockfileError::ParseJson(e) => write!(f, "JSON parse error: {}", e),
            LockfileError::SerializeJson(e) => write!(f, "JSON serialize error: {}", e),
        }
    }
}

impl std::error::Error for LockfileError {}

impl From<std::io::Error> for LockfileError {
    fn from(e: std::io::Error) -> Self {
        LockfileError::Io(e)
    }
}

impl From<serde_json::Error> for LockfileError {
    fn from(e: serde_json::Error) -> Self {
        LockfileError::ParseJson(e)
    }
}

/// Parse a lockfile from a path
pub fn parse_lockfile(path: &Path) -> Result<Lockfile, LockfileError> {
    let text = fs::read_to_string(path)?;
    parse_lockfile_str(&text)
}

/// Parse a lockfile from a string
pub fn parse_lockfile_str(text: &str) -> Result<Lockfile, LockfileError> {
    let lock: Lockfile = serde_json::from_str(text)?;
    Ok(lock)
}

/// Serialize a lockfile to a pretty-printed JSON string
pub fn serialize_lockfile(lockfile: &Lockfile) -> Result<String, LockfileError> {
    serde_json::to_string_pretty(lockfile).map_err(LockfileError::SerializeJson)
}

/// Write a lockfile to a path
pub fn write_lockfile(path: &Path, lockfile: &Lockfile) -> Result<(), LockfileError> {
    let json = serialize_lockfile(lockfile)?;
    fs::write(path, json)?;
    Ok(())
}

/// Simple `.env` / `.properties` style key/value map.
pub(crate) type EnvMap = BTreeMap<String, String>;

#[derive(Debug)]
pub(crate) enum EnvError {
    Io(std::io::Error),
}

impl From<std::io::Error> for EnvError {
    fn from(e: std::io::Error) -> Self {
        EnvError::Io(e)
    }
}

pub(crate) fn parse_env_file(path: &Path) -> Result<EnvMap, EnvError> {
    let text = fs::read_to_string(path)?;
    Ok(parse_env_str(&text))
}

pub(crate) fn parse_env_str(text: &str) -> EnvMap {
    let mut map = EnvMap::new();
    for line in text.lines() {
        let mut l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix("export ") {
            l = rest.trim();
        }
        let Some(eq) = l.find('=') else {
            continue;
        };
        let key = l[..eq].trim();
        let mut value = l[eq + 1..].trim();
        if key.is_empty() {
            continue;
        }
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            if value.len() >= 2 {
                value = &value[1..value.len() - 1];
            }
        }
        map.insert(key.to_string(), value.to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_minimal_example() {
        let txt = r#"
[package]
name = "hello-world"
version = "0.1.0"
edition = "2025"
entry = "src/main.arth"

[dependencies]
"org.example:net-http" = "^1.2.0"
"org.example:serde"    = { version = "~0.9", features = ["derive"] }
"org.example:opt"      = { version = "1.0.0", optional = true }
"org.example:simple"   = "2.0.0"
"org.example:spaced-key" = "3.0.0"
        "#;

        let manifest = parse_manifest_str(txt).expect("manifest should parse");
        assert_eq!(manifest.package.name, "hello-world");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.package.edition, "2025");
        assert_eq!(manifest.package.entry.as_deref(), Some("src/main.arth"));
        assert!(manifest.dependencies.contains_key("org.example:net-http"));
        assert!(manifest.dependencies.contains_key("org.example:serde"));
        assert!(manifest.dependencies.contains_key("org.example:opt"));
        assert!(manifest.dependencies.contains_key("org.example:simple"));
    }

    #[test]
    fn parse_lockfile_example() {
        let txt = r#"
{
  "package": { "name": "hello-world", "version": "0.1.0" },
  "dependencies": {
    "org.example:net-http": {
      "version": "1.2.3",
      "checksum": "sha256-AAA",
      "source": "registry+https://registry.arth.dev"
    },
    "org.example:serde": {
      "version": "0.9.7",
      "features": ["derive"],
      "checksum": "sha256-BBB"
    }
  },
  "metadata": {
    "lockfileVersion": 1,
    "resolvedAt": "2025-01-01T00:00:00Z"
  }
}
        "#;

        let lock = parse_lockfile_str(txt).expect("lockfile should parse");
        assert_eq!(lock.package.name, "hello-world");
        assert_eq!(lock.package.version, "0.1.0");
        assert_eq!(
            lock.metadata.lockfile_version,
            Some(1),
            "lockfileVersion should round-trip"
        );
        assert!(lock.dependencies.get("org.example:net-http").is_some());
        let serde_dep = lock
            .dependencies
            .get("org.example:serde")
            .expect("serde dep");
        assert_eq!(serde_dep.features, vec!["derive"]);
    }

    #[test]
    fn parse_env_basic_and_quoted() {
        let txt = r#"
        # comment
        KEY=value
        export FOO = "bar baz"
        SPACED =  "  spaced  "
        INVALID
        "#;

        let env = parse_env_str(txt);
        assert_eq!(env.get("KEY").map(String::as_str), Some("value"));
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar baz"));
        assert_eq!(env.get("SPACED").map(String::as_str), Some("  spaced  "));
        assert!(!env.contains_key("INVALID"));
    }

    #[test]
    fn lockfile_serialize_roundtrip() {
        use std::collections::BTreeMap;

        // Create a lockfile
        let mut deps = BTreeMap::new();
        deps.insert(
            "org.example:lib".to_string(),
            LockDependency {
                version: "1.0.0".to_string(),
                checksum: Some("sha256-abc123".to_string()),
                source: Some("registry+https://registry.arth.dev".to_string()),
                features: vec!["feature1".to_string()],
                dependencies: vec!["org.example:transitive".to_string()],
            },
        );

        let lockfile = Lockfile {
            package: LockPackage {
                name: "my-project".to_string(),
                version: "2.0.0".to_string(),
            },
            dependencies: deps,
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata {
                lockfile_version: Some(1),
                resolved_at: Some("2025-12-29T12:00:00Z".to_string()),
                arth_version: Some("0.1.0".to_string()),
            },
        };

        // Serialize to JSON
        let json = serialize_lockfile(&lockfile).expect("serialize should work");

        // Parse back
        let parsed = parse_lockfile_str(&json).expect("parse should work");

        // Verify round-trip
        assert_eq!(parsed.package.name, "my-project");
        assert_eq!(parsed.package.version, "2.0.0");
        assert_eq!(parsed.metadata.lockfile_version, Some(1));
        assert_eq!(
            parsed.metadata.resolved_at,
            Some("2025-12-29T12:00:00Z".to_string())
        );

        let dep = parsed
            .dependencies
            .get("org.example:lib")
            .expect("dep exists");
        assert_eq!(dep.version, "1.0.0");
        assert_eq!(dep.checksum, Some("sha256-abc123".to_string()));
        assert_eq!(dep.features, vec!["feature1"]);
        assert_eq!(dep.dependencies, vec!["org.example:transitive"]);
    }

    #[test]
    fn lockfile_minimal_serialize() {
        // Lockfile with no optional fields
        let lockfile = Lockfile {
            package: LockPackage {
                name: "minimal".to_string(),
                version: "1.0.0".to_string(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };

        let json = serialize_lockfile(&lockfile).expect("serialize should work");

        // Verify minimal output doesn't include empty fields
        assert!(!json.contains("metadata")); // Empty metadata is skipped
        assert!(json.contains("\"name\": \"minimal\""));

        // Parse back
        let parsed = parse_lockfile_str(&json).expect("parse should work");
        assert_eq!(parsed.package.name, "minimal");
        assert!(parsed.dependencies.is_empty());
    }
}
