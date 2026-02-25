#[cfg(test)]
mod tests {
    use crate::compiler::codegen::cranelift::compile_module_with_message;
    use crate::compiler::ir::demo_add_module;

    #[cfg(feature = "cranelift")]
    #[test]
    fn cranelift_jit_runs_ok() {
        let m = demo_add_module();
        let r = compile_module_with_message(&m, Some("IR JIT test"));
        assert!(r.is_ok());
    }

    #[cfg(not(feature = "cranelift"))]
    #[test]
    fn cranelift_feature_required() {
        let m = demo_add_module();
        let r = compile_module_with_message(&m, Some("IR JIT test"));
        assert!(r.is_err());
    }
}

