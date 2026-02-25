//! Integration tests for VM stack overflow and limit handling
//!
//! These tests verify that the VM properly detects and handles:
//! - Value stack overflow
//! - Call stack depth exceeded
//! - Step limit exceeded (infinite loops)

use arth_vm::limits::{
    CallGuard, DEFAULT_MAX_CALL_DEPTH, DEFAULT_MAX_STACK_DEPTH, DEFAULT_MAX_STEPS, MIN_CALL_DEPTH,
    MIN_STACK_DEPTH, MIN_STEPS, StackGuard, VmLimitError, VmLimits,
};
use arth_vm::{Op, Program, run_program};

// ============================================================================
// VmLimits Unit Tests
// ============================================================================

#[test]
fn limits_default_values() {
    let limits = VmLimits::default_limits();
    assert_eq!(limits.max_stack_depth, DEFAULT_MAX_STACK_DEPTH);
    assert_eq!(limits.max_call_depth, DEFAULT_MAX_CALL_DEPTH);
    assert_eq!(limits.max_steps, DEFAULT_MAX_STEPS);
}

#[test]
fn limits_for_testing() {
    let limits = VmLimits::for_testing();
    assert!(limits.max_stack_depth < DEFAULT_MAX_STACK_DEPTH);
    assert!(limits.max_call_depth < DEFAULT_MAX_CALL_DEPTH);
    assert!(limits.max_steps < DEFAULT_MAX_STEPS);
}

#[test]
fn limits_builder_pattern() {
    let limits = VmLimits::default_limits()
        .with_stack_depth(5000)
        .with_call_depth(500)
        .with_max_steps(50000);

    assert_eq!(limits.max_stack_depth, 5000);
    assert_eq!(limits.max_call_depth, 500);
    assert_eq!(limits.max_steps, 50000);
}

#[test]
fn limits_enforce_minimums() {
    // Values below minimum should be clamped
    let limits = VmLimits::default_limits()
        .with_stack_depth(1) // Below MIN_STACK_DEPTH
        .with_call_depth(1) // Below MIN_CALL_DEPTH
        .with_max_steps(1); // Below MIN_STEPS

    assert_eq!(limits.max_stack_depth, MIN_STACK_DEPTH);
    assert_eq!(limits.max_call_depth, MIN_CALL_DEPTH);
    assert_eq!(limits.max_steps, MIN_STEPS);
}

// ============================================================================
// Stack Overflow Check Tests
// ============================================================================

#[test]
fn check_stack_at_limit() {
    let limits = VmLimits::default_limits().with_stack_depth(1000);

    // At limit should be OK
    assert!(limits.check_stack(1000).is_ok());

    // One over should fail
    let err = limits.check_stack(1001).unwrap_err();
    match err {
        VmLimitError::StackOverflow { current, limit } => {
            assert_eq!(current, 1001);
            assert_eq!(limit, 1000);
        }
        _ => panic!("Expected StackOverflow error"),
    }
}

#[test]
fn check_call_depth_at_limit() {
    let limits = VmLimits::default_limits().with_call_depth(100);

    // At limit should be OK
    assert!(limits.check_call_depth(100).is_ok());

    // One over should fail
    let err = limits.check_call_depth(101).unwrap_err();
    match err {
        VmLimitError::CallDepthExceeded { current, limit } => {
            assert_eq!(current, 101);
            assert_eq!(limit, 100);
        }
        _ => panic!("Expected CallDepthExceeded error"),
    }
}

#[test]
fn check_steps_at_limit() {
    let limits = VmLimits::default_limits().with_max_steps(10000);

    // At limit should be OK
    assert!(limits.check_steps(10000).is_ok());

    // One over should fail
    let err = limits.check_steps(10001).unwrap_err();
    match err {
        VmLimitError::StepLimitExceeded { current, limit } => {
            assert_eq!(current, 10001);
            assert_eq!(limit, 10000);
        }
        _ => panic!("Expected StepLimitExceeded error"),
    }
}

// ============================================================================
// StackGuard Tests
// ============================================================================

#[test]
fn stack_guard_push_pop() {
    let mut guard = StackGuard::new(100);
    assert_eq!(guard.depth(), 0);

    // Push 50 values
    for _ in 0..50 {
        guard.push().unwrap();
    }
    assert_eq!(guard.depth(), 50);

    // Pop 20 values
    guard.pop_n(20);
    assert_eq!(guard.depth(), 30);

    // Single pop
    guard.pop();
    assert_eq!(guard.depth(), 29);
}

#[test]
fn stack_guard_overflow_on_push() {
    let mut guard = StackGuard::new(100);

    // Fill to limit
    guard.push_n(100).unwrap();
    assert_eq!(guard.depth(), 100);

    // Next push should fail
    let err = guard.push().unwrap_err();
    assert!(matches!(err, VmLimitError::StackOverflow { .. }));
}

#[test]
fn stack_guard_push_n_overflow() {
    let mut guard = StackGuard::new(100);
    guard.push_n(60).unwrap();

    // Pushing 50 more would exceed limit
    let err = guard.push_n(50).unwrap_err();
    assert!(matches!(err, VmLimitError::StackOverflow { .. }));

    // Depth should not have changed on failed push
    // (This is implementation-defined; StackGuard doesn't modify on failure)
}

#[test]
fn stack_guard_set_depth() {
    let mut guard = StackGuard::new(100);
    guard.push_n(50).unwrap();

    // Directly set depth (for frame restoration)
    guard.set_depth(25);
    assert_eq!(guard.depth(), 25);
}

