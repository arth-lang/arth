// Effect System for Arth
//
// This module implements the implicit effect system as specified in docs/scope.md:
// "Effects clarified: `mut`, `io`, `async`, `unsafe`, and `nothrow` modeled in
// the type/effect system and enforced across `try/await`."
//
// Key design principles:
// - Effects are inferred, not annotated (implicit effect system)
// - Provider-mediated mutability: shared state mutation must go through approved patterns
// - Unsafe shared mutation detection: direct mutation without proper guards is an error
// - Effect checking across boundaries: validates effects at call sites and try/catch
//
// Mutation patterns allowed (from scope.md §6):
// 1. `final` fields: immutable, always safe
// 2. `Atomic<T>` / thread-safe primitives: self-guarding
// 3. Actors with mailbox-serialized handlers: mutation via message passing
// 4. Capability-guarded methods: require Cap<Write<T>> or Cap<Emit<E>>
// 5. Owned<T> handles: exclusive ownership, no sharing

use std::collections::HashSet;

/// Effect kinds that can be inferred from function bodies
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Function mutates state (writes to mutable locals, fields, or shared state)
    Mut,
    /// Function performs I/O operations (file, network, console)
    IO,
    /// Function is async (contains await expressions)
    Async,
    /// Function calls unsafe code or extern functions
    Unsafe,
    /// Function may throw exceptions (has throws clause or calls throwing functions)
    Throws,
}

impl Effect {
    /// Get a human-readable name for the effect
    pub fn name(&self) -> &'static str {
        match self {
            Effect::Mut => "mut",
            Effect::IO => "io",
            Effect::Async => "async",
            Effect::Unsafe => "unsafe",
            Effect::Throws => "throws",
        }
    }
}

/// Set of effects for a function or expression
#[derive(Clone, Debug, Default)]
pub struct EffectSet {
    effects: HashSet<Effect>,
}

impl EffectSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, effect: Effect) {
        self.effects.insert(effect);
    }

    pub fn has(&self, effect: Effect) -> bool {
        self.effects.contains(&effect)
    }

    pub fn merge(&mut self, other: &EffectSet) {
        for e in &other.effects {
            self.effects.insert(*e);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Effect> {
        self.effects.iter()
    }

    /// Get effects as a sorted list of names (for stable output)
    pub fn to_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.effects.iter().map(|e| e.name()).collect();
        names.sort();
        names
    }
}

/// Mutation safety classification
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationSafety {
    /// Safe mutation via immutable (final) access
    Immutable,
    /// Safe mutation via self-guarding type (Atomic, Shared with internal sync)
    SelfGuarded,
    /// Safe mutation via exclusive ownership (Owned<T>)
    Exclusive,
    /// Safe mutation via actor/mailbox pattern
    ActorSerialized,
    /// Safe mutation via capability-guarded API
    CapabilityGuarded,
    /// Unsafe: direct mutation without proper guards
    Unsafe { reason: String },
}

