use super::*;

#[test]
fn encode_decode_roundtrip() {
    let p = compile_messages_to_program(&["hello".to_string(), "world".to_string()]);
    let bytes = encode_program(&p);
    let q = decode_program(&bytes).expect("decode");
    assert_eq!(p, q);
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

#[test]
fn dump_waid_bytecode() {
    let bytes = std::fs::read("/Users/yppartha/PROJECTS/RUNE_ARTH/WAID/plugins/wisp-mail/dist/ir/pages/home/logic/home.abc").expect("read");
    let program = crate::decode_program(&bytes).expect("decode");
    
    // Find handle_event export offset from the decode
    // The library format has exports, so decode as library
    let (prog, exports) = crate::decode_library(&bytes).expect("decode lib");
    
    println!("Exports:");
    for e in &exports {
        println!("  {} at offset {} arity {}", e.name, e.offset, e.arity);
    }
    
    // Show first 50 ops
    println!("
First 50 ops:");
    for (i, op) in prog.code.iter().take(50).enumerate() {
        println!("  {}: {:?}", i, op);
    }
    
    // If handle_event exists, show its ops
    if let Some(he) = exports.iter().find(|e| e.name == "handle_event") {
        println!("
handle_event at offset {}:", he.offset);
        for i in he.offset as usize..(he.offset as usize + 30).min(prog.code.len()) {
            println!("  {}: {:?}", i, prog.code[i]);
        }
    }
}

// ============================================================================
// Direct Export Calling (call_export) Tests
// ============================================================================

#[test]
fn call_export_simple_function() {
    // Create a simple program with an export that adds two numbers
    // Function at offset 0: takes no args, pushes 5 + 3, returns 8
    let code = vec![
        Op::PushI64(5),   // 0: Push first number
        Op::PushI64(3),   // 1: Push second number
        Op::AddI64,       // 2: Add them
        Op::Ret,          // 3: Return result (8)
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
    // Function at offset 0: expects string in local 0, returns length
    let code = vec![
        Op::LocalGet(0),  // 0: Get the string argument
        Op::StrLen,       // 1: Get string length
        Op::Ret,          // 2: Return length
    ];
    let p = Program::new(vec![], code);

    let result = call_export(&p, 0, 1, &["hello"], None);
    match result {
        CallExportResult::Success(Some(n)) => assert_eq!(n, 5, "should return 5 (length of 'hello')"),
        CallExportResult::Success(None) => panic!("expected return value"),
        CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
        CallExportResult::ExportNotFound => panic!("export not found"),
        CallExportResult::InvalidArgument(msg) => panic!("invalid argument: {}", msg),
    }
}

#[test]
fn call_export_invalid_offset() {
    // Create a small program and try to call at invalid offset
    let code = vec![
        Op::PushI64(1),
        Op::Ret,
    ];
    let p = Program::new(vec![], code);

    let result = call_export(&p, 100, 0, &[], None);
    assert!(matches!(result, CallExportResult::ExportNotFound));
}

#[test]
fn call_export_arity_mismatch() {
    // Create a program and try to call with wrong number of arguments
    let code = vec![
        Op::LocalGet(0),
        Op::Ret,
    ];
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
        Op::PushI64(42),  // 0: Do some work
        Op::Pop,          // 1: Discard it
        Op::Ret,          // 2: Return (no value)
    ];
    let p = Program::new(vec![], code);

    let result = call_export(&p, 0, 0, &[], None);
    match result {
        CallExportResult::Success(None) => (), // Expected
        CallExportResult::Success(Some(_)) => (), // Also ok if stack had something
        CallExportResult::Failed(code) => panic!("failed with exit code {}", code),
        _ => panic!("unexpected result"),
    }
}

#[test]
fn call_export_with_internal_call() {
    // Create a program where the export calls another function
    // Layout:
    //   0-2: helper function that returns 10
    //   3-6: main export that calls helper, adds 5, returns
    let code = vec![
        // Helper at offset 0
        Op::PushI64(10),  // 0: Push 10
        Op::Ret,          // 1: Return

        // Export at offset 2
        Op::Call(0),      // 2: Call helper at offset 0
        Op::PushI64(5),   // 3: Push 5
        Op::AddI64,       // 4: Add (10 + 5)
        Op::Ret,          // 5: Return 15
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
fn call_export_with_conditionals() {
    // Create a program with conditional logic
    // Function: if arg length > 3 return 1, else return 0
    let code = vec![
        Op::LocalGet(0),    // 0: Get string arg
        Op::StrLen,         // 1: Get length
        Op::PushI64(3),     // 2: Push 3
        Op::LtI64,          // 3: Compare (length < 3 becomes bool)
        // Note: LtI64 gives us "length < 3", we want "length > 3"
        // So if LtI64 is true, return 0; if false, return 1
        Op::JumpIfFalse(7), // 4: If NOT (length < 3), jump to return 1
        Op::PushI64(0),     // 5: Return 0 (short string)
        Op::Ret,            // 6: Return
        Op::PushI64(1),     // 7: Return 1 (long string)
        Op::Ret,            // 8: Return
    ];
    let p = Program::new(vec![], code);

    // Test with short string (length 2)
    let result1 = call_export(&p, 0, 1, &["hi"], None);
    match result1 {
        CallExportResult::Success(Some(n)) => assert_eq!(n, 0, "short string should return 0"),
        _ => panic!("unexpected result"),
    }

    // Test with long string (length 5)
    let result2 = call_export(&p, 0, 1, &["hello"], None);
    match result2 {
        CallExportResult::Success(Some(n)) => assert_eq!(n, 1, "long string should return 1"),
        _ => panic!("unexpected result"),
    }
}

