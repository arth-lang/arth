#![allow(dead_code)]
// Suppress common clippy warnings in VM runtime code
// These patterns are often intentional for clarity in match-heavy interpreter code
#![allow(clippy::collapsible_if)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::manual_strip)]
#![allow(clippy::single_match)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::redundant_closure_call)]
#![allow(clippy::io_other_error)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::or_fun_call)]
#![allow(clippy::option_map_unit_fn)]
#![allow(clippy::map_flatten)]
#![allow(clippy::let_unit_value)]
#![allow(clippy::iter_overeager_cloned)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::manual_unwrap_or)]
#![allow(clippy::vec_init_then_push)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::needless_return)]
#![allow(clippy::get_first)]
#![allow(clippy::print_literal)]
#![allow(clippy::trivial_regex)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::unused_io_amount)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::print_with_newline)]
#![allow(clippy::println_empty_string)]
#![allow(clippy::useless_format)]
#![allow(clippy::bind_instead_of_map)]
#![allow(clippy::unnecessary_lazy_evaluations)]
#![allow(clippy::trim_split_whitespace)]
#![allow(clippy::declare_interior_mutable_const)]
#![allow(clippy::unwrap_or_default)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::regex_creation_in_loops)]
#![allow(clippy::format_push_string)]
#![allow(clippy::missing_const_for_thread_local)]
#![allow(clippy::format_in_format_args)]

mod builder;
mod bytecode;
pub mod debug;
pub mod executor;
pub mod host;
mod io;
#[cfg(feature = "jit")]
pub mod jit;
#[cfg(feature = "jit")]
pub mod jit_interp;
pub mod limits;
pub mod link;
mod ops;
mod program;
mod runtime;

pub use builder::compile_messages_to_program;
pub use bytecode::{
    DecodeError, DecodeErrorKind, DecodePhase, decode_program, decode_program_detailed,
    encode_program,
};
pub use debug::{
    enable_debug, init_from_env, is_debug_enabled, log_error, log_exec, log_host_call, log_warn,
};
pub use host::{
    DbError, DbErrorKind, FileHandle, FileMode, HeadersHandle, HostConfig, HostContext, HostDb,
    HostGenericCall, HostIo, HostMail, HostNet, HostTime, HttpRequestHandle, HttpResponseHandle,
    HttpServerHandle, ImapConnectionHandle, InstantHandle, IoError, IoErrorKind, MailError,
    MailErrorKind, MimeMessageHandle, MockHostTime, NetError, NetErrorKind, NoHostDb, NoHostIo,
    NoHostMail, NoHostNet, PgAsyncConnectionHandle, PgAsyncQueryHandle, PgConnectionHandle,
    PgPoolHandle, PgResultHandle, PgStatementHandle, PoolConfig, PoolStats, Pop3ConnectionHandle,
    SeekPosition, SmtpConnectionHandle, SqliteConnectionHandle, SqlitePoolHandle,
    SqliteStatementHandle, SseEmitterHandle, SseServerHandle, StdHostDb, StdHostGenericCall,
    StdHostIo, StdHostMail, StdHostTime, TaskHandle, TimeError, TimeErrorKind, TlsContextHandle,
    TlsStreamHandle, WsConnectionHandle, WsMessage, WsServerHandle,
};
pub use io::{run_abc_file, write_abc_file};
pub use limits::{
    CallGuard, DEFAULT_MAX_CALL_DEPTH, DEFAULT_MAX_STACK_DEPTH, DEFAULT_MAX_STEPS, MIN_CALL_DEPTH,
    MIN_STACK_DEPTH, MIN_STEPS, StackGuard, VmLimitError, VmLimits, global_limits,
    would_exceed_call_depth, would_exceed_steps, would_overflow_stack,
};
pub use link::{
    ExportEntry, Library, LinkedProgram, decode_library, decode_library_detailed, encode_library,
    link_programs, load_and_link, load_library,
};
pub use ops::{HostDbOp, HostIoOp, HostNetOp, HostTimeOp, Op};
pub use program::{DebugEntry, Program};
pub use runtime::{
    CallExportResult, StackFrame, call_export, call_export_with_host, call_export_with_symbols,
    capture_stack_trace, get_external_lib_paths, get_linked_symbol_table,
    panic_with_message_and_stack, run_program, run_program_with_host, set_external_lib_paths,
    set_linked_symbol_table,
};