impl MutationSafety {
    pub fn is_safe(&self) -> bool {
        !matches!(self, MutationSafety::Unsafe { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            MutationSafety::Immutable => "immutable access (final)".to_string(),
            MutationSafety::SelfGuarded => {
                "self-guarded type (Atomic/Shared with internal sync)".to_string()
            }
            MutationSafety::Exclusive => "exclusive ownership (Owned<T>)".to_string(),
            MutationSafety::ActorSerialized => "actor-serialized mutation".to_string(),
            MutationSafety::CapabilityGuarded => "capability-guarded mutation".to_string(),
            MutationSafety::Unsafe { reason } => format!("unsafe mutation: {}", reason),
        }
    }
}

/// Classification of a value's sharing status
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharingStatus {
    /// Value is only accessible from the current task/thread
    ThreadLocal,
    /// Value may be shared across tasks but is immutable (final)
    SharedImmutable,
    /// Value is shared and mutation is guarded by the type system (Atomic, Shared, Actor)
    SharedGuarded,
    /// Value is shared without proper guards - potential data race
    SharedUnsafe,
}

impl SharingStatus {
    pub fn allows_mutation(&self) -> bool {
        match self {
            SharingStatus::ThreadLocal => true,
            SharingStatus::SharedImmutable => false,
            SharingStatus::SharedGuarded => true, // guarded mutation allowed
            SharingStatus::SharedUnsafe => false, // mutation would be a data race
        }
    }
}

/// Information about a field's mutation safety
#[derive(Clone, Debug)]
pub struct FieldMutationInfo {
    /// Name of the field
    pub field_name: String,
    /// Whether the field is marked `final`
    pub is_final: bool,
    /// Whether the field is marked `shared`
    pub is_shared: bool,
    /// Type of the field (for determining if it's self-guarding)
    pub field_type: String,
    /// Whether the field type is self-guarding (Atomic, Shared, etc.)
    pub is_self_guarding: bool,
}

impl FieldMutationInfo {
    /// Determine if mutation to this field is safe
    pub fn check_mutation_safety(&self) -> MutationSafety {
        if self.is_final {
            return MutationSafety::Unsafe {
                reason: format!("field '{}' is final and cannot be mutated", self.field_name),
            };
        }

        if self.is_shared {
            if self.is_self_guarding {
                return MutationSafety::SelfGuarded;
            } else {
                return MutationSafety::Unsafe {
                    reason: format!(
                        "shared field '{}' must use a thread-safe wrapper type (Atomic<T>, Shared<T>, Actor, etc.) for mutation",
                        self.field_name
                    ),
                };
            }
        }

        // Non-shared, non-final: thread-local mutation is allowed
        MutationSafety::Exclusive
    }
}

/// Tracks effect state during type checking of a function body
#[derive(Clone, Debug)]
pub struct EffectEnv {
    /// Effects inferred so far in the current function
    pub inferred_effects: EffectSet,
    /// Whether we're currently inside an unsafe block
    pub in_unsafe_block: bool,
    /// Whether we're inside an async function
    pub in_async_fn: bool,
    /// Whether we're inside a try block (for exception effect tracking)
    pub try_depth: u32,
    /// Variables known to be thread-local (not shared)
    pub thread_local_vars: HashSet<String>,
    /// Variables known to be shared (provider fields, shared locals)
    pub shared_vars: HashSet<String>,
    /// Variables with known self-guarding types
    pub self_guarding_vars: HashSet<String>,
    /// Provider types known in this context
    pub provider_types: HashSet<String>,
}

impl EffectEnv {
    pub fn new() -> Self {
        Self {
            inferred_effects: EffectSet::new(),
            in_unsafe_block: false,
            in_async_fn: false,
            try_depth: 0,
            thread_local_vars: HashSet::new(),
            shared_vars: HashSet::new(),
            self_guarding_vars: HashSet::new(),
            provider_types: HashSet::new(),
        }
    }

    /// Record that a mutation effect was observed
    pub fn record_mutation(&mut self) {
        self.inferred_effects.add(Effect::Mut);
    }

    /// Record that an I/O effect was observed
    pub fn record_io(&mut self) {
        self.inferred_effects.add(Effect::IO);
    }

    /// Record that an async effect was observed (await)
    pub fn record_async(&mut self) {
        self.inferred_effects.add(Effect::Async);
    }

    /// Record that an unsafe operation was observed
    pub fn record_unsafe(&mut self) {
        self.inferred_effects.add(Effect::Unsafe);
    }

    /// Record that a throws effect was observed
    pub fn record_throws(&mut self) {
        self.inferred_effects.add(Effect::Throws);
    }

    /// Mark a variable as thread-local
    pub fn mark_thread_local(&mut self, name: &str) {
        self.thread_local_vars.insert(name.to_string());
        self.shared_vars.remove(name);
    }

    /// Mark a variable as shared
    pub fn mark_shared(&mut self, name: &str) {
        self.shared_vars.insert(name.to_string());
        self.thread_local_vars.remove(name);
    }

