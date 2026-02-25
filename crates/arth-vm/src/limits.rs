//! VM Resource Limits and Stack Overflow Protection
//!
//! This module provides:
//! - Configurable resource limits for VM execution
//! - Stack overflow detection and protection
//! - Call depth limiting for recursion control
//! - Structured error types for limit violations

use std::sync::OnceLock;

/// Default maximum value stack depth (1 million values)
pub const DEFAULT_MAX_STACK_DEPTH: usize = 1_000_000;

/// Default maximum call depth (64K nested calls)
pub const DEFAULT_MAX_CALL_DEPTH: usize = 64_000;

/// Default maximum execution steps (2 million)
pub const DEFAULT_MAX_STEPS: u64 = 2_000_000;

/// Minimum allowed stack depth (prevents configuration errors)
pub const MIN_STACK_DEPTH: usize = 100;

/// Minimum allowed call depth
pub const MIN_CALL_DEPTH: usize = 10;

/// Minimum allowed steps
pub const MIN_STEPS: u64 = 1000;

/// VM resource limits configuration
#[derive(Debug, Clone)]
pub struct VmLimits {
    /// Maximum number of values on the operand stack
    pub max_stack_depth: usize,
    /// Maximum call stack depth (nested function calls)
    pub max_call_depth: usize,
    /// Maximum execution steps before abort
    pub max_steps: u64,
}

impl VmLimits {
    /// Create limits with default values
    pub fn default_limits() -> Self {
        Self {
            max_stack_depth: DEFAULT_MAX_STACK_DEPTH,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    /// Create limits from environment variables with defaults
    pub fn from_env() -> Self {
        Self {
            max_stack_depth: Self::get_env_usize(
                "ARTH_VM_MAX_STACK_DEPTH",
                DEFAULT_MAX_STACK_DEPTH,
            ),
            max_call_depth: Self::get_env_usize("ARTH_VM_MAX_CALL_DEPTH", DEFAULT_MAX_CALL_DEPTH),
            max_steps: Self::get_env_u64("ARTH_VM_MAX_STEPS", DEFAULT_MAX_STEPS),
        }
    }

    /// Create limits for testing with smaller values
    pub fn for_testing() -> Self {
        Self {
            max_stack_depth: 10_000,
            max_call_depth: 1_000,
            max_steps: 100_000,
        }
    }

    /// Create minimal limits for testing overflow behavior
    pub fn minimal() -> Self {
        Self {
            max_stack_depth: MIN_STACK_DEPTH,
            max_call_depth: MIN_CALL_DEPTH,
            max_steps: MIN_STEPS,
        }
    }

    /// Create limits with a custom stack depth
    pub fn with_stack_depth(mut self, depth: usize) -> Self {
        self.max_stack_depth = depth.max(MIN_STACK_DEPTH);
        self
    }

    /// Create limits with a custom call depth
    pub fn with_call_depth(mut self, depth: usize) -> Self {
        self.max_call_depth = depth.max(MIN_CALL_DEPTH);
        self
    }

    /// Create limits with a custom step limit
    pub fn with_max_steps(mut self, steps: u64) -> Self {
        self.max_steps = steps.max(MIN_STEPS);
        self
    }

    /// Validate that the current stack depth is within limits
    #[inline]
    pub fn check_stack(&self, current_depth: usize) -> Result<(), VmLimitError> {
        if current_depth > self.max_stack_depth {
            Err(VmLimitError::StackOverflow {
                current: current_depth,
                limit: self.max_stack_depth,
            })
        } else {
            Ok(())
        }
    }

    /// Validate that the current call depth is within limits
    #[inline]
    pub fn check_call_depth(&self, current_depth: usize) -> Result<(), VmLimitError> {
        if current_depth > self.max_call_depth {
            Err(VmLimitError::CallDepthExceeded {
                current: current_depth,
                limit: self.max_call_depth,
            })
        } else {
            Ok(())
        }
    }

    /// Validate that the step count is within limits
    #[inline]
    pub fn check_steps(&self, current_steps: u64) -> Result<(), VmLimitError> {
        if current_steps > self.max_steps {
            Err(VmLimitError::StepLimitExceeded {
                current: current_steps,
                limit: self.max_steps,
            })
        } else {
            Ok(())
        }
    }

    fn get_env_usize(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map(|v| v.max(MIN_STACK_DEPTH))
            .unwrap_or(default)
    }

    fn get_env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|v| v.max(MIN_STEPS))
            .unwrap_or(default)
    }
}