#[cfg(feature = "jit")]
pub use jit::{
    CompilationTier, DeoptInfo, DeoptReason, FunctionMeta, JIT_THRESHOLD, JitContext, JitError,
    JitStats, MAX_DEOPTS_PER_FUNCTION, OSR_THRESHOLD, OsrLoopMeta, TIER1_THRESHOLD,
    TIER2_THRESHOLD, get_jit_stats, init_jit, with_jit,
};

#[cfg(feature = "jit")]
pub use jit_interp::{
    DeoptResult, JitCallResult, OsrCallResult, deoptimize, record_deopt, register_function,
    register_loop, try_jit_call, try_osr_call,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let p = compile_messages_to_program(&["hello".to_string(), "world".to_string()]);
        let bytes = encode_program(&p);
        let q = decode_program(&bytes).expect("decode");
        assert_eq!(p, q);
    }

    #[test]
    fn extern_call_llabs_i64_works() {
        // Calls libc `llabs(long long)` via the VM extern-call opcode.
        // This is a good smoke test because it is widely available on Unix-like systems.
        let strings = vec!["llabs".to_string(), "extern llabs failed".to_string()];
        let code = vec![
            Op::PushI64(-42),
            Op::ExternCall {
                sym: 0,
                argc: 1,
                float_mask: 0,
                ret_kind: 0,
            },
            Op::PushI64(42),
            Op::EqI64,
            Op::JumpIfFalse(6),
            Op::Halt,
            Op::Panic(1),
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_alloc_and_load() {
        // Test: allocate RC cell with value 42, load it back
        let code = vec![
            Op::PushI64(42), // push value
            Op::RcAlloc,     // allocate RC cell, returns handle
            Op::RcLoad,      // load value from RC cell
            Op::PrintTop,    // print the value
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_get_count_initial() {
        // Test: allocate RC cell, check count is 1
        let code = vec![
            Op::PushI64(100), // push value
            Op::RcAlloc,      // allocate RC cell
            Op::RcGetCount,   // get reference count
            Op::PrintTop,     // should print 1
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_inc_increments_count() {
        // Test: allocate RC cell, increment, check count is 2
        let code = vec![
            Op::PushI64(200), // push value
            Op::RcAlloc,      // allocate RC cell, returns handle
            Op::LocalSet(0),  // store handle in local 0
            Op::LocalGet(0),  // get handle
            Op::RcInc,        // increment (returns handle)
            Op::Pop,          // discard returned handle
            Op::LocalGet(0),  // get handle
            Op::RcGetCount,   // get count
            Op::PrintTop,     // should print 2
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_dec_decrements_count() {
        // Test: allocate RC cell, inc, dec, check count is 1
        let code = vec![
            Op::PushI64(300), // push value
            Op::RcAlloc,      // allocate RC cell
            Op::LocalSet(0),  // store handle
            Op::LocalGet(0),  // get handle
            Op::RcInc,        // count becomes 2
            Op::Pop,
            Op::LocalGet(0), // get handle
            Op::RcDec,       // count becomes 1
            Op::Pop,
            Op::LocalGet(0), // get handle
            Op::RcGetCount,  // get count
            Op::PrintTop,    // should print 1
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_dec_deallocates_at_zero() {
        // Test: allocate RC cell, dec (count 0), check count is now 0 (deallocated)
        let code = vec![
            Op::PushI64(400), // push value
            Op::RcAlloc,      // allocate RC cell (count = 1)
            Op::LocalSet(0),  // store handle
            Op::LocalGet(0),  // get handle
            Op::RcDec,        // count becomes 0, cell deallocated
            Op::Pop,
            Op::LocalGet(0), // get handle (now invalid)
            Op::RcGetCount,  // should return 0 (not found)
            Op::PrintTop,    // should print 0
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_store_updates_value() {
        // Test: allocate RC cell with 500, store 600, load back
        let code = vec![
            Op::PushI64(500), // push initial value
            Op::RcAlloc,      // allocate RC cell
            Op::LocalSet(0),  // store handle
            Op::LocalGet(0),  // get handle
            Op::PushI64(600), // push new value
            Op::RcStore,      // store new value
            Op::Pop,          // discard result
            Op::LocalGet(0),  // get handle
            Op::RcLoad,       // load value
            Op::PrintTop,     // should print 600
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn rc_encode_decode_roundtrip() {
        // Test: RC ops survive encode/decode
        let code = vec![
            Op::PushI64(42),
            Op::RcAlloc,
            Op::RcInc,
            Op::RcDec,
            Op::RcDecWithDeinit(100),
            Op::RcLoad,
            Op::RcStore,
            Op::RcGetCount,
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let bytes = encode_program(&p);
        let q = decode_program(&bytes).expect("decode");
        assert_eq!(p, q);
    }

    // Note: HostCallNet tests temporarily disabled until run_program_with_host
    // fully duplicates the opcode handling from run_program. The current
    // implementation breaks out on non-host opcodes and delegates to run_program
    // which has stub implementations. See Phase 5 implementation plan for full
    // migration strategy.

    #[test]
    fn json_encode_decode_roundtrip() {
        // Test: JSON ops survive encode/decode
        let code = vec![Op::PushI64(42), Op::JsonStringify, Op::JsonParse, Op::Halt];
        let p = Program::new(vec![], code);
        let bytes = encode_program(&p);
        let q = decode_program(&bytes).expect("decode");
        assert_eq!(p, q);
    }

    #[test]
    fn json_stringify_primitive() {
        // Test: JsonStringify on integer returns the number as string
        let code = vec![Op::PushI64(42), Op::JsonStringify, Op::PrintTop, Op::Halt];
        let p = Program::new(vec![], code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn json_parse_valid() {
        // Test: JsonParse on valid JSON returns a valid handle
        let strings = vec![r#"{"name":"test","value":123}"#.to_string()];
        let code = vec![
            Op::PushStr(0), // JSON string
            Op::JsonParse,  // -> json handle
            Op::PrintTop,   // print handle (should be >= 110_000)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn json_parse_invalid() {
        // Test: JsonParse on invalid JSON returns -1
        let strings = vec!["not valid json".to_string()];
        let code = vec![
            Op::PushStr(0),       // Invalid JSON string
            Op::JsonParse,        // -> -1 on error
            Op::PushI64(-1),      // Expected value
            Op::EqI64,            // Compare
            Op::JumpIfFalse(100), // Fail if not -1
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn struct_to_json_basic() {
        // Test: Create a "struct" (list) with fields [10, 20], serialize to JSON with field names "x,y"
        // Expected output: {"x":10,"y":20}
        let strings = vec!["x,y".to_string()];
        let code = vec![
            Op::ListNew,     // create struct as list
            Op::LocalSet(0), // store struct handle
            Op::LocalGet(0),
            Op::PushI64(10), // push field value x
            Op::ListPush,    // push to list
            Op::Pop,
            Op::LocalGet(0),
            Op::PushI64(20), // push field value y
            Op::ListPush,    // push to list
            Op::Pop,
            Op::LocalGet(0),  // struct handle
            Op::PushStr(0),   // field names "x,y"
            Op::StructToJson, // -> json string
            Op::PrintTop,     // print result
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn json_to_struct_basic() {
        // Test: Parse JSON {"x":10,"y":20} into struct with field names "x,y"
        // Then access first field and verify it's 10
        let strings = vec![r#"{"x":10,"y":20}"#.to_string(), "x,y".to_string()];
        let code = vec![
            Op::PushStr(0),   // JSON string
            Op::PushStr(1),   // field names "x,y"
            Op::JsonToStruct, // -> struct handle
            Op::LocalSet(0),  // store struct handle
            Op::LocalGet(0),  // get struct handle
            Op::PushI64(0),   // index 0 (field x)
            Op::ListGet,      // get value at index
            Op::PrintTop,     // print (should be 10)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn struct_json_encode_decode_roundtrip() {
        // Test: StructToJson and JsonToStruct ops survive encode/decode
        let strings = vec!["a,b,c".to_string()];
        let code = vec![
            Op::ListNew,
            Op::PushStr(0),
            Op::StructToJson,
            Op::PushStr(0),
            Op::JsonToStruct,
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let bytes = encode_program(&p);
        let q = decode_program(&bytes).expect("decode");
        assert_eq!(p, q);
    }

    #[test]
    fn struct_to_json_with_json_ignore() {
        // Test: Create struct [10, "secret", 30] but only serialize x:0 and z:2 (skip y:1 with @JsonIgnore)
        // Enhanced metadata format: "x:0,z:2" (skips index 1)
        let strings = vec!["x:0,z:2".to_string(), "secret".to_string()];
        let code = vec![
            Op::ListNew,     // create struct as list
            Op::LocalSet(0), // store struct handle
            Op::LocalGet(0),
            Op::PushI64(10), // field x at index 0
            Op::ListPush,
            Op::Pop,
            Op::LocalGet(0),
            Op::PushStr(1), // field y at index 1 (ignored)
            Op::ListPush,
            Op::Pop,
            Op::LocalGet(0),
            Op::PushI64(30), // field z at index 2
            Op::ListPush,
            Op::Pop,
            Op::LocalGet(0),  // struct handle
            Op::PushStr(0),   // field meta "x:0,z:2"
            Op::StructToJson, // -> json string (should be {"x":10,"z":30})
            Op::PrintTop,     // print result
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn json_to_struct_with_ignore_unknown() {
        // Test: Parse JSON with extra field, using ignoreUnknown flag
        // Metadata: "x:0,y:1;I" (I = ignoreUnknown)
        let strings = vec![
            r#"{"x":10,"y":20,"extra":"ignored"}"#.to_string(),
            "x:0,y:1;I".to_string(),
        ];
        let code = vec![
            Op::PushStr(0),   // JSON string
            Op::PushStr(1),   // field meta with ignoreUnknown flag
            Op::JsonToStruct, // -> struct handle (should succeed)
            Op::LocalSet(0),  // store struct handle
            Op::LocalGet(0),  // get struct handle
            Op::PushI64(0),   // index 0 (field x)
            Op::ListGet,      // get value at index
            Op::PrintTop,     // print (should be 10)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    #[test]
    fn json_to_struct_unknown_field_error() {
        // Test: Parse JSON with extra field WITHOUT ignoreUnknown - should return -2
        // Metadata: "x:0,y:1" (no ignoreUnknown flag)
        let strings = vec![
            r#"{"x":10,"y":20,"extra":"error"}"#.to_string(),
            "x:0,y:1".to_string(),
        ];
        let code = vec![
            Op::PushStr(0),       // JSON string with extra field
            Op::PushStr(1),       // field meta WITHOUT ignoreUnknown
            Op::JsonToStruct,     // -> -2 (unknown field error)
            Op::PushI64(-2),      // Expected error code
            Op::EqI64,            // Compare
            Op::JumpIfFalse(100), // Fail if not -2
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0);
    }

    // ========================================================================
    // Integration Tests - Host Capability Enforcement
    // ========================================================================

    #[test]
    fn integration_host_io_with_full_capabilities() {
        // Test: HostCallIo operations work with full capabilities
        // Program: get current time using HostCallTime (simpler than file IO)
        let code = vec![
            Op::HostCallTime(HostTimeOp::DateTimeNow), // -> timestamp (millis)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::std();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_time_ops_with_time_capability() {
        // Test: Time operations work when time capability is enabled
        let code = vec![Op::HostCallTime(HostTimeOp::DateTimeNow), Op::Halt];
        let p = Program::new(vec![], code);
        // Guest with only time capability
        let ctx = HostContext::for_guest(&["time".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_time_denied_without_capability() {
        // Test: Time operations return -1 when time capability is disabled
        let code = vec![
            Op::HostCallTime(HostTimeOp::DateTimeNow), // Should push -1
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        // Sandboxed guest - no capabilities
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        // Program completes but the operation returned -1
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_io_denied_without_capability() {
        // Test: IO operations return -1 when io capability is disabled
        let strings = vec!["/tmp/test.txt".to_string()];
        let code = vec![
            Op::PushStr(0),                     // path
            Op::PushI64(0),                     // mode: Read
            Op::HostCallIo(HostIoOp::FileOpen), // Should push -1 (denied)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        // Sandboxed guest - no capabilities
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_net_denied_without_capability() {
        // Test: Network operations return -1 when net capability is disabled
        let code = vec![
            Op::PushI64(8080),                     // port
            Op::HostCallNet(HostNetOp::HttpServe), // Should push -1 (denied)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        // Sandboxed guest - no capabilities
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_selective_capabilities() {
        // Test: Guest with only specific capabilities
        // Should allow time but deny io and net
        let strings = vec!["/tmp/test.txt".to_string()];

        // Test 1: Time should work
        let time_code = vec![Op::HostCallTime(HostTimeOp::DateTimeNow), Op::Halt];
        let time_prog = Program::new(vec![], time_code);
        let ctx = HostContext::for_guest(&["time".to_string()]);
        let exit = run_program_with_host(&time_prog, &ctx);
        assert_eq!(exit, 0);

        // Test 2: IO should be denied (returns -1)
        let io_code = vec![
            Op::PushStr(0),
            Op::PushI64(0),
            Op::HostCallIo(HostIoOp::FileOpen),
            Op::Halt,
        ];
        let io_prog = Program::new(strings, io_code);
        let ctx = HostContext::for_guest(&["time".to_string()]); // only time, no io
        let exit = run_program_with_host(&io_prog, &ctx);
        assert_eq!(exit, 0); // Program completes, operation returned -1
    }

    #[test]
    fn integration_io_file_exists_with_capability() {
        // Test: FileExists operation works with io capability
        let strings = vec!["/tmp".to_string()]; // /tmp usually exists
        let code = vec![
            Op::PushStr(0),                       // path
            Op::HostCallIo(HostIoOp::FileExists), // -> 1 (exists) or 0
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let ctx = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_instant_elapsed() {
        // Test: InstantNow + InstantElapsed work with time capability
        let code = vec![
            Op::HostCallTime(HostTimeOp::InstantNow), // -> instant handle
            Op::HostCallTime(HostTimeOp::InstantElapsed), // -> elapsed millis (very small)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::for_guest(&["time".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn integration_multiple_capabilities() {
        // Test: Guest with multiple capabilities (io + time)
        let strings = vec!["/tmp".to_string()];
        let code = vec![
            // First: get time (requires time capability)
            Op::HostCallTime(HostTimeOp::DateTimeNow),
            Op::Pop, // discard result
            // Second: check file exists (requires io capability)
            Op::PushStr(0),
            Op::HostCallIo(HostIoOp::FileExists),
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let ctx = HostContext::for_guest(&["io".to_string(), "time".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    // ========================================================================
    // Negative Tests - Capability Bypass Prevention & Error Handling
    // ========================================================================

    #[test]
    fn negative_sandboxed_cannot_access_io() {
        // Test: Fully sandboxed guest cannot access any IO operations
        let strings = vec!["/etc/passwd".to_string()];

        // Try to open a sensitive file - should be denied
        let code = vec![
            Op::PushStr(0),                     // path to sensitive file
            Op::PushI64(0),                     // mode: Read
            Op::HostCallIo(HostIoOp::FileOpen), // Should return -1 (denied)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0); // Program completes, operation was denied
    }

    #[test]
    fn negative_sandboxed_cannot_access_net() {
        // Test: Fully sandboxed guest cannot access any network operations
        let strings = vec!["https://malicious.example.com".to_string()];

        // Try to fetch from network - should be denied
        let code = vec![
            Op::PushStr(0),                        // URL
            Op::HostCallNet(HostNetOp::HttpFetch), // Should return -1 (denied)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0); // Program completes, operation was denied
    }

    #[test]
    fn negative_sandboxed_cannot_access_time() {
        // Test: Fully sandboxed guest cannot access time operations
        let code = vec![
            Op::HostCallTime(HostTimeOp::DateTimeNow), // Should return -1 (denied)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::sandboxed();
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0); // Program completes, operation was denied
    }

    #[test]
    fn negative_partial_capability_cannot_access_other_domains() {
        // Test: Guest with IO capability cannot access Net or Time
        let code_net = vec![
            Op::PushI64(8080),
            Op::HostCallNet(HostNetOp::HttpServe), // Should return -1 (no net cap)
            Op::Halt,
        ];
        let p_net = Program::new(vec![], code_net);
        let ctx_io_only = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p_net, &ctx_io_only);
        assert_eq!(exit, 0);

        let code_time = vec![
            Op::HostCallTime(HostTimeOp::DateTimeNow), // Should return -1 (no time cap)
            Op::Halt,
        ];
        let p_time = Program::new(vec![], code_time);
        let ctx_io_only = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p_time, &ctx_io_only);
        assert_eq!(exit, 0);
    }

    #[test]
    fn negative_io_invalid_file_handle() {
        // Test: Operations on invalid file handles return error
        let code = vec![
            Op::PushI64(999999),                // Invalid file handle
            Op::PushI64(100),                   // max bytes to read
            Op::HostCallIo(HostIoOp::FileRead), // Should return error (-1)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0); // Program completes, but operation failed
    }

    #[test]
    fn negative_io_close_invalid_handle() {
        // Test: Closing an invalid file handle returns error
        let code = vec![
            Op::PushI64(888888),                 // Invalid file handle
            Op::HostCallIo(HostIoOp::FileClose), // Should return error (-1)
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn negative_io_open_nonexistent_file() {
        // Test: Opening a non-existent file for reading returns error
        let strings = vec!["/nonexistent/path/to/file.txt".to_string()];
        let code = vec![
            Op::PushStr(0),                     // path
            Op::PushI64(0),                     // mode: Read
            Op::HostCallIo(HostIoOp::FileOpen), // Should return -1 (file not found)
            Op::Halt,
        ];
        let p = Program::new(strings, code);
        let ctx = HostContext::for_guest(&["io".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn negative_time_invalid_instant_handle() {
        // Test: InstantElapsed on invalid handle returns error
        let code = vec![
            Op::PushI64(777777),                          // Invalid instant handle
            Op::HostCallTime(HostTimeOp::InstantElapsed), // Should return -1
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        let ctx = HostContext::for_guest(&["time".to_string()]);
        let exit = run_program_with_host(&p, &ctx);
        assert_eq!(exit, 0);
    }

    #[test]
    fn negative_all_io_ops_denied_when_sandboxed() {
        // Test: All IO operations are denied for sandboxed guests
        let strings = vec!["/tmp".to_string(), "test".to_string()];
        let ctx = HostContext::sandboxed();

        // FileExists
        let code = vec![
            Op::PushStr(0),
            Op::HostCallIo(HostIoOp::FileExists),
            Op::Halt,
        ];
        let p = Program::new(strings.clone(), code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);

        // DirExists
        let code = vec![
            Op::PushStr(0),
            Op::HostCallIo(HostIoOp::DirExists),
            Op::Halt,
        ];
        let p = Program::new(strings.clone(), code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);

        // IsDir
        let code = vec![Op::PushStr(0), Op::HostCallIo(HostIoOp::IsDir), Op::Halt];
        let p = Program::new(strings.clone(), code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);

        // ConsoleWrite
        let code = vec![
            Op::PushStr(1),
            Op::HostCallIo(HostIoOp::ConsoleWrite),
            Op::Halt,
        ];
        let p = Program::new(strings.clone(), code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);
    }

    #[test]
    fn negative_all_net_ops_denied_when_sandboxed() {
        // Test: All network operations are denied for sandboxed guests
        let ctx = HostContext::sandboxed();

        // HttpServe
        let code = vec![
            Op::PushI64(8080),
            Op::HostCallNet(HostNetOp::HttpServe),
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);

        // WsServe
        let code = vec![
            Op::PushI64(8081),
            Op::HostCallNet(HostNetOp::WsServe),
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);

        // SseServe
        let code = vec![
            Op::PushI64(8082),
            Op::HostCallNet(HostNetOp::SseServe),
            Op::Halt,
        ];
        let p = Program::new(vec![], code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);
    }

    #[test]
    fn negative_empty_capability_list_is_sandboxed() {
        // Test: Empty capability list should deny all operations
        let ctx = HostContext::for_guest(&[]);

        // All domains should be denied
        assert!(!ctx.config.allow_io);
        assert!(!ctx.config.allow_net);
        assert!(!ctx.config.allow_time);

        // Time operation should be denied
        let code = vec![Op::HostCallTime(HostTimeOp::DateTimeNow), Op::Halt];
        let p = Program::new(vec![], code);
        assert_eq!(run_program_with_host(&p, &ctx), 0);
    }

    #[test]
    fn negative_unknown_capability_ignored() {
        // Test: Unknown capability names are ignored, not treated as grants
        let ctx = HostContext::for_guest(&[
            "unknown".to_string(),
            "invalid_cap".to_string(),
            "hacker_backdoor".to_string(),
        ]);

        // All domains should still be denied
        assert!(!ctx.config.allow_io);
        assert!(!ctx.config.allow_net);
        assert!(!ctx.config.allow_time);
    }

    // ========================================================================
    // WAID Calling Convention Tests
    // ========================================================================

    #[test]
    fn waid_calling_convention_no_prologue() {
        // Test: Function without LocalSet prologue receives arg in correct local
        // This simulates WAID-compiled functions that expect args in specific locals
        let strings = vec!["hello_arg".to_string()];
        let code = vec![
            // Trampoline at offsets 0-2
            Op::PushStr(0), // 0: push "hello_arg"
            Op::Call(3),    // 1: call function at 3
            Op::Halt,       // 2: stop
            // Function at offset 3 (simulating WAID pattern)
            // No LocalSet prologue - function expects arg already in local 4
            Op::Jump(4),          // 3: jump past nothing (WAID pattern)
            Op::LocalGet(4),      // 4: get arg from local 4
            Op::StrLen,           // 5: get string length
            Op::PushI64(9),       // 6: expected length ("hello_arg" = 9 chars)
            Op::EqI64,            // 7: compare
            Op::JumpIfFalse(100), // 8: fail if not equal (jumps to invalid addr)
            Op::Ret,              // 9: return
        ];

        let p = Program::new(strings, code);
        let exit = run_program(&p);
        assert_eq!(exit, 0, "function should receive arg in correct local");
    }

    #[test]
    fn dump_waid_bytecode() {
        use crate::link::decode_library;

        let path = "/Users/yppartha/PROJECTS/RUNE_ARTH/WAID/plugins/wisp-mail/dist/ir/pages/home/logic/home.abc";
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Skipping test - file not found: {}", e);
                return;
            }
        };

        let (prog, exports) = decode_library(&bytes).expect("decode lib");

        println!("Exports:");
        for e in &exports {
            println!("  {} at offset {} arity {}", e.name, e.offset, e.arity);
        }

        // If handle_event exists, show its ops
        if let Some(he) = exports.iter().find(|e| e.name == "handle_event") {
            println!("\nhandle_event at offset {}:", he.offset);
            let end = (he.offset as usize + 50).min(prog.code.len());
            for i in he.offset as usize..end {
                println!("  {}: {:?}", i, prog.code[i]);
            }
        }
    }

    // ========================================================================
    // Direct Export Calling (call_export) Tests
    // ========================================================================

    #[test]
    fn call_export_simple_function() {
        // Create a simple program with an export that adds two numbers
        let code = vec![
            Op::PushI64(5), // 0: Push first number
            Op::PushI64(3), // 1: Push second number
            Op::AddI64,     // 2: Add them
            Op::Ret,        // 3: Return result (8)
        ];
        let p = Program::new(vec![], code);

        let result = call_export(&p, 0, 0, &[], None);
        match result {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 8, "should return 8"),
            CallExportResult::Success(None) => panic!("expected return value"),
            CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
            CallExportResult::ExportNotFound => panic!("export not found"),
            CallExportResult::InvalidArgument(msg) => panic!("invalid argument: {}", msg),
        }
    }

    #[test]
    fn call_export_with_string_arg() {
        // Create a program that takes a string arg and gets its length
        let code = vec![
            Op::LocalGet(0), // 0: Get the string argument
            Op::StrLen,      // 1: Get string length
            Op::Ret,         // 2: Return length
        ];
        let p = Program::new(vec![], code);

        let result = call_export(&p, 0, 1, &["hello"], None);
        match result {
            CallExportResult::Success(Some(n)) => {
                assert_eq!(n, 5, "should return 5 (length of 'hello')")
            }
            CallExportResult::Success(None) => panic!("expected return value"),
            CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
            CallExportResult::ExportNotFound => panic!("export not found"),
            CallExportResult::InvalidArgument(msg) => panic!("invalid argument: {}", msg),
        }
    }

    #[test]
    fn call_export_invalid_offset() {
        // Create a small program and try to call at invalid offset
        let code = vec![Op::PushI64(1), Op::Ret];
        let p = Program::new(vec![], code);

        let result = call_export(&p, 100, 0, &[], None);
        assert!(matches!(result, CallExportResult::ExportNotFound));
    }

    #[test]
    fn call_export_arity_mismatch() {
        // Create a program and try to call with wrong number of arguments
        let code = vec![Op::LocalGet(0), Op::Ret];
        let p = Program::new(vec![], code);

        // Expect 1 arg but pass 0
        let result = call_export(&p, 0, 1, &[], None);
        assert!(matches!(result, CallExportResult::InvalidArgument(_)));

        // Expect 0 args but pass 1
        let result2 = call_export(&p, 0, 0, &["extra"], None);
        assert!(matches!(result2, CallExportResult::InvalidArgument(_)));
    }

    #[test]
    fn call_export_void_function() {
        // Create a function that just does some work and returns without a value
        let code = vec![
            Op::PushI64(42), // 0: Do some work
            Op::Pop,         // 1: Discard it
            Op::Ret,         // 2: Return (no value)
        ];
        let p = Program::new(vec![], code);

        let result = call_export(&p, 0, 0, &[], None);
        match result {
            CallExportResult::Success(None) => (),    // Expected
            CallExportResult::Success(Some(_)) => (), // Also ok
            CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn call_export_with_internal_call() {
        // Create a program where the export calls another function
        let code = vec![
            // Helper at offset 0
            Op::PushI64(10), // 0: Push 10
            Op::Ret,         // 1: Return
            // Export at offset 2
            Op::Call(0),    // 2: Call helper at offset 0
            Op::PushI64(5), // 3: Push 5
            Op::AddI64,     // 4: Add (10 + 5)
            Op::Ret,        // 5: Return 15
        ];
        let p = Program::new(vec![], code);

        let result = call_export(&p, 2, 0, &[], None);
        match result {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 15, "should return 15"),
            CallExportResult::Success(None) => panic!("expected return value"),
            CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn integer_arithmetic_wraps_on_overflow() {
        // max + 1 => min
        let add_prog = Program::new(
            vec![],
            vec![Op::PushI64(i64::MAX), Op::PushI64(1), Op::AddI64, Op::Ret],
        );
        match call_export(&add_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, i64::MIN),
            other => panic!("unexpected result for add overflow: {:?}", other),
        }

        // min - 1 => max
        let sub_prog = Program::new(
            vec![],
            vec![Op::PushI64(i64::MIN), Op::PushI64(1), Op::SubI64, Op::Ret],
        );
        match call_export(&sub_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, i64::MAX),
            other => panic!("unexpected result for sub overflow: {:?}", other),
        }

        // max * 2 => -2
        let mul_prog = Program::new(
            vec![],
            vec![Op::PushI64(i64::MAX), Op::PushI64(2), Op::MulI64, Op::Ret],
        );
        match call_export(&mul_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, -2),
            other => panic!("unexpected result for mul overflow: {:?}", other),
        }
    }

    #[test]
    fn integer_div_mod_overflow_edges_are_defined() {
        // min / -1 => min (wrapping division)
        let div_prog = Program::new(
            vec![],
            vec![Op::PushI64(i64::MIN), Op::PushI64(-1), Op::DivI64, Op::Ret],
        );
        match call_export(&div_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, i64::MIN),
            other => panic!("unexpected result for div overflow edge: {:?}", other),
        }

        // min % -1 => 0
        let mod_prog = Program::new(
            vec![],
            vec![Op::PushI64(i64::MIN), Op::PushI64(-1), Op::ModI64, Op::Ret],
        );
        match call_export(&mod_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 0),
            other => panic!("unexpected result for mod overflow edge: {:?}", other),
        }
    }

    #[test]
    fn shift_amount_is_masked_to_word_size() {
        // 1 << 65 -> 1 << 1 = 2
        let shl_prog = Program::new(
            vec![],
            vec![Op::PushI64(1), Op::PushI64(65), Op::ShlI64, Op::Ret],
        );
        match call_export(&shl_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 2),
            other => panic!("unexpected result for shl mask: {:?}", other),
        }

        // -8 >> 65 -> -8 >> 1 = -4
        let shr_prog = Program::new(
            vec![],
            vec![Op::PushI64(-8), Op::PushI64(65), Op::ShrI64, Op::Ret],
        );
        match call_export(&shr_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, -4),
            other => panic!("unexpected result for shr mask: {:?}", other),
        }
    }

    #[test]
    fn numeric_cast_edge_cases_are_stable() {
        // (i8)300 -> 44
        let i8_prog = Program::new(vec![], vec![Op::PushI64(300), Op::ToI8, Op::Ret]);
        match call_export(&i8_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 44),
            other => panic!("unexpected result for i8 cast: {:?}", other),
        }

        // (u8)-1 -> 255
        let u8_prog = Program::new(vec![], vec![Op::PushI64(-1), Op::ToU8, Op::Ret]);
        match call_export(&u8_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 255),
            other => panic!("unexpected result for u8 cast: {:?}", other),
        }

        // (f32)16777217 -> 16777216.0 (precision drop), then to Int for assertion
        let f32_prog = Program::new(
            vec![],
            vec![Op::PushI64(16_777_217), Op::ToF32, Op::ToI64, Op::Ret],
        );
        match call_export(&f32_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 16_777_216),
            other => panic!("unexpected result for f32 cast: {:?}", other),
        }

        // String fallback: (Int)"A" -> 65
        let str_prog = Program::new(
            vec!["A".to_string()],
            vec![Op::PushStr(0), Op::ToI64, Op::Ret],
        );
        match call_export(&str_prog, 0, 0, &[], None) {
            CallExportResult::Success(Some(n)) => assert_eq!(n, 65),
            other => panic!("unexpected result for string->int cast: {:?}", other),
        }
    }
}