    /// Mark a variable as having a self-guarding type
    pub fn mark_self_guarding(&mut self, name: &str) {
        self.self_guarding_vars.insert(name.to_string());
    }

    /// Register a provider type
    pub fn register_provider(&mut self, provider_name: &str) {
        self.provider_types.insert(provider_name.to_string());
    }

    /// Check if a type is a known provider
    pub fn is_provider_type(&self, type_name: &str) -> bool {
        self.provider_types.contains(type_name)
    }

    /// Determine the sharing status of a variable
    pub fn get_sharing_status(&self, name: &str) -> SharingStatus {
        if self.shared_vars.contains(name) {
            if self.self_guarding_vars.contains(name) {
                SharingStatus::SharedGuarded
            } else {
                SharingStatus::SharedUnsafe
            }
        } else if self.thread_local_vars.contains(name) {
            SharingStatus::ThreadLocal
        } else {
            // Default: assume thread-local
            SharingStatus::ThreadLocal
        }
    }

    /// Check if a mutation to a variable is safe
    pub fn check_mutation(&self, name: &str) -> MutationSafety {
        let status = self.get_sharing_status(name);
        match status {
            SharingStatus::ThreadLocal => MutationSafety::Exclusive,
            SharingStatus::SharedImmutable => MutationSafety::Unsafe {
                reason: format!("'{}' is shared and immutable", name),
            },
            SharingStatus::SharedGuarded => MutationSafety::SelfGuarded,
            SharingStatus::SharedUnsafe => MutationSafety::Unsafe {
                reason: format!(
                    "'{}' is shared without proper synchronization; use Atomic<T>, Shared<T>, or capability-guarded access",
                    name
                ),
            },
        }
    }

    /// Enter a try block
    pub fn enter_try(&mut self) {
        self.try_depth += 1;
    }

    /// Exit a try block
    pub fn exit_try(&mut self) {
        self.try_depth = self.try_depth.saturating_sub(1);
    }