// ============================================================================
// CallGuard Tests
// ============================================================================

#[test]
fn call_guard_enter_exit() {
    let mut guard = CallGuard::new(10);
    assert_eq!(guard.depth(), 0);

    // Enter 5 calls
    for _ in 0..5 {
        guard.enter().unwrap();
    }
    assert_eq!(guard.depth(), 5);

    // Exit 2 calls
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
    assert_eq!(guard.depth(), 10);

    // Next enter should fail
    let err = guard.enter().unwrap_err();
    assert!(matches!(err, VmLimitError::CallDepthExceeded { .. }));
}

// ============================================================================
// Error Display Tests
// ============================================================================

#[test]
fn error_display_contains_useful_info() {
    let stack_err = VmLimitError::StackOverflow {
        current: 1001,
        limit: 1000,
    };
    let msg = format!("{}", stack_err);
    assert!(msg.contains("stack overflow"));
    assert!(msg.contains("1001"));
    assert!(msg.contains("1000"));
    assert!(msg.contains("ARTH_VM_MAX_STACK_DEPTH"));

    let call_err = VmLimitError::CallDepthExceeded {
        current: 101,
        limit: 100,
    };
    let msg = format!("{}", call_err);
    assert!(msg.contains("call depth"));
    assert!(msg.contains("ARTH_VM_MAX_CALL_DEPTH"));

    let step_err = VmLimitError::StepLimitExceeded {
        current: 10001,
        limit: 10000,
    };
    let msg = format!("{}", step_err);
    assert!(msg.contains("step limit"));
    assert!(msg.contains("infinite loop"));
    assert!(msg.contains("ARTH_VM_MAX_STEPS"));
}

// ============================================================================
// VM Integration Tests - Programs that test overflow handling
// ============================================================================

/// Test: Simple program that runs within limits should succeed
#[test]
fn vm_simple_program_within_limits() {
    let code = vec![
        Op::PushI64(1),
        Op::PushI64(2),
        Op::AddI64,
        Op::Pop,
        Op::Halt,
    ];
    let p = Program::new(vec![], code);
    let exit = run_program(&p);
    assert_eq!(exit, 0);
}

/// Test: Program with nested function calls within limits
#[test]
fn vm_nested_calls_within_limits() {
    // Create a program with 10 nested calls
    // Layout: main calls f1, f1 calls f2, ..., f9 returns
    let mut code = vec![];

    // Main: call f1, then halt
    code.push(Op::Call(3)); // 0: call f1 at index 3
    code.push(Op::Pop); // 1
    code.push(Op::Halt); // 2

    // f1 through f8: call next, then return
    for i in 0..8 {
        let next_fn = 3 + (i + 1) * 3;
        code.push(Op::Call(next_fn as u32));
        code.push(Op::Pop);
        code.push(Op::Ret);
    }

    // f9: just return
    code.push(Op::Ret);

    let p = Program::new(vec![], code);
    let exit = run_program(&p);
    assert_eq!(exit, 0);
}

/// Test: Program with many pushes/pops within limits
#[test]
fn vm_stack_operations_within_limits() {
    let mut code = vec![];

    // Push 100 values
    for i in 0..100 {
        code.push(Op::PushI64(i));
    }

    // Pop them all
    for _ in 0..100 {
        code.push(Op::Pop);
    }

    code.push(Op::Halt);

    let p = Program::new(vec![], code);
    let exit = run_program(&p);
    assert_eq!(exit, 0);
}

/// Test: Loop that completes within step limits
#[test]
fn vm_loop_within_step_limits() {
    // Loop 100 times
    let code = vec![
        Op::PushI64(0),      // 0: counter
        Op::LocalSet(0),     // 1
        Op::LocalGet(0),     // 2: loop start
        Op::PushI64(100),    // 3
        Op::LtI64,           // 4
        Op::JumpIfFalse(12), // 5: exit if counter >= 100
        Op::LocalGet(0),     // 6
        Op::PushI64(1),      // 7
        Op::AddI64,          // 8
        Op::LocalSet(0),     // 9
        Op::Jump(2),         // 10: back to loop start
        Op::Halt,            // 11
        Op::Halt,            // 12: exit point
    ];

    let p = Program::new(vec![], code);
    let exit = run_program(&p);
    assert_eq!(exit, 0);
}

// ============================================================================
// Error Equality Tests
// ============================================================================

#[test]
fn vm_limit_error_equality() {
    let err1 = VmLimitError::StackOverflow {
        current: 100,
        limit: 50,
    };
    let err2 = VmLimitError::StackOverflow {
        current: 100,
        limit: 50,
    };
    let err3 = VmLimitError::StackOverflow {
        current: 101,
        limit: 50,
    };

    assert_eq!(err1, err2);
    assert_ne!(err1, err3);

    let call_err = VmLimitError::CallDepthExceeded {
        current: 100,
        limit: 50,
    };
    assert_ne!(err1, call_err);
}

// ============================================================================
// Guard from Limits Tests
// ============================================================================

#[test]
fn guards_from_limits() {
    let limits = VmLimits::for_testing();

    let stack_guard = StackGuard::from_limits(&limits);
    assert_eq!(stack_guard.depth(), 0);

    let call_guard = CallGuard::from_limits(&limits);
    assert_eq!(call_guard.depth(), 0);
}