impl Default for VmLimits {
    fn default() -> Self {
        Self::default_limits()
    }
}

/// Error types for VM limit violations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmLimitError {
    /// Value stack exceeded maximum depth
    StackOverflow {
        /// Current stack depth when overflow occurred
        current: usize,
        /// Configured limit
        limit: usize,
    },
    /// Call stack exceeded maximum depth (too many nested function calls)
    CallDepthExceeded {
        /// Current call depth when limit exceeded
        current: usize,
        /// Configured limit
        limit: usize,
    },
    /// Execution step limit exceeded (possible infinite loop)
    StepLimitExceeded {
        /// Current step count when limit exceeded
        current: u64,
        /// Configured limit
        limit: u64,
    },
}

impl std::fmt::Display for VmLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmLimitError::StackOverflow { current, limit } => {
                write!(
                    f,
                    "stack overflow: {} values exceeds limit of {}. \
                     Set ARTH_VM_MAX_STACK_DEPTH to increase.",
                    current, limit
                )
            }
            VmLimitError::CallDepthExceeded { current, limit } => {
                write!(
                    f,
                    "call depth exceeded: {} calls exceeds limit of {}. \
                     Set ARTH_VM_MAX_CALL_DEPTH to increase.",
                    current, limit
                )
            }
            VmLimitError::StepLimitExceeded { current, limit } => {
                write!(
                    f,
                    "step limit exceeded: {} steps exceeds limit of {} (possible infinite loop). \
                     Set ARTH_VM_MAX_STEPS to increase.",
                    current, limit
                )
            }
        }
    }
}

impl std::error::Error for VmLimitError {}

/// Global VM limits, lazily initialized from environment
pub fn global_limits() -> &'static VmLimits {
    static LIMITS: OnceLock<VmLimits> = OnceLock::new();
    LIMITS.get_or_init(VmLimits::from_env)
}

/// Check if a stack depth would cause overflow with global limits
#[inline]
pub fn would_overflow_stack(depth: usize) -> bool {
    depth > global_limits().max_stack_depth
}

/// Check if a call depth would exceed limits with global limits
#[inline]
pub fn would_exceed_call_depth(depth: usize) -> bool {
    depth > global_limits().max_call_depth
}

/// Check if a step count would exceed limits with global limits
#[inline]
pub fn would_exceed_steps(steps: u64) -> bool {
    steps > global_limits().max_steps
}

/// Stack guard that provides efficient overflow checking
///
/// This struct tracks stack usage and provides fast path checking
/// by using a warning threshold before the actual limit.
#[derive(Debug)]
pub struct StackGuard {
    /// Current stack depth
    current: usize,
    /// Warning threshold (e.g., 90% of limit)
    warning_threshold: usize,
    /// Hard limit
    limit: usize,
    /// Whether warning has been triggered
    warning_triggered: bool,
}

impl StackGuard {
    /// Create a new stack guard with the given limit
    pub fn new(limit: usize) -> Self {
        let warning_threshold = (limit as f64 * 0.9) as usize;
        Self {
            current: 0,
            warning_threshold,
            limit,
            warning_triggered: false,
        }
    }

    /// Create from VmLimits
    pub fn from_limits(limits: &VmLimits) -> Self {
        Self::new(limits.max_stack_depth)
    }

    /// Push operation - returns error if would overflow
    #[inline]
    pub fn push(&mut self) -> Result<(), VmLimitError> {
        self.current += 1;
        if self.current > self.limit {
            Err(VmLimitError::StackOverflow {
                current: self.current,
                limit: self.limit,
            })
        } else {
            // Log warning once when approaching limit
            if !self.warning_triggered && self.current >= self.warning_threshold {
                self.warning_triggered = true;
                eprintln!(
                    "warning: stack depth {} approaching limit of {}",
                    self.current, self.limit
                );
            }
            Ok(())
        }
    }

