// Capability Token Tracking for Arth
//
// This module implements tracking for capability tokens as specified in docs/spec.md §11:
// "Providers mint zero-sized, linear capability tokens (e.g., `Cap<WriteCache>`).
// Functions requiring mutation must accept the relevant capability."
//
// Key design principles:
// - Capability tokens should originate from providers (provider fields or provider methods)
// - Capabilities gate mutation operations on Watch<T> and Notify<E>
// - The compiler tracks capability origin to enforce provider-mediated mutation
//
// This provides compile-time enforcement of the capability-based access control pattern
// without runtime overhead (capabilities are zero-sized at runtime).

use std::collections::HashMap;

/// Origin of a capability token - where the Cap<...> value came from
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityOrigin {
    /// Capability obtained from a provider field access (e.g., `provider.writeCap`)
    ProviderField {
        /// Name of the provider type
        provider_type: String,
        /// Name of the field that yielded the capability
        field_name: String,
    },
    /// Capability obtained from a provider method call (e.g., `ProviderFns.getCap(p)`)
    ProviderMethod {
        /// Name of the provider type
        provider_type: String,
        /// Name of the method that returned the capability
        method_name: String,
    },
    /// Capability passed as a function parameter (assumed valid if caller provided it)
    Parameter {
        /// Parameter name
        param_name: String,
    },
    /// Capability from an unknown source (local creation, not provider-minted)
    /// This generates a warning as capabilities should be provider-minted
    Unknown,
}

impl CapabilityOrigin {
    /// Check if this capability has a valid provider origin
    pub fn is_provider_minted(&self) -> bool {
        matches!(
            self,
            CapabilityOrigin::ProviderField { .. }
                | CapabilityOrigin::ProviderMethod { .. }
                | CapabilityOrigin::Parameter { .. }
        )
    }

    /// Get a description for diagnostics
    pub fn describe(&self) -> String {
        match self {
            CapabilityOrigin::ProviderField {
                provider_type,
                field_name,
            } => {
                format!("provider field '{}.{}'", provider_type, field_name)
            }
            CapabilityOrigin::ProviderMethod {
                provider_type,
                method_name,
            } => {
                format!("provider method '{}.{}'", provider_type, method_name)
            }
            CapabilityOrigin::Parameter { param_name } => {
                format!("parameter '{}'", param_name)
            }
            CapabilityOrigin::Unknown => "unknown source (not provider-minted)".to_string(),
        }
    }
}

/// Information about a capability token held by a local variable
#[derive(Clone, Debug)]
pub struct CapabilityInfo {
    /// The type parameter of the capability (e.g., "WriteCache" for Cap<WriteCache>)
    pub cap_type: String,
    /// Where this capability originated
    pub origin: CapabilityOrigin,
    /// Whether this capability has been consumed (for linearity tracking)
    pub consumed: bool,
}

/// Tracks capability tokens in the current function scope
#[derive(Clone, Debug, Default)]
pub struct CapabilityEnv {
    /// Map from local variable name to capability info
    /// Only populated for locals that hold Cap<...> types
    capabilities: HashMap<String, CapabilityInfo>,

    /// Set of provider type names known in the current context
    /// Used to determine if a type is a provider when checking field access
    known_providers: std::collections::HashSet<String>,
}

impl CapabilityEnv {
    /// Create a new capability environment
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider type as known
    pub fn register_provider(&mut self, provider_name: String) {
        self.known_providers.insert(provider_name);
    }

    /// Check if a type name refers to a known provider
    pub fn is_provider_type(&self, type_name: &str) -> bool {
        self.known_providers.contains(type_name)
    }

    /// Record a capability token held by a local variable
    pub fn record_capability(
        &mut self,
        local_name: String,
        cap_type: String,
        origin: CapabilityOrigin,
    ) {
        self.capabilities.insert(
            local_name,
            CapabilityInfo {
                cap_type,
                origin,
                consumed: false,
            },
        );
    }

    /// Get capability info for a local variable (if it holds a capability)
    pub fn get_capability(&self, local_name: &str) -> Option<&CapabilityInfo> {
        self.capabilities.get(local_name)
    }

    /// Mark a capability as consumed (for linearity tracking)
    pub fn consume_capability(&mut self, local_name: &str) -> bool {
        if let Some(info) = self.capabilities.get_mut(local_name) {
            if info.consumed {
                return false; // Already consumed
            }
            info.consumed = true;
            true
        } else {
            false
        }
    }

