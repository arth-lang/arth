#[cfg(test)]
mod tests {
    use crate::compiler::codegen::llvm_text::emit_module_text;
    use crate::compiler::ir::{
        BlockData, EnumDef, EnumVariantDef, Func, Inst, InstKind, Linkage, Module, StructDef,
        StructFieldDef, Terminator, Ty, Value, demo_add_module,
    };

    #[test]
    fn llvm_text_demo_add_stable() {
        let m = demo_add_module();
        let txt = emit_module_text(&m);
        let expected = "; ModuleID = 'demo'\n\
define i64 @add(i64 %0, i64 %1) {\n\
entry:\n\
  %2 = add i64 %0, %1\n\
  ret i64 %2\n\
}\n\n";
        assert_eq!(txt, expected);
    }

    #[test]
    fn llvm_text_emits_struct_type_definition() {
        let mut m = Module::new("types_struct");
        m.structs.push(StructDef {
            name: "Point".to_string(),
            fields: vec![
                StructFieldDef {
                    name: "x".to_string(),
                    ty: Ty::I64,
                },
                StructFieldDef {
                    name: "y".to_string(),
                    ty: Ty::I64,
                },
            ],
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("%Point = type { i64, i64 }"));
    }

    #[test]
    fn llvm_text_emits_enum_type_definition() {
        let mut m = Module::new("types_enum");
        m.enums.push(EnumDef {
            name: "MaybeInt".to_string(),
            variants: vec![
                EnumVariantDef {
                    name: "None".to_string(),
                    payload_types: vec![],
                },
                EnumVariantDef {
                    name: "Some".to_string(),
                    payload_types: vec![Ty::I64],
                },
            ],
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("%MaybeInt = type"));
        assert!(txt.contains("%MaybeInt = type { i32,"));
    }

    #[test]
    fn llvm_text_emits_closure_runtime_decls_when_used() {
        let mut m = Module::new("closures");
        m.funcs.push(Func {
            name: "closure_target".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        m.funcs.push(Func {
            name: "main".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        kind: InstKind::ConstI64(7),
                        result: Value(0),
                        span: None,
                    },
                    Inst {
                        kind: InstKind::MakeClosure {
                            func: "closure_target".to_string(),
                            captures: vec![Value(0)],
                        },
                        result: Value(1),
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(1))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("%Closure = type { ptr, ptr, i64 }"));
        assert!(txt.contains("declare i64 @arth_rt_closure_new(ptr, i64)"));
        assert!(txt.contains("declare i64 @arth_rt_closure_call_variadic(i64, ptr, i64)"));
    }

    #[test]
    fn llvm_text_omits_closure_runtime_decls_when_unused() {
        let txt = emit_module_text(&demo_add_module());
        assert!(!txt.contains("%Closure = type { ptr, ptr, i64 }"));
        assert!(!txt.contains("declare i64 @arth_rt_closure_new(ptr, i64)"));
    }

    #[test]
    fn llvm_text_emits_provider_runtime_decls_when_used() {
        let mut m = Module::new("providers");
        m.funcs.push(Func {
            name: "main".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        kind: InstKind::ConstI64(42),
                        result: Value(0),
                        span: None,
                    },
                    Inst {
                        kind: InstKind::ProviderNew {
                            name: "App.Config".to_string(),
                            values: vec![("value".to_string(), Value(0))],
                        },
                        result: Value(1),
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(1))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("declare i64 @arth_rt_provider_new(ptr, i64, i64)"));
        assert!(txt.contains("@.str.provider.App_Config"));
    }

    #[test]
    fn llvm_text_uses_variadic_runtime_dispatch_for_closure_call_with_more_than_8_args() {
        let mut m = Module::new("closures_variadic");
        m.funcs.push(Func {
            name: "closure_target".to_string(),
            params: vec![
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
                Ty::I64,
            ],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        let mut main_insts = Vec::new();
        for i in 0..9_u32 {
            main_insts.push(Inst {
                kind: InstKind::ConstI64((i + 1) as i64),
                result: Value(i),
                span: None,
            });
        }
        main_insts.push(Inst {
            kind: InstKind::MakeClosure {
                func: "closure_target".to_string(),
                captures: vec![],
            },
            result: Value(9),
            span: None,
        });
        main_insts.push(Inst {
            kind: InstKind::ClosureCall {
                closure: Value(9),
                args: (0..9_u32).map(Value).collect(),
                ret: Ty::I64,
            },
            result: Value(10),
            span: None,
        });

        m.funcs.push(Func {
            name: "main".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: main_insts,
                term: Terminator::Ret(Some(Value(10))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("call i64 @arth_rt_closure_call_variadic"));
        assert!(txt.contains("alloca [9 x i64], align 8"));
        assert!(!txt.contains("call i64 @arth_rt_closure_call_8(i64 %v9"));
    }

    #[test]
    fn llvm_text_emits_typed_closure_environment_layout_from_capture_metadata() {
        let mut m = Module::new("closures_layout");
        m.structs.push(StructDef {
            name: "Point".to_string(),
            fields: vec![
                StructFieldDef {
                    name: "x".to_string(),
                    ty: Ty::I64,
                },
                StructFieldDef {
                    name: "y".to_string(),
                    ty: Ty::I64,
                },
            ],
        });
        m.funcs.push(Func {
            name: "__lambda_struct".to_string(),
            params: vec![Ty::Struct("Point".to_string()), Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![],
                term: Terminator::Ret(Some(Value(1))),
                span: None,
            }],
            linkage: Linkage::Private,
            span: None,
        });
        m.funcs.push(Func {
            name: "main".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        kind: InstKind::ConstI64(0),
                        result: Value(0),
                        span: None,
                    },
                    Inst {
                        kind: InstKind::MakeClosure {
                            func: "__lambda_struct".to_string(),
                            captures: vec![Value(0)],
                        },
                        result: Value(1),
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(1))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        });

        let txt = emit_module_text(&m);
        assert!(txt.contains("%ClosureEnv___lambda_struct = type { %Point }"));
        assert!(txt.contains("; capture alignments: [8]"));
    }
}