    /// Push multiple values - returns error if would overflow
    #[inline]
    pub fn push_n(&mut self, n: usize) -> Result<(), VmLimitError> {
        let new_depth = self.current.saturating_add(n);
        if new_depth > self.limit {
            Err(VmLimitError::StackOverflow {
                current: new_depth,
                limit: self.limit,
            })
        } else {
            self.current = new_depth;
            if !self.warning_triggered && self.current >= self.warning_threshold {
                self.warning_triggered = true;
                eprintln!(
                    "warning: stack depth {} approaching limit of {}",
                    self.current, self.limit
                );
            }
            Ok(())
        }
    }

    /// Pop operation
    #[inline]
    pub fn pop(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Pop multiple values
    #[inline]
    pub fn pop_n(&mut self, n: usize) {
        self.current = self.current.saturating_sub(n);
    }

    /// Get current depth
    pub fn depth(&self) -> usize {
        self.current
    }

    /// Set current depth directly (for frame restoration)
    pub fn set_depth(&mut self, depth: usize) {
        self.current = depth;
    }

    /// Check if near limit
    pub fn is_near_limit(&self) -> bool {
        self.current >= self.warning_threshold
    }
}

/// Call stack guard for tracking function call depth
#[derive(Debug)]
pub struct CallGuard {
    /// Current call depth
    current: usize,
    /// Hard limit
    limit: usize,
}

impl CallGuard {
    /// Create a new call guard with the given limit
    pub fn new(limit: usize) -> Self {
        Self { current: 0, limit }
    }

    /// Create from VmLimits
    pub fn from_limits(limits: &VmLimits) -> Self {
        Self::new(limits.max_call_depth)
    }

    /// Enter a function call - returns error if would exceed depth
    #[inline]
    pub fn enter(&mut self) -> Result<(), VmLimitError> {
        self.current += 1;
        if self.current > self.limit {
            Err(VmLimitError::CallDepthExceeded {
                current: self.current,
                limit: self.limit,
            })
        } else {
            Ok(())
        }
    }