    /// Check if a capability has been consumed
    pub fn is_consumed(&self, local_name: &str) -> bool {
        self.capabilities
            .get(local_name)
            .map(|i| i.consumed)
            .unwrap_or(false)
    }

    /// Remove a capability (e.g., when variable goes out of scope)
    pub fn remove_capability(&mut self, local_name: &str) {
        self.capabilities.remove(local_name);
    }

    /// Check if any capability in the arguments has a valid provider origin
    /// Returns (has_capability, has_provider_origin, origin_description)
    pub fn check_capability_args(
        &self,
        arg_names: &[String],
    ) -> (bool, bool, Option<CapabilityOrigin>) {
        for name in arg_names {
            if let Some(info) = self.capabilities.get(name) {
                let is_valid = info.origin.is_provider_minted();
                return (true, is_valid, Some(info.origin.clone()));
            }
        }
        (false, false, None)
    }

    /// Clear all capabilities (e.g., at function boundary)
    pub fn clear(&mut self) {
        self.capabilities.clear();
    }
}

/// Validation result for capability usage
#[derive(Debug)]
pub enum CapabilityValidation {
    /// Capability is valid and properly provider-minted
    Valid,
    /// Capability present but from unknown origin (warning)
    UnknownOrigin { origin_desc: String },
    /// No capability provided where one is required (error)
    Missing,
    /// Capability was already consumed (error for linear capabilities)
    AlreadyConsumed { local_name: String },
}

/// Extract the inner type from a Cap<T> type string
/// Returns None if not a Cap type
pub fn extract_cap_inner_type(cap_type: &str) -> Option<&str> {
    let trimmed = cap_type.trim();
    if trimmed.starts_with("Cap<") && trimmed.ends_with('>') {
        Some(&trimmed[4..trimmed.len() - 1])
    } else {
        None
    }
}

/// Check if a type path represents a Cap<...> type
pub fn is_cap_type_path(path: &[String]) -> bool {
    path.last().map(|s| s == "Cap").unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_origin_provider_field() {
        let origin = CapabilityOrigin::ProviderField {
            provider_type: "CacheProvider".to_string(),
            field_name: "writeCap".to_string(),
        };
        assert!(origin.is_provider_minted());
        assert!(origin.describe().contains("CacheProvider"));
    }

    #[test]
    fn test_capability_origin_unknown() {
        let origin = CapabilityOrigin::Unknown;
        assert!(!origin.is_provider_minted());
        assert!(origin.describe().contains("not provider-minted"));
    }

    #[test]
    fn test_capability_env_record_and_get() {
        let mut env = CapabilityEnv::new();
        env.record_capability(
            "cap".to_string(),
            "WriteCache".to_string(),
            CapabilityOrigin::ProviderField {
                provider_type: "CacheProvider".to_string(),
                field_name: "writeCap".to_string(),
            },
        );

        let info = env.get_capability("cap").unwrap();
        assert_eq!(info.cap_type, "WriteCache");
        assert!(!info.consumed);
        assert!(info.origin.is_provider_minted());
    }

    #[test]
    fn test_capability_consumption() {
        let mut env = CapabilityEnv::new();
        env.record_capability(
            "cap".to_string(),
            "WriteCache".to_string(),
            CapabilityOrigin::Unknown,
        );

        assert!(!env.is_consumed("cap"));
        assert!(env.consume_capability("cap"));
        assert!(env.is_consumed("cap"));
        // Second consumption should fail
        assert!(!env.consume_capability("cap"));
    }

    #[test]
    fn test_extract_cap_inner_type() {
        assert_eq!(
            extract_cap_inner_type("Cap<WriteCache>"),
            Some("WriteCache")
        );
        assert_eq!(
            extract_cap_inner_type("Cap<Emit<Event>>"),
            Some("Emit<Event>")
        );
        assert_eq!(extract_cap_inner_type("NotACap"), None);
        assert_eq!(extract_cap_inner_type("Cap"), None);
    }

    #[test]
    fn test_provider_registration() {
        let mut env = CapabilityEnv::new();
        assert!(!env.is_provider_type("CacheProvider"));

        env.register_provider("CacheProvider".to_string());
        assert!(env.is_provider_type("CacheProvider"));
    }
}