    /// Check if we're inside a try block
    pub fn in_try_block(&self) -> bool {
        self.try_depth > 0
    }
}

impl Default for EffectEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Known I/O function patterns for effect inference
const IO_FUNCTIONS: &[&str] = &[
    "print",
    "println",
    "read",
    "readLine",
    "write",
    "open",
    "close",
    "flush",
    "seek",
    "File.read",
    "File.write",
    "File.open",
    "File.close",
    "Console.read",
    "Console.write",
    "Net.connect",
    "Net.listen",
    "Net.send",
    "Net.receive",
    "Http.get",
    "Http.post",
    "Http.fetch",
];

/// Known self-guarding types (types that provide internal synchronization)
const SELF_GUARDING_TYPES: &[&str] = &[
    "Atomic",
    "AtomicInt",
    "AtomicBool",
    "AtomicRef",
    "Shared",
    "Watch",
    "Notify",
    "Actor",
    "Mutex",
    "RwLock",
    "Channel",
    "Mailbox",
];

/// Known mutator function names
const MUTATOR_NAMES: &[&str] = &[
    "put",
    "remove",
    "insert",
    "set",
    "push",
    "pop",
    "clear",
    "update",
    "swap",
    "append",
    "prepend",
    "add",
    "delete",
    "offer",
    "poll",
    "write",
    "inc",
    "dec",
    "publish",
    "emit",
    "post",
    "send",
    "store",
    "compareAndSet",
    "getAndSet",
    "getAndAdd",
];

/// Check if a function name is a known I/O function
pub fn is_io_function(name: &str) -> bool {
    IO_FUNCTIONS.contains(&name)
}

/// Check if a type name is a known self-guarding type
pub fn is_self_guarding_type(type_name: &str) -> bool {
    SELF_GUARDING_TYPES.contains(&type_name)
}

/// Check if a function name is a known mutator
pub fn is_mutator_function(name: &str) -> bool {
    MUTATOR_NAMES.contains(&name)
}

/// Validate that a mutation operation is safe given the receiver type
pub fn validate_mutation(
    receiver_type: &str,
    operation: &str,
    is_shared_context: bool,
    has_capability: bool,
) -> MutationSafety {
    // Self-guarding types always allow mutation
    if is_self_guarding_type(receiver_type) {
        return MutationSafety::SelfGuarded;
    }

    // Capability-guarded mutation
    if has_capability {
        return MutationSafety::CapabilityGuarded;
    }

    // In shared context without guards, mutation is unsafe
    if is_shared_context {
        return MutationSafety::Unsafe {
            reason: format!(
                "mutation '{}' on shared '{}' requires Atomic<T>, Shared<T>, or capability token",
                operation, receiver_type
            ),
        };
    }

    // Thread-local mutation is allowed
    MutationSafety::Exclusive
}

/// Errors related to effect violations
#[derive(Clone, Debug)]
pub enum EffectError {
    /// Mutation of shared state without proper guards
    UnsafeSharedMutation {
        variable: String,
        operation: String,
        reason: String,
    },
    /// I/O operation in a context that doesn't allow it
    UnexpectedIO { operation: String, context: String },
    /// Async operation (await) in non-async context
    AwaitInNonAsync { context: String },
    /// Unsafe operation outside unsafe block
    UnsafeOutsideUnsafeBlock { operation: String },
    /// Throwing operation without proper handling
    UnhandledThrows {
        operation: String,
        exception_types: Vec<String>,
    },
    /// Mutation across try boundary may violate exception safety
    MutationAcrossTry { variable: String, reason: String },
}

impl EffectError {
    pub fn to_message(&self) -> String {
        match self {
            EffectError::UnsafeSharedMutation {
                variable,
                operation,
                reason,
            } => {
                format!(
                    "unsafe shared mutation: '{}' on '{}' - {}",
                    operation, variable, reason
                )
            }
            EffectError::UnexpectedIO { operation, context } => {
                format!("I/O operation '{}' not allowed in {}", operation, context)
            }
            EffectError::AwaitInNonAsync { context } => {
                format!(
                    "'await' is only allowed in async functions, found in {}",
                    context
                )
            }
            EffectError::UnsafeOutsideUnsafeBlock { operation } => {
                format!(
                    "unsafe operation '{}' requires an unsafe block or unsafe function",
                    operation
                )
            }
            EffectError::UnhandledThrows {
                operation,
                exception_types,
            } => {
                format!(
                    "'{}' may throw {} which must be caught or declared in throws clause",
                    operation,
                    exception_types.join(", ")
                )
            }
            EffectError::MutationAcrossTry { variable, reason } => {
                format!(
                    "mutation of '{}' across try boundary may violate exception safety: {}",
                    variable, reason
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effect_set_basic() {
        let mut effects = EffectSet::new();
        assert!(effects.is_empty());

        effects.add(Effect::Mut);
        assert!(effects.has(Effect::Mut));
        assert!(!effects.has(Effect::IO));

        effects.add(Effect::IO);
        assert!(effects.has(Effect::IO));
    }

    #[test]
    fn test_effect_set_merge() {
        let mut effects1 = EffectSet::new();
        effects1.add(Effect::Mut);

        let mut effects2 = EffectSet::new();
        effects2.add(Effect::IO);
        effects2.add(Effect::Async);

        effects1.merge(&effects2);
        assert!(effects1.has(Effect::Mut));
        assert!(effects1.has(Effect::IO));
        assert!(effects1.has(Effect::Async));
    }

    #[test]
    fn test_effect_env_mutation_thread_local() {
        let mut env = EffectEnv::new();
        env.mark_thread_local("x");

        let safety = env.check_mutation("x");
        assert!(safety.is_safe());
        assert_eq!(safety, MutationSafety::Exclusive);
    }

    #[test]
    fn test_effect_env_mutation_shared_unsafe() {
        let mut env = EffectEnv::new();
        env.mark_shared("x");
        // Not marked as self-guarding

        let safety = env.check_mutation("x");
        assert!(!safety.is_safe());
        assert!(matches!(safety, MutationSafety::Unsafe { .. }));
    }

    #[test]
    fn test_effect_env_mutation_shared_guarded() {
        let mut env = EffectEnv::new();
        env.mark_shared("x");
        env.mark_self_guarding("x");

        let safety = env.check_mutation("x");
        assert!(safety.is_safe());
        assert_eq!(safety, MutationSafety::SelfGuarded);
    }

    #[test]
    fn test_is_self_guarding_type() {
        assert!(is_self_guarding_type("Atomic"));
        assert!(is_self_guarding_type("Shared"));
        assert!(is_self_guarding_type("Actor"));
        assert!(!is_self_guarding_type("String"));
        assert!(!is_self_guarding_type("MyStruct"));
    }

    #[test]
    fn test_is_mutator_function() {
        assert!(is_mutator_function("put"));
        assert!(is_mutator_function("push"));
        assert!(is_mutator_function("set"));
        assert!(!is_mutator_function("get"));
        assert!(!is_mutator_function("read"));
    }

    #[test]
    fn test_validate_mutation_self_guarding() {
        let safety = validate_mutation("Atomic", "set", true, false);
        assert_eq!(safety, MutationSafety::SelfGuarded);
    }

    #[test]
    fn test_validate_mutation_capability() {
        let safety = validate_mutation("MyType", "update", true, true);
        assert_eq!(safety, MutationSafety::CapabilityGuarded);
    }

    #[test]
    fn test_validate_mutation_shared_unsafe() {
        let safety = validate_mutation("MyType", "set", true, false);
        assert!(!safety.is_safe());
    }

    #[test]
    fn test_validate_mutation_thread_local() {
        let safety = validate_mutation("MyType", "set", false, false);
        assert!(safety.is_safe());
        assert_eq!(safety, MutationSafety::Exclusive);
    }

    #[test]
    fn test_field_mutation_info_final() {
        let info = FieldMutationInfo {
            field_name: "count".to_string(),
            is_final: true,
            is_shared: false,
            field_type: "Int".to_string(),
            is_self_guarding: false,
        };
        let safety = info.check_mutation_safety();
        assert!(!safety.is_safe());
    }

    #[test]
    fn test_field_mutation_info_shared_guarded() {
        let info = FieldMutationInfo {
            field_name: "counter".to_string(),
            is_final: false,
            is_shared: true,
            field_type: "Atomic<Int>".to_string(),
            is_self_guarding: true,
        };
        let safety = info.check_mutation_safety();
        assert!(safety.is_safe());
        assert_eq!(safety, MutationSafety::SelfGuarded);
    }

    #[test]
    fn test_field_mutation_info_shared_unguarded() {
        let info = FieldMutationInfo {
            field_name: "data".to_string(),
            is_final: false,
            is_shared: true,
            field_type: "String".to_string(),
            is_self_guarding: false,
        };
        let safety = info.check_mutation_safety();
        assert!(!safety.is_safe());
    }

    #[test]
    fn test_effect_error_messages() {
        let err = EffectError::UnsafeSharedMutation {
            variable: "cache".to_string(),
            operation: "put".to_string(),
            reason: "shared without guards".to_string(),
        };
        assert!(err.to_message().contains("cache"));
        assert!(err.to_message().contains("put"));
    }

    #[test]
    fn test_effect_env_try_depth() {
        let mut env = EffectEnv::new();
        assert!(!env.in_try_block());

        env.enter_try();
        assert!(env.in_try_block());
        assert_eq!(env.try_depth, 1);

        env.enter_try();
        assert_eq!(env.try_depth, 2);

        env.exit_try();
        assert_eq!(env.try_depth, 1);
        assert!(env.in_try_block());

        env.exit_try();
        assert!(!env.in_try_block());
    }
}