    /// Exit a function call
    #[inline]
    pub fn exit(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Get current depth
    pub fn depth(&self) -> usize {
        self.current
    }

    /// Set current depth directly
    pub fn set_depth(&mut self, depth: usize) {
        self.current = depth;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_default_values() {
        let limits = VmLimits::default_limits();
        assert_eq!(limits.max_stack_depth, DEFAULT_MAX_STACK_DEPTH);
        assert_eq!(limits.max_call_depth, DEFAULT_MAX_CALL_DEPTH);
        assert_eq!(limits.max_steps, DEFAULT_MAX_STEPS);
    }

    #[test]
    fn limits_minimal() {
        let limits = VmLimits::minimal();
        assert_eq!(limits.max_stack_depth, MIN_STACK_DEPTH);
        assert_eq!(limits.max_call_depth, MIN_CALL_DEPTH);
        assert_eq!(limits.max_steps, MIN_STEPS);
    }

    #[test]
    fn limits_with_methods() {
        let limits = VmLimits::default_limits()
            .with_stack_depth(500)
            .with_call_depth(100)
            .with_max_steps(5000);

        assert_eq!(limits.max_stack_depth, 500);
        assert_eq!(limits.max_call_depth, 100);
        assert_eq!(limits.max_steps, 5000);
    }

    #[test]
    fn limits_enforce_minimums() {
        let limits = VmLimits::default_limits()
            .with_stack_depth(10) // Too low
            .with_call_depth(5); // Too low

        assert_eq!(limits.max_stack_depth, MIN_STACK_DEPTH);
        assert_eq!(limits.max_call_depth, MIN_CALL_DEPTH);
    }

    #[test]
    fn check_stack_within_limit() {
        let limits = VmLimits::default_limits().with_stack_depth(1000);
        assert!(limits.check_stack(500).is_ok());
        assert!(limits.check_stack(1000).is_ok());
    }

    #[test]
    fn check_stack_exceeds_limit() {
        let limits = VmLimits::default_limits().with_stack_depth(1000);
        let err = limits.check_stack(1001).unwrap_err();
        assert!(matches!(
            err,
            VmLimitError::StackOverflow {
                current: 1001,
                limit: 1000
            }
        ));
    }

    #[test]
    fn check_call_depth_within_limit() {
        let limits = VmLimits::default_limits().with_call_depth(100);
        assert!(limits.check_call_depth(50).is_ok());
        assert!(limits.check_call_depth(100).is_ok());
    }

    #[test]
    fn check_call_depth_exceeds_limit() {
        let limits = VmLimits::default_limits().with_call_depth(100);
        let err = limits.check_call_depth(101).unwrap_err();
        assert!(matches!(
            err,
            VmLimitError::CallDepthExceeded {
                current: 101,
                limit: 100
            }
        ));
    }

    #[test]
    fn check_steps_within_limit() {
        let limits = VmLimits::default_limits().with_max_steps(10000);
        assert!(limits.check_steps(5000).is_ok());
        assert!(limits.check_steps(10000).is_ok());
    }

    #[test]
    fn check_steps_exceeds_limit() {
        let limits = VmLimits::default_limits().with_max_steps(10000);
        let err = limits.check_steps(10001).unwrap_err();
        assert!(matches!(
            err,
            VmLimitError::StepLimitExceeded {
                current: 10001,
                limit: 10000
            }
        ));
    }

    #[test]
    fn error_display_stack_overflow() {
        let err = VmLimitError::StackOverflow {
            current: 1001,
            limit: 1000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("stack overflow"));
        assert!(msg.contains("1001"));
        assert!(msg.contains("1000"));
    }

    #[test]
    fn error_display_call_depth() {
        let err = VmLimitError::CallDepthExceeded {
            current: 101,
            limit: 100,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("call depth"));
        assert!(msg.contains("101"));
        assert!(msg.contains("100"));
    }

    #[test]
    fn error_display_step_limit() {
        let err = VmLimitError::StepLimitExceeded {
            current: 10001,
            limit: 10000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("step limit"));
        assert!(msg.contains("infinite loop"));
    }

    #[test]
    fn stack_guard_basic_operations() {
        let mut guard = StackGuard::new(100);
        assert_eq!(guard.depth(), 0);

        // Push some values
        for _ in 0..50 {
            guard.push().unwrap();
        }
        assert_eq!(guard.depth(), 50);

        // Pop some values
        guard.pop_n(10);
        assert_eq!(guard.depth(), 40);
    }

    #[test]
    fn stack_guard_overflow_detection() {
        let mut guard = StackGuard::new(100);

        // Fill to limit
        for _ in 0..100 {
            guard.push().unwrap();
        }
        assert_eq!(guard.depth(), 100);

        // Overflow
        let err = guard.push().unwrap_err();
        assert!(matches!(
            err,
            VmLimitError::StackOverflow {
                current: 101,
                limit: 100
            }
        ));
    }

    #[test]
    fn stack_guard_push_n_overflow() {
        let mut guard = StackGuard::new(100);
        guard.push_n(50).unwrap();
        assert_eq!(guard.depth(), 50);

        // Try to push too many
        let err = guard.push_n(60).unwrap_err();
        assert!(matches!(err, VmLimitError::StackOverflow { .. }));
    }

    #[test]
    fn call_guard_basic_operations() {
        let mut guard = CallGuard::new(10);
        assert_eq!(guard.depth(), 0);

        // Enter some calls
        for _ in 0..5 {
            guard.enter().unwrap();
        }
        assert_eq!(guard.depth(), 5);

        // Exit some calls
        guard.exit();
        guard.exit();
        assert_eq!(guard.depth(), 3);
    }

    #[test]
    fn call_guard_depth_exceeded() {
        let mut guard = CallGuard::new(10);

        // Fill to limit
        for _ in 0..10 {
            guard.enter().unwrap();
        }

        // Exceed limit
        let err = guard.enter().unwrap_err();
        assert!(matches!(
            err,
            VmLimitError::CallDepthExceeded {
                current: 11,
                limit: 10
            }
        ));
    }

    #[test]
    fn global_limits_accessible() {
        // Just verify we can access global limits without panicking
        let limits = global_limits();
        assert!(limits.max_stack_depth >= MIN_STACK_DEPTH);
        assert!(limits.max_call_depth >= MIN_CALL_DEPTH);
        assert!(limits.max_steps >= MIN_STEPS);
    }

    #[test]
    fn would_overflow_functions() {
        // These use global limits so just verify they return boolean
        let _ = would_overflow_stack(100);
        let _ = would_exceed_call_depth(100);
        let _ = would_exceed_steps(100);
    }
}
