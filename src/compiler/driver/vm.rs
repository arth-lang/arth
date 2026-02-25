use std::collections::HashMap;

use arth_vm as vm;

// No direct AST lowering here; IR→VM is used.

/// FNV-1a hash for async body function name lookup (matches hir_to_ir.rs)
fn compute_string_hash(s: &str) -> i64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

pub fn compile_ir_to_program(
    funcs: &[crate::compiler::ir::Func],
    strings: &[String],
    _providers: &[crate::compiler::ir::Provider],
) -> vm::Program {
    if funcs.is_empty() {
        return vm::Program::new(vec![], vec![vm::Op::Halt]);
    }

    let mut out_strings: Vec<String> = Vec::new();
    let mut code: Vec<vm::Op> = Vec::new();
    let mut func_offsets: HashMap<String, u32> = HashMap::new();
    let mut debug_entries: Vec<vm::DebugEntry> = Vec::new();
    let mut provider_meta: HashMap<String, &crate::compiler::ir::Provider> = HashMap::new();
    for p in _providers {
        provider_meta.insert(p.name.clone(), p);
    }
    // Map from fn_id hash to body function name for async lookup
    let mut async_body_hashes: HashMap<i64, String> = HashMap::new();
    let mut call_patches: Vec<(usize, String)> = Vec::new();
    let mut closure_patches: Vec<(usize, String)> = Vec::new();
    let func_names: std::collections::HashSet<String> =
        funcs.iter().map(|f| f.name.clone()).collect();

    // Collect async body function hashes for lookup during codegen
    // The hash is computed from the unqualified name (e.g., "computeValue$async_body")
    // but the actual function name may be qualified (e.g., "Main.computeValue$async_body")
    for f in funcs {
        if f.name.ends_with("$async_body") {
            // Strip module prefix if present (e.g., "Main.computeValue$async_body" -> "computeValue$async_body")
            let unqualified_name = if let Some(dot_pos) = f.name.find('.') {
                &f.name[dot_pos + 1..]
            } else {
                &f.name
            };
            let hash = compute_string_hash(unqualified_name);
            async_body_hashes.insert(hash, f.name.clone());
        }
    }

    // Infer function arities from call sites within this module set.
    // Check both Call instructions and Invoke terminators.
    let mut func_argc: HashMap<String, usize> = HashMap::new();
    for f in funcs {
        for b in &f.blocks {
            // Check instructions for Call
            for inst in &b.insts {
                if let crate::compiler::ir::InstKind::Call { name, args, .. } = &inst.kind
                    && func_names.contains(name)
                {
                    let n = args.len();
                    let e = func_argc.entry(name.clone()).or_insert(0);
                    if n > *e {
                        *e = n;
                    }
                }
            }
            // Check terminator for Invoke (calls that may throw inside try blocks)
            if let crate::compiler::ir::Terminator::Invoke { callee, args, .. } = &b.term {
                if func_names.contains(callee) {
                    let n = args.len();
                    let e = func_argc.entry(callee.clone()).or_insert(0);
                    if n > *e {
                        *e = n;
                    }
                }
            }
        }
    }

    // Helper to intern output strings
    fn str_index(pool: &mut Vec<String>, s: &str) -> u32 {
        if let Some((i, _)) = pool.iter().enumerate().find(|(_, x)| x == &s) {
            i as u32
        } else {
            pool.push(s.to_string());
            (pool.len() - 1) as u32
        }
    }

    for f in funcs {
        // Record this function's starting offset
        let offset = code.len() as u32;
        func_offsets.insert(f.name.clone(), offset);
        // Add debug entry for stack trace symbolication
        let debug_entry = if let Some(ref span) = f.span {
            // Extract file path from span
            let file_path = span.file.display().to_string();
            // Note: line number would require source text to convert byte offset.
            // For now, we store 0 and could enhance this later with a line map.
            vm::DebugEntry::with_location(offset, f.name.clone(), file_path, 0)
        } else {
            vm::DebugEntry::new(offset, f.name.clone())
        };
        debug_entries.push(debug_entry);

        // Per-function state
        let mut const_int: HashMap<u32, i64> = HashMap::new();
        let mut const_str: HashMap<u32, String> = HashMap::new();
        let mut const_f64: HashMap<u32, f64> = HashMap::new();
        let mut val_local: HashMap<u32, u32> = HashMap::new();
        let mut alloca_slot: HashMap<u32, u32> = HashMap::new();
        let mut next_local: u32 = 0;
        let mut block_offset: Vec<Option<u32>> = vec![None; f.blocks.len()];
        let mut patch_jumps: Vec<(usize, u32)> = Vec::new();

        // Helper fns that don't capture borrows
        fn ensure_local_for(
            val_local: &mut HashMap<u32, u32>,
            next_local: &mut u32,
            v_id: u32,
        ) -> u32 {
            if let Some(ix) = val_local.get(&v_id).copied() {
                ix
            } else {
                let ix = *next_local;
                *next_local += 1;
                val_local.insert(v_id, ix);
                ix
            }
        }
        fn push_value(
            code: &mut Vec<vm::Op>,
            val_local: &HashMap<u32, u32>,
            const_int: &HashMap<u32, i64>,
            const_f64: &HashMap<u32, f64>,
            const_str: &HashMap<u32, String>,
            out_strings: &mut Vec<String>,
            vid: u32,
        ) {
            if let Some(ix) = val_local.get(&vid).copied() {
                code.push(vm::Op::LocalGet(ix));
            } else if let Some(c) = const_int.get(&vid).copied() {
                code.push(vm::Op::PushI64(c));
            } else if let Some(f) = const_f64.get(&vid).copied() {
                code.push(vm::Op::PushF64(f));
            } else if let Some(s) = const_str.get(&vid) {
                let ix = {
                    if let Some((i, _)) = out_strings.iter().enumerate().find(|(_, x)| *x == s) {
                        i as u32
                    } else {
                        out_strings.push(s.clone());
                        (out_strings.len() - 1) as u32
                    }
                };
                code.push(vm::Op::PushStr(ix));
            } else {
                code.push(vm::Op::PushI64(0));
            }
        }

        // Function entry prologue: pop N arguments from the value stack into
        // locals 0..N-1. Callers push arguments left-to-right, so pop in reverse
        // to assign local0=arg0, local1=arg1, ...
        // Use the IR function's declared param count as argc (more reliable than call-site inference)
        let argc = f.params.len();
        if argc > 0 {
            for i in (0..argc).rev() {
                code.push(vm::Op::LocalSet(i as u32));
            }
            // Seed IR param Values (0..argc-1) to corresponding locals so push_value can load them
            for i in 0..argc {
                val_local.insert(i as u32, i as u32);
            }
            // IMPORTANT: Advance next_local past the parameter slots so Alloca doesn't reuse them
            next_local = argc as u32;
        }

        // Collect phi nodes per block for phi-value assignment before branches.
        // phi_info[target_block_idx] = Vec of (phi_result_id, Vec<(pred_block_idx, incoming_value_id)>)
        let mut phi_info: Vec<Vec<(u32, Vec<(u32, u32)>)>> = vec![Vec::new(); f.blocks.len()];
        for (bi, b) in f.blocks.iter().enumerate() {
            for inst in &b.insts {
                if let crate::compiler::ir::InstKind::Phi(ops) = &inst.kind {
                    let phi_dst = inst.result.0;
                    let incoming: Vec<(u32, u32)> = ops.iter().map(|(bb, v)| (bb.0, v.0)).collect();
                    phi_info[bi].push((phi_dst, incoming));
                }
            }
        }

        // Pre-allocate locals for all phi destinations so they exist before branches
        for phis in &phi_info {
            for (phi_dst, _) in phis {
                ensure_local_for(&mut val_local, &mut next_local, *phi_dst);
            }
        }

        // Helper closure to emit phi assignments before jumping from `from_block` to `to_block`.
        // This must be called as a function that captures the necessary context.
        fn emit_phi_assignments(
            code: &mut Vec<vm::Op>,
            val_local: &HashMap<u32, u32>,
            const_int: &HashMap<u32, i64>,
            const_f64: &HashMap<u32, f64>,
            const_str: &HashMap<u32, String>,
            out_strings: &mut Vec<String>,
            phi_info: &[Vec<(u32, Vec<(u32, u32)>)>],
            from_block: u32,
            to_block: u32,
        ) {
            if let Some(phis) = phi_info.get(to_block as usize) {
                for (phi_dst, incoming) in phis {
                    // Find the incoming value for `from_block`
                    if let Some((_, val_id)) = incoming.iter().find(|(bb, _)| *bb == from_block) {
                        // Push the incoming value
                        push_value(
                            code,
                            val_local,
                            const_int,
                            const_f64,
                            const_str,
                            out_strings,
                            *val_id,
                        );
                        // Store to phi destination
                        if let Some(&dst_local) = val_local.get(phi_dst) {
                            code.push(vm::Op::LocalSet(dst_local));
                        }
                    }
                }
            }
        }

        for (bi, b) in f.blocks.iter().enumerate() {
            // Mark block start offset
            block_offset[bi] = Some(code.len() as u32);

            // Emit instructions
            for inst in &b.insts {
                use crate::compiler::ir::InstKind as I;
                match &inst.kind {
                    I::ConstI64(n) => {
                        const_int.insert(inst.result.0, *n);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(*n));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::ConstStr(ix) => {
                        let s = strings.get(*ix as usize).cloned().unwrap_or_default();
                        const_str.insert(inst.result.0, s);
                        // No runtime action for string pointers on the demo VM
                    }
                    I::ConstF64(x) => {
                        const_f64.insert(inst.result.0, *x);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushF64(*x));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::Copy(v) => {
                        let src = v.0;
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            src,
                        );
                        code.push(vm::Op::LocalSet(dst));
                        // propagate const if known
                        if let Some(n) = const_int.get(&src).copied() {
                            const_int.insert(inst.result.0, n);
                        }
                        if let Some(s) = const_str.get(&src).cloned() {
                            const_str.insert(inst.result.0, s);
                        }
                        if let Some(f) = const_f64.get(&src).copied() {
                            const_f64.insert(inst.result.0, f);
                        }
                    }
                    I::Binary(op, a, b) => {
                        use crate::compiler::ir::BinOp as B;
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        let a_id = a.0;
                        let b_id = b.0;
                        match op {
                            B::Add | B::Sub | B::Mul | B::Div | B::Mod | B::Shl | B::Shr => {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                match op {
                                    B::Add => code.push(vm::Op::AddI64),
                                    B::Sub => code.push(vm::Op::SubI64),
                                    B::Mul => code.push(vm::Op::MulI64),
                                    B::Div => code.push(vm::Op::DivI64),
                                    B::Mod => code.push(vm::Op::ModI64),
                                    B::Shl => code.push(vm::Op::ShlI64),
                                    B::Shr => code.push(vm::Op::ShrI64),
                                    _ => {}
                                }
                                code.push(vm::Op::LocalSet(dst));
                            }
                            B::And | B::Or | B::Xor => {
                                // Implement bitwise ops on the VM
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                match op {
                                    B::And => code.push(vm::Op::AndI64),
                                    B::Or => code.push(vm::Op::OrI64),
                                    B::Xor => code.push(vm::Op::XorI64),
                                    _ => {}
                                }
                                code.push(vm::Op::LocalSet(dst));
                            }
                        }
                    }
                    I::Cmp(pred, a, b) => {
                        use crate::compiler::ir::CmpPred as C;
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        let a_id = a.0;
                        let b_id = b.0;
                        match pred {
                            C::Eq => {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                code.push(vm::Op::EqI64);
                            }
                            C::Lt => {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                code.push(vm::Op::LtI64);
                            }
                            C::Gt => {
                                // b < a
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                code.push(vm::Op::LtI64);
                            }
                            C::Le => {
                                // !(b < a)
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                code.push(vm::Op::LtI64);
                                code.push(vm::Op::PushBool(0));
                                code.push(vm::Op::EqI64);
                            }
                            C::Ge => {
                                // !(a < b)
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                code.push(vm::Op::LtI64);
                                code.push(vm::Op::PushBool(0));
                                code.push(vm::Op::EqI64);
                            }
                            C::Ne => {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a_id,
                                );
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    b_id,
                                );
                                code.push(vm::Op::EqI64);
                                code.push(vm::Op::PushBool(0));
                                code.push(vm::Op::EqI64);
                            }
                        }
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::StrEq(a, b) => {
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        let a_id = a.0;
                        let b_id = b.0;
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            a_id,
                        );
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            b_id,
                        );
                        code.push(vm::Op::EqStr);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::StrConcat(a, b) => {
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        let a_id = a.0;
                        let b_id = b.0;
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            a_id,
                        );
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            b_id,
                        );
                        code.push(vm::Op::ConcatStr);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::Alloca => {
                        // Allocate a fresh local slot representing this stack cell
                        let slot = next_local;
                        next_local += 1;
                        alloca_slot.insert(inst.result.0, slot);
                    }
                    I::Load(p) => {
                        let Some(&slot) = alloca_slot.get(&p.0) else {
                            continue;
                        };
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalGet(slot));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::Store(p, v) => {
                        if let Some(&slot) = alloca_slot.get(&p.0) {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                v.0,
                            );
                            code.push(vm::Op::LocalSet(slot));
                        }
                    }
                    I::ProviderNew { name, values } => {
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        let Some(prov) = provider_meta.get(name) else {
                            eprintln!("error: internal: provider '{}' not found in metadata", name);
                            continue;
                        };
                        let field_count = prov.fields.len();

                        let name_ix = str_index(&mut out_strings, name);
                        code.push(vm::Op::PushStr(name_ix));
                        code.push(vm::Op::PushI64(field_count as i64));
                        code.push(vm::Op::StructNew);

                        let temp_struct = next_local;
                        next_local += 1;
                        code.push(vm::Op::LocalSet(temp_struct));

                        for (i, field) in prov.fields.iter().enumerate() {
                            let init_val_id = values
                                .iter()
                                .find(|(n, _)| n == &field.name)
                                .map(|(_, v)| v.0)
                                .unwrap_or(0);

                            code.push(vm::Op::LocalGet(temp_struct));
                            code.push(vm::Op::PushI64(i as i64));

                            if field.is_shared {
                                code.push(vm::Op::SharedNew);
                            } else {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    init_val_id,
                                );
                            }

                            let field_name_ix = str_index(&mut out_strings, &field.name);
                            code.push(vm::Op::PushStr(field_name_ix));
                            code.push(vm::Op::StructSet);
                            code.push(vm::Op::Pop);

                            if field.is_shared && init_val_id != 0 {
                                // Store initial value into shared cell
                                code.push(vm::Op::LocalGet(temp_struct));
                                code.push(vm::Op::PushI64(i as i64));
                                code.push(vm::Op::StructGet);
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    init_val_id,
                                );
                                code.push(vm::Op::SharedStore);
                                code.push(vm::Op::Pop);
                            }
                        }

                        code.push(vm::Op::LocalGet(temp_struct));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::ProviderFieldGet {
                        obj,
                        provider,
                        field,
                    } => {
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        if obj.0 == 0 {
                            let full_name = if provider.is_empty() {
                                field.clone()
                            } else {
                                format!("{}.{}", provider, field)
                            };
                            let name_ix = str_index(&mut out_strings, &full_name);
                            code.push(vm::Op::SharedGetByName(name_ix));
                            code.push(vm::Op::SharedLoad);
                        } else {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                obj.0,
                            );
                            let field_name_ix = str_index(&mut out_strings, field);
                            code.push(vm::Op::PushStr(field_name_ix));
                            code.push(vm::Op::StructGetNamed);

                            let is_shared = provider_meta
                                .get(provider)
                                .map(|p| p.fields.iter().any(|f| &f.name == field && f.is_shared))
                                .unwrap_or(false);
                            if is_shared {
                                code.push(vm::Op::SharedLoad);
                            }
                        }
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::ProviderFieldSet {
                        obj,
                        provider,
                        field,
                        value,
                    } => {
                        if obj.0 == 0 {
                            let full_name = if provider.is_empty() {
                                field.clone()
                            } else {
                                format!("{}.{}", provider, field)
                            };
                            let name_ix = str_index(&mut out_strings, &full_name);
                            code.push(vm::Op::SharedGetByName(name_ix));
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                value.0,
                            );
                            code.push(vm::Op::SharedStore);
                            code.push(vm::Op::Pop);
                        } else {
                            // Load existing handle if shared, or just store if non-shared
                            let is_shared = provider_meta
                                .get(provider)
                                .map(|p| p.fields.iter().any(|f| &f.name == field && f.is_shared))
                                .unwrap_or(false);

                            if is_shared {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    obj.0,
                                );
                                let field_name_ix = str_index(&mut out_strings, field);
                                code.push(vm::Op::PushStr(field_name_ix));
                                code.push(vm::Op::StructGetNamed);
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    value.0,
                                );
                                code.push(vm::Op::SharedStore);
                                code.push(vm::Op::Pop);
                            } else {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    obj.0,
                                );
                                // Find field index
                                let field_idx = provider_meta
                                    .get(provider)
                                    .and_then(|p| p.fields.iter().position(|f| &f.name == field))
                                    .unwrap_or(0);
                                code.push(vm::Op::PushI64(field_idx as i64));
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    value.0,
                                );
                                let field_name_ix = str_index(&mut out_strings, field);
                                code.push(vm::Op::PushStr(field_name_ix));
                                code.push(vm::Op::StructSet);
                                code.push(vm::Op::Pop);
                            }
                        }
                    }
                    I::Call { name, args, .. } => {
                        // Destination local for this call's result
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        if name == "__arth_cast_f64" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_i64" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToI64OrEnumTag);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_bool" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToBool);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_char" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToChar);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_i8" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToI8);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_i16" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToI16);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_i32" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToI32);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_u8" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToU8);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_u16" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToU16);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_u32" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToU32);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_u64" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToU64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_cast_f32" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ToF32);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_shared_new" && args.is_empty() {
                            code.push(vm::Op::SharedNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_shared_store" && args.len() == 2 {
                            // push handle, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::SharedStore);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_shared_load" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::SharedLoad);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_shared_get_named" && args.len() == 1 {
                            // Arg must be a const string; look it up and emit name-indexed op.
                            if let Some(s) = const_str.get(&args[0].0) {
                                let ix = str_index(&mut out_strings, s);
                                code.push(vm::Op::SharedGetByName(ix));
                                code.push(vm::Op::LocalSet(dst));
                            } else {
                                // Fallback: push 0
                                code.push(vm::Op::PushI64(0));
                                code.push(vm::Op::LocalSet(dst));
                            }

                        // --- Concurrent Executor intrinsics ---
                        } else if name == "__arth_executor_init" && args.len() == 1 {
                            // Initialize the global thread pool with N threads
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ExecutorInit);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_thread_count" && args.is_empty() {
                            // Get number of threads in the pool
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::ExecutorThreadCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_active_workers" && args.is_empty() {
                            // Get number of active workers
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::ExecutorActiveWorkers);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_spawn" && args.len() == 1 {
                            // Spawn a task on the thread pool (fn_id -> task_id)
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ExecutorSpawn);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_join" && args.len() == 1 {
                            // Wait for a task and get its result
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ExecutorJoin);
                            code.push(vm::Op::LocalSet(dst));

                        // C02 work-stealing stats intrinsics
                        } else if name == "__arth_executor_spawn_with_arg" && args.len() == 2 {
                            // Spawn task with fn_id (work type) and arg
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push fn_id first, then arg (stack order: fn_id, arg)
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ExecutorSpawnWithArg);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_active_executor_count" && args.is_empty()
                        {
                            // Get count of workers that executed at least one task
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::ExecutorActiveExecutorCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_worker_task_count" && args.len() == 1 {
                            // Get task count for specific worker
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ExecutorWorkerTaskCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_reset_stats" && args.is_empty() {
                            // Reset all worker task counters
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::ExecutorResetStats);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_executor_spawn_await" && args.len() == 3 {
                            // C04: Spawn sub-task and suspend until complete
                            // Args: sub_fn_id, sub_arg, local_accumulator
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::ExecutorSpawnAwait);
                            code.push(vm::Op::LocalSet(dst));
                        // ====================================================
                        // MPMC Channel operations (C06)
                        // ====================================================
                        } else if name == "__arth_mpmc_chan_create" && args.len() == 1 {
                            // Create MPMC channel with capacity
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_send" && args.len() == 2 {
                            // Non-blocking send: handle, value -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MpmcChanSend);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_send_blocking" && args.len() == 2 {
                            // Blocking send: handle, value -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MpmcChanSendBlocking);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_recv" && args.len() == 1 {
                            // Non-blocking receive: handle -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanRecv);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_recv_blocking" && args.len() == 1 {
                            // Blocking receive: handle -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanRecvBlocking);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_close" && args.len() == 1 {
                            // Close channel: handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_len" && args.len() == 1 {
                            // Get channel length: handle -> length
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanLen);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_is_empty" && args.len() == 1 {
                            // Check if empty: handle -> is_empty
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanIsEmpty);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_is_full" && args.len() == 1 {
                            // Check if full: handle -> is_full
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanIsFull);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_is_closed" && args.len() == 1 {
                            // Check if closed: handle -> is_closed
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanIsClosed);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_capacity" && args.len() == 1 {
                            // Get capacity: handle -> capacity
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanCapacity);
                            code.push(vm::Op::LocalSet(dst));

                        // ====================================================
                        // C07: Executor-integrated MPMC Channel operations
                        // ====================================================
                        } else if name == "__arth_mpmc_chan_send_with_task" && args.len() == 3 {
                            // Send with task suspension support: handle, value, task_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::MpmcChanSendWithTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_recv_with_task" && args.len() == 2 {
                            // Recv with task suspension support: handle, task_id -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MpmcChanRecvWithTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_recv_and_wake" && args.len() == 1 {
                            // Recv and wake waiting sender: handle -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanRecvAndWake);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_pop_waiting_sender" && args.len() == 1 {
                            // Pop waiting sender: handle -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanPopWaitingSender);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_get_waiting_sender_value"
                            && args.is_empty()
                        {
                            // Get waiting sender's value: -> value
                            code.push(vm::Op::MpmcChanGetWaitingSenderValue);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_pop_waiting_receiver" && args.len() == 1
                        {
                            // Pop waiting receiver: handle -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanPopWaitingReceiver);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_waiting_sender_count" && args.len() == 1
                        {
                            // Count waiting senders: handle -> count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanWaitingSenderCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_waiting_receiver_count"
                            && args.len() == 1
                        {
                            // Count waiting receivers: handle -> count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanWaitingReceiverCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_get_woken_sender" && args.is_empty() {
                            // Get woken sender task ID: -> task_id
                            code.push(vm::Op::MpmcChanGetWokenSender);
                            code.push(vm::Op::LocalSet(dst));

                        // ====================================================
                        // C08: Blocking Receive operations
                        // ====================================================
                        } else if name == "__arth_mpmc_chan_send_and_wake" && args.len() == 2 {
                            // Send and wake waiting receiver: handle, value -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MpmcChanSendAndWake);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_get_woken_receiver" && args.is_empty() {
                            // Get woken receiver task ID: -> task_id
                            code.push(vm::Op::MpmcChanGetWokenReceiver);
                            code.push(vm::Op::LocalSet(dst));

                        // =====================================================
                        // C09: Channel Select Operations
                        // =====================================================
                        } else if name == "__arth_mpmc_chan_select_clear" && args.is_empty() {
                            // Clear select set: -> (no result, but we still set dst to 0)
                            code.push(vm::Op::MpmcChanSelectClear);
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_add" && args.len() == 1 {
                            // Add channel to select set: handle -> index
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanSelectAdd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_count" && args.is_empty() {
                            // Get select set count: -> count
                            code.push(vm::Op::MpmcChanSelectCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_try_select_recv" && args.is_empty() {
                            // Try select recv: -> status
                            code.push(vm::Op::MpmcChanTrySelectRecv);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_recv_blocking" && args.is_empty()
                        {
                            // Blocking select recv: -> status
                            code.push(vm::Op::MpmcChanSelectRecvBlocking);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_recv_with_task"
                            && args.len() == 1
                        {
                            // Select recv with task: task_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanSelectRecvWithTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_get_ready_index"
                            && args.is_empty()
                        {
                            // Get ready index: -> index
                            code.push(vm::Op::MpmcChanSelectGetReadyIndex);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_get_value" && args.is_empty() {
                            // Get select value: -> value
                            code.push(vm::Op::MpmcChanSelectGetValue);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_deregister" && args.len() == 2 {
                            // Deregister from select: task_id, except_index -> (no result)
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MpmcChanSelectDeregister);
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_mpmc_chan_select_get_handle" && args.len() == 1 {
                            // Get handle by index: index -> handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MpmcChanSelectGetHandle);
                            code.push(vm::Op::LocalSet(dst));

                        // ===================================================================
                        // C11: Actor operations (Actor = Task + Channel)
                        // ===================================================================
                        } else if name == "__arth_actor_create" && args.len() == 1 {
                            // Create actor: capacity -> actor_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_spawn" && args.len() == 2 {
                            // Spawn actor: capacity, task_handle -> actor_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ActorSpawn);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_send" && args.len() == 2 {
                            // Send: actor_handle, message -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ActorSend);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_send_blocking" && args.len() == 2 {
                            // Send blocking: actor_handle, message -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ActorSendBlocking);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_recv" && args.len() == 1 {
                            // Recv: actor_handle -> message
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorRecv);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_recv_blocking" && args.len() == 1 {
                            // Recv blocking: actor_handle -> message
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorRecvBlocking);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_close" && args.len() == 1 {
                            // Close: actor_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_stop" && args.len() == 1 {
                            // Stop: actor_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorStop);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_get_task" && args.len() == 1 {
                            // Get task: actor_handle -> task_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorGetTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_get_mailbox" && args.len() == 1 {
                            // Get mailbox: actor_handle -> mailbox_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorGetMailbox);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_is_running" && args.len() == 1 {
                            // Is running: actor_handle -> is_running
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorIsRunning);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_get_state" && args.len() == 1 {
                            // Get state: actor_handle -> state
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorGetState);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_message_count" && args.len() == 1 {
                            // Message count: actor_handle -> count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorMessageCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_mailbox_empty" && args.len() == 1 {
                            // Mailbox empty: actor_handle -> is_empty
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorMailboxEmpty);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_mailbox_len" && args.len() == 1 {
                            // Mailbox length: actor_handle -> length
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorMailboxLen);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_set_task" && args.len() == 2 {
                            // Set task: actor_handle, task_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ActorSetTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_mark_stopped" && args.len() == 1 {
                            // Mark stopped: actor_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorMarkStopped);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_mark_failed" && args.len() == 1 {
                            // Mark failed: actor_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorMarkFailed);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_actor_is_failed" && args.len() == 1 {
                            // Is failed: actor_handle -> result
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ActorIsFailed);
                            code.push(vm::Op::LocalSet(dst));

                        // ===================================================================
                        // C19: Atomic<T> Operations
                        // ===================================================================
                        } else if name == "__arth_atomic_create" && args.len() == 1 {
                            // Create atomic: initial_value -> atomic_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::AtomicCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_load" && args.len() == 1 {
                            // Load: atomic_handle -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::AtomicLoad);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_store" && args.len() == 2 {
                            // Store: atomic_handle, value -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::AtomicStore);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_cas" && args.len() == 3 {
                            // CAS: atomic_handle, expected, new_value -> success
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::AtomicCas);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_fetch_add" && args.len() == 2 {
                            // FetchAdd: atomic_handle, delta -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::AtomicFetchAdd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_fetch_sub" && args.len() == 2 {
                            // FetchSub: atomic_handle, delta -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::AtomicFetchSub);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_swap" && args.len() == 2 {
                            // Swap: atomic_handle, new_value -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::AtomicSwap);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_get" && args.len() == 1 {
                            // Get (alias for load): atomic_handle -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::AtomicGet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_set" && args.len() == 2 {
                            // Set (alias for store): atomic_handle, value -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::AtomicSet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_inc" && args.len() == 1 {
                            // Inc: atomic_handle -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::AtomicInc);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_atomic_dec" && args.len() == 1 {
                            // Dec: atomic_handle -> old_value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::AtomicDec);
                            code.push(vm::Op::LocalSet(dst));

                        // ===================================================================
                        // C21: Event Loop Operations
                        // ===================================================================
                        } else if name == "__arth_event_loop_create" && args.is_empty() {
                            // Create: -> loop_handle
                            code.push(vm::Op::EventLoopCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_register_timer" && args.len() == 2 {
                            // RegisterTimer: loop_handle, timeout_ms -> token
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::EventLoopRegisterTimer);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_register_fd" && args.len() == 3 {
                            // RegisterFd: loop_handle, fd, interest -> token
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::EventLoopRegisterFd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_deregister" && args.len() == 2 {
                            // Deregister: loop_handle, token -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::EventLoopDeregister);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_poll" && args.len() == 2 {
                            // Poll: loop_handle, timeout_ms -> num_events
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::EventLoopPoll);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_get_event" && args.len() == 1 {
                            // GetEvent: index -> token
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EventLoopGetEvent);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_get_event_type" && args.len() == 1 {
                            // GetEventType: index -> event_type
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EventLoopGetEventType);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_close" && args.len() == 1 {
                            // Close: loop_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EventLoopClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_pipe_create" && args.is_empty() {
                            // PipeCreate: -> read_fd
                            code.push(vm::Op::EventLoopPipeCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_pipe_get_write_fd" && args.is_empty() {
                            // PipeGetWriteFd: -> write_fd
                            code.push(vm::Op::EventLoopPipeGetWriteFd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_pipe_write" && args.len() == 2 {
                            // PipeWrite: write_fd, value -> bytes_written
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::EventLoopPipeWrite);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_pipe_read" && args.len() == 1 {
                            // PipeRead: read_fd -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EventLoopPipeRead);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_event_loop_pipe_close" && args.len() == 1 {
                            // PipeClose: fd -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EventLoopPipeClose);
                            code.push(vm::Op::LocalSet(dst));

                        // ===================================================================
                        // C22: Timer Operations
                        // ===================================================================
                        } else if name == "__arth_timer_sleep" && args.len() == 1 {
                            // Sleep: ms -> ()
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerSleep);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_sleep_async" && args.len() == 2 {
                            // SleepAsync: ms, task_id -> timer_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::TimerSleepAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_task_yield" && args.is_empty() {
                            // Task yield: cooperatively yield via zero-duration timer sleep.
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::TimerSleep);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_task_check_cancelled" && args.is_empty() {
                            // Current-task cancellation check stub.
                            // The VM currently has no ambient current-task context in this path,
                            // so this reports "not cancelled" (0).
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_check_expired" && args.len() == 1 {
                            // CheckExpired: timer_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerCheckExpired);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_get_waiting_task" && args.len() == 1 {
                            // GetWaitingTask: timer_id -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerGetWaitingTask);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_cancel" && args.len() == 1 {
                            // Cancel: timer_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerCancel);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_poll_expired" && args.is_empty() {
                            // PollExpired: -> timer_id
                            code.push(vm::Op::TimerPollExpired);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_now" && args.is_empty() {
                            // Now: -> ms
                            code.push(vm::Op::TimerNow);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_elapsed" && args.len() == 1 {
                            // Elapsed: start_ms -> elapsed_ms
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerElapsed);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_remove" && args.len() == 1 {
                            // Remove: timer_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerRemove);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_timer_remaining" && args.len() == 1 {
                            // Remaining: timer_id -> remaining_ms
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TimerRemaining);
                            code.push(vm::Op::LocalSet(dst));

                        // ================================================
                        // C23: TCP Socket Operations
                        // ================================================
                        } else if name == "__arth_tcp_listener_bind" && args.len() == 1 {
                            // Bind: port -> listener_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpListenerBind);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_listener_accept" && args.len() == 1 {
                            // Accept: listener_handle -> stream_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpListenerAccept);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_listener_accept_async" && args.len() == 2 {
                            // AcceptAsync: listener_handle, task_id -> request_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::TcpListenerAcceptAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_listener_close" && args.len() == 1 {
                            // Close: listener_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpListenerClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_listener_local_port" && args.len() == 1 {
                            // LocalPort: listener_handle -> port
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpListenerLocalPort);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_connect" && args.len() == 2 {
                            // Connect: host_str, port -> stream_handle
                            // Get host string and add to strings table
                            let host_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let host_idx = str_index(&mut out_strings, &host_str);
                            code.push(vm::Op::PushI64(host_idx as i64));
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::TcpStreamConnect);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_connect_async" && args.len() == 3 {
                            // ConnectAsync: host_str, port, task_id -> request_id
                            let host_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let host_idx = str_index(&mut out_strings, &host_str);
                            code.push(vm::Op::PushI64(host_idx as i64));
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::TcpStreamConnectAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_read" && args.len() == 2 {
                            // Read: stream_handle, max_bytes -> bytes_read
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::TcpStreamRead);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_read_async" && args.len() == 3 {
                            // ReadAsync: stream_handle, max_bytes, task_id -> request_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::TcpStreamReadAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_write" && args.len() == 2 {
                            // Write: stream_handle, data_str -> bytes_written
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            let data_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let data_idx = str_index(&mut out_strings, &data_str);
                            code.push(vm::Op::PushI64(data_idx as i64));
                            code.push(vm::Op::TcpStreamWrite);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_write_async" && args.len() == 3 {
                            // WriteAsync: stream_handle, data_str, task_id -> request_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            let data_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let data_idx = str_index(&mut out_strings, &data_str);
                            code.push(vm::Op::PushI64(data_idx as i64));
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::TcpStreamWriteAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_close" && args.len() == 1 {
                            // Close: stream_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpStreamClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_get_last_read_string"
                            && args.is_empty()
                        {
                            // GetLastRead: -> str_len
                            code.push(vm::Op::TcpStreamGetLastRead);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_stream_set_timeout" && args.len() == 2 {
                            // SetTimeout: stream_handle, timeout_ms -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::TcpStreamSetTimeout);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_check_ready" && args.len() == 1 {
                            // CheckReady: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpCheckReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_get_result" && args.len() == 1 {
                            // GetResult: request_id -> result
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpGetResult);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_poll_ready" && args.is_empty() {
                            // PollReady: -> request_id
                            code.push(vm::Op::TcpPollReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_remove_request" && args.len() == 1 {
                            // RemoveRequest: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpRemoveRequest);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_tcp_get_waiting_task" && args.len() == 1 {
                            // GetWaitingTask: request_id -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TcpGetResult); // Reuse TcpGetResult for task lookup
                            code.push(vm::Op::LocalSet(dst));

                        // ================================================
                        // C24: HTTP Client Operations
                        // ================================================
                        } else if name == "__arth_http_get" && args.len() == 2 {
                            // HttpGet: url, timeout_ms -> response_handle
                            // Push URL string
                            let url_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let url_ix = str_index(&mut out_strings, &url_str);
                            code.push(vm::Op::PushStr(url_ix));
                            // Push timeout
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HttpGet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_post" && args.len() == 3 {
                            // HttpPost: url, body, timeout_ms -> response_handle
                            // Push URL string
                            let url_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let url_ix = str_index(&mut out_strings, &url_str);
                            code.push(vm::Op::PushStr(url_ix));
                            // Push body string
                            let body_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let body_ix = str_index(&mut out_strings, &body_str);
                            code.push(vm::Op::PushStr(body_ix));
                            // Push timeout
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HttpPost);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_async" && args.len() == 3 {
                            // HttpGetAsync: url, timeout_ms, task_id -> request_id
                            // Push URL string
                            let url_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let url_ix = str_index(&mut out_strings, &url_str);
                            code.push(vm::Op::PushStr(url_ix));
                            // Push timeout
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            // Push task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HttpGetAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_post_async" && args.len() == 4 {
                            // HttpPostAsync: url, body, timeout_ms, task_id -> request_id
                            // Push URL string
                            let url_str = const_str.get(&args[0].0).cloned().unwrap_or_default();
                            let url_ix = str_index(&mut out_strings, &url_str);
                            code.push(vm::Op::PushStr(url_ix));
                            // Push body string
                            let body_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let body_ix = str_index(&mut out_strings, &body_str);
                            code.push(vm::Op::PushStr(body_ix));
                            // Push timeout
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            // Push task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[3].0,
                            );
                            code.push(vm::Op::HttpPostAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_response_status" && args.len() == 1 {
                            // HttpResponseStatus: response_handle -> status_code
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpResponseStatus);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_response_header" && args.len() == 2 {
                            // HttpResponseHeader: response_handle, key -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Push header key string
                            let key_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let key_ix = str_index(&mut out_strings, &key_str);
                            code.push(vm::Op::PushStr(key_ix));
                            code.push(vm::Op::HttpResponseHeader);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_response_body" && args.len() == 1 {
                            // HttpResponseBody: response_handle -> body_str
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpResponseBody);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_response_close" && args.len() == 1 {
                            // HttpResponseClose: response_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpResponseClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_check_ready" && args.len() == 1 {
                            // HttpCheckReady: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpCheckReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_result" && args.len() == 1 {
                            // HttpGetResult: request_id -> response_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpGetResult);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_poll_ready" && args.is_empty() {
                            // HttpPollReady: -> request_id
                            code.push(vm::Op::HttpPollReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_remove_request" && args.len() == 1 {
                            // HttpRemoveRequest: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRemoveRequest);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_body_length" && args.len() == 1 {
                            // HttpGetBodyLength: response_handle -> length
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpGetBodyLength);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_header_count" && args.len() == 1 {
                            // HttpGetHeaderCount: response_handle -> count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpGetHeaderCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_waiting_task" && args.len() == 1 {
                            // HttpGetWaitingTask: request_id -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpGetResult); // Reuse HttpGetResult for task lookup
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_last_header_string" && args.is_empty() {
                            // Get last header string (used after HttpResponseHeader)
                            code.push(vm::Op::HttpPollReady); // Placeholder - returns a string
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_get_last_body_string" && args.is_empty() {
                            // Get last body string (used after HttpResponseBody)
                            code.push(vm::Op::HttpPollReady); // Placeholder - returns a string
                            code.push(vm::Op::LocalSet(dst));

                        // ================================================
                        // C25: HTTP Server Operations
                        // ================================================
                        } else if name == "__arth_http_server_create" && args.len() == 1 {
                            // HttpServerCreate: port -> server_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerCreate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_close" && args.len() == 1 {
                            // HttpServerClose: server_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerClose);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_get_port" && args.len() == 1 {
                            // HttpServerGetPort: server_handle -> port
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerGetPort);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_accept" && args.len() == 1 {
                            // HttpServerAccept: server_handle -> conn_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerAccept);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_accept_async" && args.len() == 2 {
                            // HttpServerAcceptAsync: server_handle, task_id -> request_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HttpServerAcceptAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_method" && args.len() == 1 {
                            // HttpRequestMethod: conn_handle -> method_str
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRequestMethod);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_path" && args.len() == 1 {
                            // HttpRequestPath: conn_handle -> path_str
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRequestPath);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_header" && args.len() == 2 {
                            // HttpRequestHeader: conn_handle, name -> value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            let name_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let name_ix = str_index(&mut out_strings, &name_str);
                            code.push(vm::Op::PushStr(name_ix));
                            code.push(vm::Op::HttpRequestHeader);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_body" && args.len() == 1 {
                            // HttpRequestBody: conn_handle -> body_str
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRequestBody);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_header_count" && args.len() == 1 {
                            // HttpRequestHeaderCount: conn_handle -> count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRequestHeaderCount);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_request_body_length" && args.len() == 1 {
                            // HttpRequestBodyLength: conn_handle -> length
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpRequestBodyLength);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_writer_status" && args.len() == 2 {
                            // HttpWriterStatus: conn_handle, status_code -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HttpWriterStatus);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_writer_header" && args.len() == 3 {
                            // HttpWriterHeader: conn_handle, name, value -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            let name_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let name_ix = str_index(&mut out_strings, &name_str);
                            code.push(vm::Op::PushStr(name_ix));
                            let value_str = const_str.get(&args[2].0).cloned().unwrap_or_default();
                            let value_ix = str_index(&mut out_strings, &value_str);
                            code.push(vm::Op::PushStr(value_ix));
                            code.push(vm::Op::HttpWriterHeader);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_writer_body" && args.len() == 2 {
                            // HttpWriterBody: conn_handle, body -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            let body_str = const_str.get(&args[1].0).cloned().unwrap_or_default();
                            let body_ix = str_index(&mut out_strings, &body_str);
                            code.push(vm::Op::PushStr(body_ix));
                            code.push(vm::Op::HttpWriterBody);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_writer_send" && args.len() == 1 {
                            // HttpWriterSend: conn_handle -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpWriterSend);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_writer_send_async" && args.len() == 2 {
                            // HttpWriterSendAsync: conn_handle, task_id -> request_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HttpWriterSendAsync);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_check_ready" && args.len() == 1 {
                            // HttpServerCheckReady: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerCheckReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_get_result" && args.len() == 1 {
                            // HttpServerGetResult: request_id -> result
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerGetResult);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_poll_ready" && args.is_empty() {
                            // HttpServerPollReady: -> request_id
                            code.push(vm::Op::HttpServerPollReady);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_remove_request" && args.len() == 1 {
                            // HttpServerRemoveRequest: request_id -> status
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerRemoveRequest);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_server_get_waiting_task" && args.len() == 1 {
                            // HttpServerGetWaitingTask: request_id -> task_id
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HttpServerGetResult); // Reuse for task lookup
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_vm_print_str" && !args.is_empty() {
                            let s = args
                                .first()
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let ix = str_index(&mut out_strings, &s);
                            code.push(vm::Op::Print(ix));
                        } else if name == "__arth_vm_print_val" && !args.is_empty() {
                            let v = args[0].0;
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                v,
                            );
                            code.push(vm::Op::PrintTop);
                        } else if name == "__arth_vm_print_str_val" && args.len() >= 2 {
                            let s = args
                                .first()
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let ix = str_index(&mut out_strings, &s);
                            let v = args[1].0;
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                v,
                            );
                            code.push(vm::Op::PrintStrVal(ix));
                        } else if name == "__arth_vm_print_raw" && !args.is_empty() {
                            let s = args
                                .first()
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let ix = str_index(&mut out_strings, &s);
                            code.push(vm::Op::PrintRaw(ix));
                        } else if name == "__arth_vm_print_raw_str_val" && args.len() >= 2 {
                            let s = args
                                .first()
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let ix = str_index(&mut out_strings, &s);
                            let v = args[1].0;
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                v,
                            );
                            code.push(vm::Op::PrintRawStrVal(ix));
                        } else if name == "__arth_math_sqrt" && args.len() == 1 {
                            let a = args[0].0;
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                a,
                            );
                            code.push(vm::Op::SqrtF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_pow" && args.len() == 2 {
                            let a = args[0].0;
                            let b = args[1].0;
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                a,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                b,
                            );
                            code.push(vm::Op::PowF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_sin" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::SinF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_cos" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::CosF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_tan" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TanF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_floor" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::FloorF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_ceil" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::CeilF64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_math_round" && args.len() == 1 {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::RoundF64);
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_math_round_n, __arth_math_min_f, __arth_math_max_f,
                        //          __arth_math_clamp_f, __arth_math_abs_f, __arth_math_min_i,
                        //          __arth_math_max_i, __arth_math_clamp_i, __arth_math_abs_i
                        // These are now pure Arth in stdlib/src/math/Math.arth
                        } else if name == "__arth_vm_print_ln" {
                            code.push(vm::Op::PrintLn);
                        } else if name == "__arth_list_new" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::ListNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_list_push" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push list, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ListPush);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_list_get" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push list, index
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ListGet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_list_set" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push list, index, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::ListSet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_list_len" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ListLen);
                            code.push(vm::Op::LocalSet(dst));
                        // List intrinsics removed: index_of, contains, insert - now pure Arth
                        } else if name == "__arth_list_remove" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push list, index
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::ListRemove);
                            code.push(vm::Op::LocalSet(dst));
                        // List intrinsics removed: clear, reverse, concat, slice, unique - now pure Arth
                        } else if name == "__arth_list_sort" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::ListSort);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_new" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::MapNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_put" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push map, key, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::MapPut);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_get" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push map, key
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MapGet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_len" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MapLen);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_contains_key" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MapContainsKey);
                            code.push(vm::Op::LocalSet(dst));
                        // Map intrinsic removed: contains_value - now pure Arth
                        } else if name == "__arth_map_remove" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MapRemove);
                            code.push(vm::Op::LocalSet(dst));
                        // Map intrinsics removed: clear, is_empty, get_or_default, values - now pure Arth
                        } else if name == "__arth_map_keys" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::MapKeys);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_map_merge" && args.len() == 2 {
                            // Map merge: copies all entries from src to dest (for struct spread)
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push dest map handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Push src map handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::MapMerge);
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_map_values - now pure Arth code in stdlib/src/arth/map.arth

                        // --- String intrinsics ---
                        } else if name == "__arth_str_len" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrLen);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_substring" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push str, start, end
                            for i in 0..3 {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    args[i].0,
                                );
                            }
                            code.push(vm::Op::StrSubstring);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_indexof" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrIndexOf);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_lastindexof" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrLastIndexOf);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_startswith" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrStartsWith);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_endswith" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrEndsWith);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_split" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrSplit);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_trim" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrTrim);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_tolower" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrToLower);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_toupper" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrToUpper);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_replace" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push str, old, new
                            for i in 0..3 {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    args[i].0,
                                );
                            }
                            code.push(vm::Op::StrReplace);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_charat" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrCharAt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_contains" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrContains);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_repeat" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StrRepeat);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_parseint" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrParseInt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_parsefloat" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrParseFloat);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_fromint" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrFromInt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_str_fromfloat" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::StrFromFloat);
                            code.push(vm::Op::LocalSet(dst));

                        // --- Optional<T> intrinsics ---
                        } else if name == "__arth_opt_some" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::OptSome);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_opt_none" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::OptNone);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_opt_is_some" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::OptIsSome);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_opt_is_none" && args.len() == 1 {
                            // isNone is just !isSome - push value, OptIsSome, then negate
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::OptIsSome);
                            // Negate result: push 0, compare equal (isSome==0 means isNone)
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::EqI64);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_opt_unwrap" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::OptUnwrap);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_opt_or_else" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push optional handle, default value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::OptOrElse);
                            code.push(vm::Op::LocalSet(dst));

                        // --- Native Struct intrinsics ---
                        } else if name == "__arth_struct_new" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push type_name, field_count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StructNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_set" && args.len() == 4 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push struct_handle, field_idx, value, field_name
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[3].0,
                            );
                            code.push(vm::Op::StructSet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_get" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push struct_handle, field_idx
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StructGet);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_get_named" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push struct_handle, field_name
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StructGetNamed);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_set_named" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push struct_handle, field_name, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::StructSetNamed);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_copy" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push dest_handle, src_handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StructCopy);
                            code.push(vm::Op::LocalSet(dst));

                        // --- Native Enum intrinsics ---
                        } else if name == "__arth_enum_new" && args.len() == 4 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push enum_name, variant_name, tag, payload_count
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[3].0,
                            );
                            code.push(vm::Op::EnumNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_enum_set_payload" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push enum_handle, payload_idx, value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::EnumSetPayload);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_enum_get_payload" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // push enum_handle, payload_idx
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::EnumGetPayload);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_enum_get_tag" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::EnumGetTag);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_serve" && args.len() == 1 {
                            // HTTP server: intrinsic call lowered from Http.serve(port)
                            // Uses HostCallNet for capability-based dispatch
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallNet(vm::HostNetOp::HttpServe));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_fetch" && args.len() == 1 {
                            // HTTP fetch: pop URL, push task handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallNet(vm::HostNetOp::HttpFetch));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_accept" && args.len() == 1 {
                            // HTTP accept: pop server handle, push task handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallNet(vm::HostNetOp::HttpAccept));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_http_respond" && args.len() == 4 {
                            // HTTP respond: request_handle, status, headers_handle, body_str
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallNet(vm::HostNetOp::HttpRespond));
                            code.push(vm::Op::Pop); // discard result
                        // Removed: __arth_http_request_method, __arth_http_request_path,
                        //          __arth_http_request_header, __arth_http_request_body,
                        //          __arth_http_response_status, __arth_http_response_body
                        // Request/Response data is now accessed via struct fields in pure Arth
                        } else if name == "__arth_json_stringify" && args.len() == 1 {
                            // JSON stringify: convert a value to JSON string
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonStringify);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_parse" && args.len() == 1 {
                            // JSON parse: parse a JSON string into a JsonValue handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonParse);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_struct_to_json" && args.len() == 2 {
                            // Struct to JSON: args[0] = struct handle, args[1] = struct name (string const)
                            // We need to get field names for the struct and pass them to the VM op
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push struct handle
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Get struct name from const_str and look up field names
                            // For now, pass the struct name - the VM will need field info
                            // In a real implementation, we'd look up field names from type info
                            // For MVP, we pass struct name and handle it specially in runtime
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::StructToJson);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_to_struct" && args.len() == 2 {
                            // JSON to struct: args[0] = JSON string, args[1] = struct name
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push JSON string
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Push struct name / field info
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::JsonToStruct);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_get_field" && args.len() == 2 {
                            // JSON getField: args[0] = json handle, args[1] = key string
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::JsonGetField);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_get_index" && args.len() == 2 {
                            // JSON getIndex: args[0] = json handle, args[1] = index
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::JsonGetIndex);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_get_string" && args.len() == 1 {
                            // JSON getString: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonGetString);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_get_number" && args.len() == 1 {
                            // JSON getNumber: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonGetNumber);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_get_bool" && args.len() == 1 {
                            // JSON getBool: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonGetBool);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_is_null" && args.len() == 1 {
                            // JSON isNull: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonIsNull);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_is_object" && args.len() == 1 {
                            // JSON isObject: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonIsObject);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_is_array" && args.len() == 1 {
                            // JSON isArray: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonIsArray);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_array_len" && args.len() == 1 {
                            // JSON arrayLen: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonArrayLen);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_json_keys" && args.len() == 1 {
                            // JSON keys: args[0] = json handle
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::JsonKeys);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_log_emit_str" && args.len() >= 3 {
                            let lvl = args
                                .first()
                                .and_then(|v| const_int.get(&v.0))
                                .copied()
                                .unwrap_or(2);
                            let ev = args
                                .get(1)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let msg = args
                                .get(2)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let fld = args
                                .get(3)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let lvl_s = match lvl {
                                0 => "TRACE",
                                1 => "DEBUG",
                                2 => "INFO",
                                3 => "WARN",
                                _ => "ERROR",
                            };
                            let mut line = String::new();
                            line.push_str(lvl_s);
                            if !ev.is_empty() {
                                line.push(' ');
                                line.push_str(&ev);
                            }
                            if !msg.is_empty() {
                                line.push_str(": ");
                                line.push_str(&msg);
                            }
                            if !fld.is_empty() {
                                line.push(' ');
                                line.push_str(&fld);
                            }
                            let ix = str_index(&mut out_strings, &line);
                            code.push(vm::Op::Print(ix));
                        } else if name == "__arth_log_emit_str_i64" && args.len() >= 5 {
                            // Dynamic integer field
                            let lvl = args
                                .first()
                                .and_then(|v| const_int.get(&v.0))
                                .copied()
                                .unwrap_or(2);
                            let ev = args
                                .get(1)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let msg = args
                                .get(2)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default();
                            let label = args
                                .get(3)
                                .and_then(|v| const_str.get(&v.0))
                                .cloned()
                                .unwrap_or_default(); // already includes leading space and '='
                            let val_id = args[4].0;
                            let lvl_s = match lvl {
                                0 => "TRACE",
                                1 => "DEBUG",
                                2 => "INFO",
                                3 => "WARN",
                                _ => "ERROR",
                            };
                            let mut prefix = String::new();
                            prefix.push_str(lvl_s);
                            if !ev.is_empty() {
                                prefix.push(' ');
                                prefix.push_str(&ev);
                            }
                            if !msg.is_empty() {
                                prefix.push_str(": ");
                                prefix.push_str(&msg);
                            }
                            let pfx_ix = str_index(&mut out_strings, &prefix);
                            code.push(vm::Op::PrintRaw(pfx_ix));
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                val_id,
                            );
                            let lab_ix = str_index(&mut out_strings, &label);
                            code.push(vm::Op::PrintRawStrVal(lab_ix));
                            code.push(vm::Op::PrintLn);
                        }
                        // --- File I/O intrinsics (using HostCallIo) ---
                        else if name == "__arth_file_open" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push path string
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Push mode to stack (HostCallIo expects mode on stack)
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileOpen));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_close" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileClose));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_read" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileRead));
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_file_read_all - now pure Arth in stdlib/src/io/File.arth
                        } else if name == "__arth_file_write" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileWriteStr));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_flush" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileFlush));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_seek" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileSeek));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_size" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileSize));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_exists" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileExists));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_delete" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileDelete));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_copy" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileCopy));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_file_move" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::FileMove));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- Directory operations (using HostCallIo) ---
                        else if name == "__arth_dir_create" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::DirCreate));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_dir_create_all" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::DirCreateAll));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_dir_delete" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::DirDelete));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_dir_list" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::DirList));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_dir_exists" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::DirExists));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_is_dir" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::IsDir));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_is_file" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::IsFile));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- Path operations (using HostCallIo) ---
                        // Removed: __arth_path_join, __arth_path_parent, __arth_path_filename, __arth_path_extension
                        // These are now pure Arth code in stdlib/src/io/Path.arth
                        else if name == "__arth_path_absolute" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::PathAbsolute));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- Console I/O (using HostCallIo) ---
                        else if name == "__arth_console_read_line" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::ConsoleReadLine));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_console_write" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::ConsoleWrite));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_console_writeln" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // For writeln, we still use the Print op since HostCallIo::ConsoleWrite
                            // doesn't add a newline, and the Print/PrintLn ops provide this feature
                            if let Some(s) = const_str.get(&args[0].0) {
                                let ix = str_index(&mut out_strings, s);
                                code.push(vm::Op::Print(ix));
                            } else {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    args[0].0,
                                );
                                code.push(vm::Op::PrintTop);
                                code.push(vm::Op::PrintLn);
                            }
                            code.push(vm::Op::PushI64(0));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_console_write_err" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallIo(vm::HostIoOp::ConsoleWriteErr));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- DateTime operations (using HostCallTime) ---
                        else if name == "__arth_datetime_now" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::HostCallTime(vm::HostTimeOp::DateTimeNow));
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_datetime_from_millis, __arth_datetime_to_millis,
                        //          __arth_datetime_year, __arth_datetime_month, __arth_datetime_day,
                        //          __arth_datetime_hour, __arth_datetime_minute, __arth_datetime_second,
                        //          __arth_datetime_day_of_week, __arth_datetime_day_of_year
                        //          - now pure Arth in stdlib/src/time/DateTime.arth
                        } else if name == "__arth_datetime_parse" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallTime(vm::HostTimeOp::DateTimeParse));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_datetime_format" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallTime(vm::HostTimeOp::DateTimeFormat));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // Duration operations removed - all now pure Arth code
                        // --- Instant operations (using HostCallTime) ---
                        else if name == "__arth_instant_now" && args.is_empty() {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::HostCallTime(vm::HostTimeOp::InstantNow));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_instant_elapsed" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallTime(vm::HostTimeOp::InstantElapsed));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- SQLite database operations (using HostCallDb) ---
                        else if name == "__arth_sqlite_open" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteOpen));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_close" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteClose));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_prepare" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePrepare));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_step" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteStep));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_finalize" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteFinalize));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_reset" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteReset));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_int" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindInt));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_int64" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindInt64));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_double" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindDouble));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_text" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindText));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_blob" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindBlob));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_bind_null" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBindNull));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_int" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnInt));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_int64" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnInt64));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_double" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnDouble));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_text" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnText));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_blob" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnBlob));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_type" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnType));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_count" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnCount));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_column_name" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteColumnName));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_is_null" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteIsNull));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_changes" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteChanges));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_last_insert_rowid" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteLastInsertRowid));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_errmsg" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteErrmsg));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_begin" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteBegin));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_commit" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteCommit));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_rollback" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteRollback));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_release_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteReleaseSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_rollback_to_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteRollbackToSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- PostgreSQL database operations (using HostCallDb) ---
                        else if name == "__arth_pg_connect" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgConnect));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_disconnect" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgDisconnect));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_status" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgStatus));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_query" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgQuery));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_execute" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgExecute));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_prepare" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPrepare));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_execute_prepared" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgExecutePrepared));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_row_count" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgRowCount));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_column_count" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgColumnCount));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_column_name" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgColumnName));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_column_type" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgColumnType));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_value" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetValue));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_int" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetInt));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_int64" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetInt64));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_double" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetDouble));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_text" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetText));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_bytes" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetBytes));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_bool" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetBool));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_is_null" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgIsNull));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_affected_rows" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgAffectedRows));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_begin" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgBegin));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_commit" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgCommit));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_rollback" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgRollback));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_release_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgReleaseSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_rollback_to_savepoint" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgRollbackToSavepoint));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_errmsg" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgErrmsg));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_escape" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            for arg in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgEscape));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_free_result" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgFreeResult));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- Async PostgreSQL operations ---
                        else if name == "__arth_pg_connect_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgConnectAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_disconnect_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgDisconnectAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_status_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgStatusAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_query_async" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgQueryAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_execute_async" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgExecuteAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_prepare_async" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPrepareAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_execute_prepared_async" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgExecutePreparedAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_is_ready" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgIsReady));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_get_async_result" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgGetAsyncResult));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_cancel_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgCancelAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_begin_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgBeginAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_commit_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgCommitAsync));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_rollback_async" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgRollbackAsync));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- SQLite Pool operations ---
                        else if name == "__arth_sqlite_pool_create" && args.len() == 7 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push all 7 args: conn_str, min, max, acquire_timeout, idle_timeout, max_lifetime, test_on_acquire
                            for arg in args.iter() {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePoolCreate));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_pool_close" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePoolClose));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_pool_acquire" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePoolAcquire));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_pool_release" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePoolRelease));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_pool_stats" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqlitePoolStats));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- PostgreSQL Pool operations ---
                        else if name == "__arth_pg_pool_create" && args.len() == 7 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push all 7 args: conn_str, min, max, acquire_timeout, idle_timeout, max_lifetime, test_on_acquire
                            for arg in args.iter() {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    arg.0,
                                );
                            }
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPoolCreate));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_pool_close" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPoolClose));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_pool_acquire" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPoolAcquire));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_pool_release" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPoolRelease));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_pool_stats" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgPoolStats));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- SQLite Transaction Helper operations ---
                        else if name == "__arth_sqlite_tx_scope_begin" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteTxScopeBegin));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_tx_scope_end" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteTxScopeEnd));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_tx_depth" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteTxDepth));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_sqlite_tx_active" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::SqliteTxActive));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- PostgreSQL Transaction Helper operations ---
                        else if name == "__arth_pg_tx_scope_begin" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgTxScopeBegin));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_tx_scope_end" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgTxScopeEnd));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_tx_depth" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgTxDepth));
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_pg_tx_active" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::HostCallDb(vm::HostDbOp::PgTxActive));
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- BigDecimal operations ---
                        else if name == "__arth_bigdecimal_new" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_from_int" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalFromInt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_from_float" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalFromFloat);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_add" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalAdd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_sub" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalSub);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_mul" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalMul);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_div" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::BigDecimalDiv);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_rem" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalRem);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_pow" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalPow);
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_bigdecimal_abs - now pure Arth in stdlib/src/numeric/BigDecimal.arth
                        } else if name == "__arth_bigdecimal_negate" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalNegate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_compare" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigDecimalCompare);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_to_string" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalToString);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_to_int" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalToInt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_to_float" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalToFloat);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_scale" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigDecimalScale);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_set_scale" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::BigDecimalSetScale);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigdecimal_round" && args.len() == 3 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[2].0,
                            );
                            code.push(vm::Op::BigDecimalRound);
                            code.push(vm::Op::LocalSet(dst));
                        }
                        // --- BigInt operations ---
                        else if name == "__arth_bigint_new" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigIntNew);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_from_int" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigIntFromInt);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_add" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntAdd);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_sub" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntSub);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_mul" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntMul);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_div" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntDiv);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_rem" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntRem);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_pow" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntPow);
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_bigint_abs - now pure Arth in stdlib/src/numeric/BigInt.arth
                        } else if name == "__arth_bigint_negate" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigIntNegate);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_compare" && args.len() == 2 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[1].0,
                            );
                            code.push(vm::Op::BigIntCompare);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_to_string" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigIntToString);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_bigint_to_int" && args.len() == 1 {
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::BigIntToInt);
                            code.push(vm::Op::LocalSet(dst));
                        // Removed: __arth_bigint_gcd, __arth_bigint_mod_pow - now pure Arth in stdlib/src/numeric/BigInt.arth

                        // --- Async/Task runtime calls ---
                        // Cooperative State Machine: Create task handle at spawn, execute at await
                        } else if name == "__arth_task_spawn_fn" && args.len() >= 2 {
                            // Async task spawn: __arth_task_spawn_fn(fn_id, argc, arg0, arg1, ...)
                            // Deferred execution: create task handle and store args, don't execute yet
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);

                            // Get fn_id from first arg (should be a constant)
                            let fn_id = const_int.get(&args[0].0).copied().unwrap_or(0);
                            let argc = const_int.get(&args[1].0).copied().unwrap_or(0) as usize;

                            // Push fn_id and argc for TaskSpawn
                            code.push(vm::Op::PushI64(fn_id));
                            code.push(vm::Op::PushI64(argc as i64));
                            code.push(vm::Op::TaskSpawn);

                            // Store task handle temporarily
                            code.push(vm::Op::LocalSet(dst));

                            // Push args to the task via TaskPushArg
                            for i in 0..argc {
                                if i + 2 < args.len() {
                                    // Get the task handle
                                    code.push(vm::Op::LocalGet(dst));
                                    // Push the argument value
                                    let arg_vid = args[i + 2].0;
                                    push_value(
                                        &mut code,
                                        &val_local,
                                        &const_int,
                                        &const_f64,
                                        &const_str,
                                        &mut out_strings,
                                        arg_vid,
                                    );
                                    code.push(vm::Op::TaskPushArg);
                                    code.push(vm::Op::Pop); // Discard TaskPushArg result
                                }
                            }
                            // Task handle is in dst - body will be executed at await
                        } else if name == "__arth_await" && args.len() == 1 {
                            // Await a task: __arth_await(handle) -> result
                            // Deferred execution: TaskAwait will dispatch to body if task is pending
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            code.push(vm::Op::TaskAwait);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "__arth_end_catch" {
                            // End catch: cleanup after handling an exception
                            // For the VM, this is a no-op since exception cleanup is handled
                            // by the VM's exception system (ClearUnwindHandler, etc.)
                            // Just push a placeholder result for the instruction
                            code.push(vm::Op::PushI64(0));
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            code.push(vm::Op::LocalSet(dst));
                        } else if name == "host_call" && args.len() == 1 {
                            // host_call(json_payload) -> result_string
                            // This is a TS guest intrinsic for calling host functions
                            let dst =
                                ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                            // Push the JSON payload argument
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                args[0].0,
                            );
                            // Call the generic host call opcode
                            code.push(vm::Op::HostCallGeneric);
                            code.push(vm::Op::LocalSet(dst));
                        } else {
                            // Generic call: only emit when callee is a known function in this module set
                            // Handle both qualified (Module.func) and unqualified (func) names
                            let qualified_name = if func_names.contains(name) {
                                Some(name.clone())
                            } else {
                                // Try to find a qualified match (e.g., "Main.computeValue" for "computeValue")
                                func_names
                                    .iter()
                                    .find(|n| n.ends_with(&format!(".{}", name)) || *n == name)
                                    .cloned()
                            };

                            // Get the callee name - use qualified name if found locally,
                            // otherwise use the original name (for external/cross-library calls)
                            let callee_name = qualified_name.as_ref().unwrap_or(name);

                            // Push arguments left-to-right
                            for a in args {
                                push_value(
                                    &mut code,
                                    &val_local,
                                    &const_int,
                                    &const_f64,
                                    &const_str,
                                    &mut out_strings,
                                    a.0,
                                );
                            }
                            let pos = code.len();
                            code.push(vm::Op::Call(0));
                            call_patches.push((pos, callee_name.clone()));
                            // Store returned value into result local
                            code.push(vm::Op::LocalSet(dst));
                        }
                    }
                    I::LandingPad { .. } => {
                        // Get the exception value and store in result local
                        // (VM uses runtime type dispatch, so catch_types are handled via GetTypeName)
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::GetException);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::SetUnwindHandler(target_block) => {
                        // Emit SetUnwindHandler with placeholder, patch later
                        let handler_pos = code.len();
                        code.push(vm::Op::SetUnwindHandler(0)); // placeholder
                        patch_jumps.push((handler_pos, target_block.0));
                    }
                    I::ClearUnwindHandler => {
                        code.push(vm::Op::ClearUnwindHandler);
                    }
                    I::Phi(_ops) => {
                        // Not expected from current HIR→IR path (uses allocas). Skip.
                    }
                    I::MakeClosure { func, captures } => {
                        // Create a closure object from function + captured variables
                        // Function ID will be patched later once all functions are processed
                        let num_captures = captures.len() as u32;

                        // Emit ClosureNew with placeholder func_id = 0
                        let closure_new_pos = code.len();
                        code.push(vm::Op::ClosureNew(0, num_captures));
                        closure_patches.push((closure_new_pos, func.clone()));

                        let closure_handle_dst =
                            ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(closure_handle_dst));

                        // Push each captured value and add to closure
                        for cap_val in captures {
                            // Push the captured variable value
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                cap_val.0,
                            );
                            // Push closure handle
                            code.push(vm::Op::LocalGet(closure_handle_dst));
                            // Capture the value
                            code.push(vm::Op::ClosureCapture);
                            code.push(vm::Op::Pop); // Pop the result (0)
                        }

                        // Closure handle is already in the destination local
                    }
                    I::ClosureCall {
                        closure,
                        args,
                        ret: _,
                    } => {
                        // Call a closure indirectly
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);

                        // Push arguments onto stack
                        for arg in args {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                arg.0,
                            );
                        }

                        // Push closure handle
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            closure.0,
                        );

                        // Call the closure
                        code.push(vm::Op::ClosureCall(args.len() as u32));

                        // Store result
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::Drop { value, ty_name } => {
                        // Emit call to the deinit function for this type
                        // The deinit function is in the companion module <TypeName>Fns
                        // Format: pkg.TypeName -> pkg.TypeNameFns.deinit
                        let deinit_fn = if let Some(dot_pos) = ty_name.rfind('.') {
                            let pkg = &ty_name[..dot_pos];
                            let type_name = &ty_name[dot_pos + 1..];
                            format!("{}.{}Fns.deinit", pkg, type_name)
                        } else {
                            format!("{}Fns.deinit", ty_name)
                        };

                        // Push the value to drop onto the stack
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            value.0,
                        );

                        // Emit call to deinit
                        let pos = code.len();
                        code.push(vm::Op::Call(0)); // placeholder, will be patched
                        call_patches.push((pos, deinit_fn));

                        // Drop doesn't produce a meaningful result, but we need to store something
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::CondDrop {
                        value,
                        flag,
                        ty_name,
                    } => {
                        // Conditional drop: only call deinit if flag is 0 (not moved)
                        // Format deinit function name
                        let deinit_fn = if let Some(dot_pos) = ty_name.rfind('.') {
                            let pkg = &ty_name[..dot_pos];
                            let type_name = &ty_name[dot_pos + 1..];
                            format!("{}.{}Fns.deinit", pkg, type_name)
                        } else {
                            format!("{}Fns.deinit", ty_name)
                        };

                        // Push the flag and compare to 0
                        // If flag == 0 (not moved), result is 1 (true), don't jump, do drop
                        // If flag != 0 (moved), result is 0 (false), jump to skip drop
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            flag.0,
                        );
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::EqI64);

                        // JumpIfFalse: if flag != 0 (moved), skip the drop
                        let jump_pos = code.len();
                        code.push(vm::Op::JumpIfFalse(0)); // placeholder, will be patched

                        // Push the value to drop onto the stack
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            value.0,
                        );

                        // Emit call to deinit
                        let pos = code.len();
                        code.push(vm::Op::Call(0)); // placeholder, will be patched
                        call_patches.push((pos, deinit_fn));

                        // Patch the jump to skip here
                        let end_pos = code.len();
                        code[jump_pos] = vm::Op::JumpIfFalse((end_pos - jump_pos) as u32);

                        // CondDrop doesn't produce a meaningful result
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::FieldDrop {
                        value,
                        field_name,
                        ty_name,
                    } => {
                        // Field drop: drop a specific field of a struct
                        // For now, we treat this similar to a regular drop call
                        // In a full implementation, we'd need to extract the field value first
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            value.0,
                        );

                        // Find deinit function for the field type
                        let deinit_fn = format!("{}.deinit", ty_name.replace('.', "_"));
                        let pos = code.len();
                        code.push(vm::Op::Call(0)); // Placeholder for call (will be patched)
                        call_patches.push((pos, deinit_fn.clone()));
                        code.push(vm::Op::Pop); // Drop deinit result

                        // Add comment instruction for debugging
                        let _ = field_name; // Use the field_name to avoid unused warning

                        // FieldDrop doesn't produce a meaningful result
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    // Reference counting operations
                    I::RcAlloc { initial_value } => {
                        // Push value, emit RcAlloc, store handle
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            initial_value.0,
                        );
                        code.push(vm::Op::RcAlloc);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RcInc { handle } => {
                        // Push handle, emit RcInc, store handle
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            handle.0,
                        );
                        code.push(vm::Op::RcInc);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RcDec { handle, ty_name } => {
                        // Push handle, emit RcDec or RcDecWithDeinit
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            handle.0,
                        );
                        if let Some(tn) = ty_name {
                            // Format deinit function name
                            let deinit_fn = if let Some(dot_pos) = tn.rfind('.') {
                                let pkg = &tn[..dot_pos];
                                let type_name = &tn[dot_pos + 1..];
                                format!("{}.{}Fns.deinit", pkg, type_name)
                            } else {
                                format!("{}Fns.deinit", tn)
                            };
                            // Emit RcDecWithDeinit with patched function offset
                            let pos = code.len();
                            code.push(vm::Op::RcDecWithDeinit(0)); // placeholder
                            call_patches.push((pos, deinit_fn));
                        } else {
                            code.push(vm::Op::RcDec);
                        }
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RcLoad { handle } => {
                        // Push handle, emit RcLoad, store value
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            handle.0,
                        );
                        code.push(vm::Op::RcLoad);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RcStore { handle, value } => {
                        // Push handle, push value, emit RcStore
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            handle.0,
                        );
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            value.0,
                        );
                        code.push(vm::Op::RcStore);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RcGetCount { handle } => {
                        // Push handle, emit RcGetCount, store count
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            handle.0,
                        );
                        code.push(vm::Op::RcGetCount);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RegionEnter { region_id } => {
                        // Enter a new allocation region
                        code.push(vm::Op::RegionEnter(*region_id));
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0)); // Result is void/unit
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::RegionExit {
                        region_id,
                        deinit_calls,
                    } => {
                        // Emit deinit calls for values needing cleanup before region exit
                        for (value, ty_name) in deinit_calls {
                            // Push the value to deinit
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                value.0,
                            );
                            // Format deinit function name
                            let deinit_fn = if let Some(dot_pos) = ty_name.rfind('.') {
                                let pkg = &ty_name[..dot_pos];
                                let type_name = &ty_name[dot_pos + 1..];
                                format!("{}.{}Fns.deinit", pkg, type_name)
                            } else {
                                format!("{}Fns.deinit", ty_name)
                            };
                            // Emit call with patched function offset
                            let pos = code.len();
                            code.push(vm::Op::Call(0)); // placeholder
                            call_patches.push((pos, deinit_fn));
                            code.push(vm::Op::Pop); // Drop deinit result
                        }
                        // Exit the region (bulk deallocate)
                        code.push(vm::Op::RegionExit(*region_id));
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0)); // Result is void/unit
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::GetTypeName(value) => {
                        // Get the struct type name for exception type dispatch
                        // Push struct handle, emit StructTypeName
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            value.0,
                        );
                        code.push(vm::Op::StructTypeName);
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    I::ExternCall {
                        name,
                        args,
                        params,
                        ret,
                    } => {
                        // Push args (left-to-right), then emit a single extern call opcode.
                        for a in args {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                a.0,
                            );
                        }

                        // Encode ABI signature (float vs int args) for the VM.
                        let argc_u8: u8 = params.len().min(u8::MAX as usize) as u8;
                        let mut float_mask: u32 = 0;
                        for (i, pty) in params.iter().enumerate() {
                            if *pty == crate::compiler::ir::Ty::F64 {
                                float_mask |= 1u32 << (i as u32);
                            }
                        }
                        let ret_kind: u8 = match *ret {
                            crate::compiler::ir::Ty::F64 => 1,
                            crate::compiler::ir::Ty::Void => 2,
                            _ => 0, // integer-like
                        };

                        let sym = str_index(&mut out_strings, name);
                        code.push(vm::Op::ExternCall {
                            sym,
                            argc: argc_u8,
                            float_mask,
                            ret_kind,
                        });
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::LocalSet(dst));
                    }
                    // Async state machine operations (used by async_lower.rs)
                    // For single-threaded cooperative execution, these are no-ops or simple passthrough
                    I::AsyncFrameAlloc { .. }
                    | I::AsyncFrameFree { .. }
                    | I::AsyncFrameGetState { .. }
                    | I::AsyncFrameSetState { .. }
                    | I::AsyncFrameStore { .. }
                    | I::AsyncFrameLoad { .. }
                    | I::AsyncYield { .. }
                    | I::AsyncCheckCancelled { .. }
                    | I::AwaitPoint { .. } => {
                        // These are state machine operations that need full async runtime
                        // For now, store 0 as result (no-op)
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::LocalSet(dst));
                    }
                    // Native struct/enum operations (LLVM backend only)
                    // VM backend uses hash-map based structs via __arth_struct_* calls
                    I::StructAlloc { .. }
                    | I::StructFieldGet { .. }
                    | I::StructFieldSet { .. }
                    | I::EnumAlloc { .. }
                    | I::EnumGetTag { .. }
                    | I::EnumSetTag { .. }
                    | I::EnumGetPayload { .. }
                    | I::EnumSetPayload { .. } => {
                        // These are native struct/enum operations for the LLVM backend.
                        // The VM backend uses runtime calls instead (__arth_struct_new, etc.)
                        // Store 0 as placeholder result
                        let dst = ensure_local_for(&mut val_local, &mut next_local, inst.result.0);
                        code.push(vm::Op::PushI64(0));
                        code.push(vm::Op::LocalSet(dst));
                    }
                }
            }

            // Terminator lowering
            match &b.term {
                crate::compiler::ir::Terminator::Ret(rv) => {
                    if let Some(v) = rv {
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            v.0,
                        );
                    }
                    code.push(vm::Op::Ret);
                }
                crate::compiler::ir::Terminator::Br(bb) => {
                    // Emit phi assignments for the target block before the jump
                    emit_phi_assignments(
                        &mut code,
                        &val_local,
                        &const_int,
                        &const_f64,
                        &const_str,
                        &mut out_strings,
                        &phi_info,
                        bi as u32,
                        bb.0,
                    );
                    let pos = code.len();
                    code.push(vm::Op::Jump(0));
                    patch_jumps.push((pos, bb.0));
                }
                crate::compiler::ir::Terminator::CondBr {
                    cond,
                    then_bb,
                    else_bb,
                } => {
                    // For CondBr with phi nodes in targets, we need to:
                    // 1. Evaluate condition
                    // 2. If false, jump to else_phi_setup which sets up phis then jumps to else_bb
                    // 3. If true (fall through), set up then_bb phis then jump to then_bb
                    push_value(
                        &mut code,
                        &val_local,
                        &const_int,
                        &const_f64,
                        &const_str,
                        &mut out_strings,
                        cond.0,
                    );
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0)); // Will jump to else_phi_setup

                    // Then branch: emit phi assignments for then_bb, then jump to then_bb
                    emit_phi_assignments(
                        &mut code,
                        &val_local,
                        &const_int,
                        &const_f64,
                        &const_str,
                        &mut out_strings,
                        &phi_info,
                        bi as u32,
                        then_bb.0,
                    );
                    let jmp_then_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    patch_jumps.push((jmp_then_pos, then_bb.0));

                    // Else branch setup: patch JumpIfFalse to here
                    let else_setup_pos = code.len() as u32;
                    if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                        *t = else_setup_pos;
                    }
                    // Emit phi assignments for else_bb, then jump to else_bb
                    emit_phi_assignments(
                        &mut code,
                        &val_local,
                        &const_int,
                        &const_f64,
                        &const_str,
                        &mut out_strings,
                        &phi_info,
                        bi as u32,
                        else_bb.0,
                    );
                    let jmp_else_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    patch_jumps.push((jmp_else_pos, else_bb.0));
                }
                crate::compiler::ir::Terminator::Switch {
                    scrut,
                    default,
                    cases,
                } => {
                    // Compare scrut to each case in order. For each check:
                    //  - if equal: emit phi assignments, then jump to the case block
                    //  - else: fall through to the next check
                    // If none match: emit phi assignments for default, then jump to default.
                    for (val, bb) in cases {
                        // scrut == val ? jump bb : continue
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            scrut.0,
                        );
                        code.push(vm::Op::PushI64(*val));
                        code.push(vm::Op::EqI64);
                        let jif_pos = code.len();
                        code.push(vm::Op::JumpIfFalse(0)); // patched to next check position below

                        // Emit phi assignments for this case's target block
                        emit_phi_assignments(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            &phi_info,
                            bi as u32,
                            bb.0,
                        );
                        let jmp_pos = code.len();
                        code.push(vm::Op::Jump(0)); // patched to case block below
                        // Patch the JumpIfFalse to skip the phi assignments + unconditional jump
                        let next_check_ip = code.len() as u32;
                        if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                            *t = next_check_ip;
                        }
                        // Patch the Jump to branch to the case block
                        patch_jumps.push((jmp_pos, bb.0));
                    }
                    // No match: emit phi assignments for default, then jump to default block
                    emit_phi_assignments(
                        &mut code,
                        &val_local,
                        &const_int,
                        &const_f64,
                        &const_str,
                        &mut out_strings,
                        &phi_info,
                        bi as u32,
                        default.0,
                    );
                    let jmp_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    patch_jumps.push((jmp_pos, default.0));
                }
                crate::compiler::ir::Terminator::Unreachable => {
                    // No-op
                }
                crate::compiler::ir::Terminator::Throw(exc) => {
                    // Throw exception: push exception value onto stack and throw
                    if let Some(v) = exc {
                        push_value(
                            &mut code,
                            &val_local,
                            &const_int,
                            &const_f64,
                            &const_str,
                            &mut out_strings,
                            v.0,
                        );
                    } else {
                        // No exception value - push 0
                        code.push(vm::Op::PushI64(0));
                    }
                    code.push(vm::Op::Throw);
                }
                crate::compiler::ir::Terminator::Panic(msg) => {
                    // Panic: emit the message string and call panic opcode
                    let msg_idx = if let Some(v) = msg {
                        // Check if the value is a known string constant
                        if let Some(s) = const_str.get(&v.0) {
                            str_index(&mut out_strings, s)
                        } else {
                            // Default panic message if we can't resolve the string
                            str_index(&mut out_strings, "panic")
                        }
                    } else {
                        // No message provided, use default
                        str_index(&mut out_strings, "panic")
                    };
                    code.push(vm::Op::Panic(msg_idx));
                }
                crate::compiler::ir::Terminator::Invoke {
                    callee,
                    args,
                    ret: _,
                    result,
                    normal,
                    unwind: _,
                } => {
                    // Invoke is a terminating call - call the function and jump to normal on success
                    // Exception handling is implicit via SetUnwindHandler (already on unwind stack)

                    // Check if function is known - try both direct and qualified names
                    let qualified_callee = if func_names.contains(callee) {
                        Some(callee.clone())
                    } else {
                        // Try to find a qualified match (e.g., "Main.doThrow" for "doThrow")
                        func_names
                            .iter()
                            .find(|qn| qn.ends_with(&format!(".{}", callee)))
                            .cloned()
                    };

                    if let Some(callee_name) = qualified_callee {
                        // Push arguments left-to-right
                        for a in args {
                            push_value(
                                &mut code,
                                &val_local,
                                &const_int,
                                &const_f64,
                                &const_str,
                                &mut out_strings,
                                a.0,
                            );
                        }
                        // Emit call with placeholder (will be patched)
                        let call_pos = code.len();
                        code.push(vm::Op::Call(0));
                        call_patches.push((call_pos, callee_name));

                        // Store result if present
                        if let Some(res) = result {
                            let dst = ensure_local_for(&mut val_local, &mut next_local, res.0);
                            code.push(vm::Op::LocalSet(dst));
                        } else {
                            // Discard result
                            code.push(vm::Op::Pop);
                        }
                    } else {
                        // Unknown function - push placeholder result
                        if result.is_some() {
                            code.push(vm::Op::PushI64(0));
                            if let Some(res) = result {
                                let dst = ensure_local_for(&mut val_local, &mut next_local, res.0);
                                code.push(vm::Op::LocalSet(dst));
                            }
                        }
                    }

                    // Jump to normal continuation
                    let jmp_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    patch_jumps.push((jmp_pos, normal.0));
                }
                crate::compiler::ir::Terminator::PollReturn { .. } => {
                    // PollReturn is used for async state machines
                    // For now, treat as a regular return (single-threaded cooperative execution)
                    code.push(vm::Op::Ret);
                }
            }
        }

        // Patch jump targets for this function
        for (pos, bb_id) in patch_jumps {
            let Some(Some(tgt)) = block_offset.get(bb_id as usize) else {
                continue;
            };
            match &mut code[pos] {
                vm::Op::Jump(t) => {
                    *t = *tgt;
                }
                vm::Op::JumpIfFalse(t) => {
                    *t = *tgt;
                }
                vm::Op::SetUnwindHandler(t) => {
                    *t = *tgt;
                }
                _ => {}
            }
        }
    }

    // Patch calls to function offsets
    // For external functions (not in func_offsets), use CallSymbol
    for (pos, name) in call_patches {
        if let Some(&off) = func_offsets.get(&name) {
            // Local function - patch with offset
            match &mut code[pos] {
                vm::Op::Call(t) => *t = off,
                vm::Op::TaskRunBody(t) => *t = off,
                _ => {}
            }
        } else {
            // External function - use CallSymbol with the function name
            // Add function name to string table if not already present
            let sym_idx = if let Some(idx) = out_strings.iter().position(|s| s == &name) {
                idx as u32
            } else {
                let idx = out_strings.len() as u32;
                out_strings.push(name.clone());
                idx
            };
            code[pos] = vm::Op::CallSymbol(sym_idx);
        }
    }

    // Patch closures to function offsets
    for (pos, name) in closure_patches {
        if let Some(&off) = func_offsets.get(&name)
            && let vm::Op::ClosureNew(func_id, _) = &mut code[pos]
        {
            *func_id = off;
        }
    }

    // Build async dispatch table: fn_id (hash) -> bytecode offset
    let async_dispatch: Vec<(i64, u32)> = async_body_hashes
        .iter()
        .filter_map(|(fn_id, body_name)| {
            func_offsets.get(body_name).map(|&offset| (*fn_id, offset))
        })
        .collect();

    code.push(vm::Op::Halt);
    vm::Program::with_debug_info(out_strings, code, async_dispatch, debug_entries)
}

/// Compile IR to a VM program and return function bytecode offsets.
/// This is used when building libraries that need to export functions with correct offsets.
pub fn compile_ir_to_program_with_offsets(
    funcs: &[crate::compiler::ir::Func],
    strings: &[String],
    providers: &[crate::compiler::ir::Provider],
) -> (vm::Program, HashMap<String, u32>) {
    let prog = compile_ir_to_program(funcs, strings, providers);

    // Extract function offsets from the debug entries.
    // The compile_ir_to_program function records the correct offset for each function
    // in the debug_entries, which we can use directly.
    let func_offsets: HashMap<String, u32> = prog
        .debug_entries
        .iter()
        .map(|entry| (entry.function_name.clone(), entry.offset))
        .collect();

    (prog, func_offsets)
}

// Translate a very small subset of IR (only logging intrinsic) into VM program
#[allow(dead_code)]
pub(crate) fn compile_ir_logs_to_program(
    funcs: &[crate::compiler::ir::Func],
    strings: &[String],
) -> vm::Program {
    let mut out_strings: Vec<String> = Vec::new();
    let mut code: Vec<vm::Op> = Vec::new();

    fn str_index(pool: &mut Vec<String>, s: &str) -> u32 {
        if let Some((i, _)) = pool.iter().enumerate().find(|(_, x)| x == &s) {
            i as u32
        } else {
            pool.push(s.to_string());
            (pool.len() - 1) as u32
        }
    }

    for f in funcs {
        // Track simple constant propagation for strings and ints
        let mut ints: HashMap<u32, i64> = HashMap::new();
        let mut strs: HashMap<u32, String> = HashMap::new();
        for b in &f.blocks {
            for inst in &b.insts {
                match &inst.kind {
                    crate::compiler::ir::InstKind::ConstI64(n) => {
                        ints.insert(inst.result.0, *n);
                    }
                    crate::compiler::ir::InstKind::ConstStr(ix) => {
                        let s = strings.get(*ix as usize).cloned().unwrap_or_default();
                        strs.insert(inst.result.0, s);
                    }
                    crate::compiler::ir::InstKind::Call { name, args, .. } => {
                        if name == "__arth_log_emit_str" && args.len() >= 3 {
                            let lvl = ints.get(&args[0].0).copied().unwrap_or(2);
                            let ev = strs
                                .get(&args[1].0)
                                .cloned()
                                .unwrap_or_else(|| "".to_string());
                            let msg = strs
                                .get(&args[2].0)
                                .cloned()
                                .unwrap_or_else(|| "".to_string());
                            let fld = if args.len() >= 4 {
                                strs.get(&args[3].0).cloned().unwrap_or_default()
                            } else {
                                String::new()
                            };
                            let lvl_s = match lvl {
                                0 => "TRACE",
                                1 => "DEBUG",
                                2 => "INFO",
                                3 => "WARN",
                                _ => "ERROR",
                            };
                            let mut line = String::new();
                            line.push_str(lvl_s);
                            if !ev.is_empty() {
                                line.push(' ');
                                line.push_str(&ev);
                            }
                            if !msg.is_empty() {
                                line.push_str(": ");
                                line.push_str(&msg);
                            }
                            if !fld.is_empty() {
                                line.push(' ');
                                line.push_str(&fld);
                            }
                            let ix = str_index(&mut out_strings, &line);
                            code.push(vm::Op::Print(ix));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    code.push(vm::Op::Halt);
    vm::Program::new(out_strings, code)
}

pub(crate) fn control_messages(kinds: &[crate::compiler::ast::ControlKind]) -> Vec<String> {
    let mut out = Vec::with_capacity(kinds.len());
    for k in kinds {
        let s = match k {
            crate::compiler::ast::ControlKind::If => "control: if",
            crate::compiler::ast::ControlKind::Else => "control: else",
            crate::compiler::ast::ControlKind::While => "control: while",
            crate::compiler::ast::ControlKind::For => "control: for",
            crate::compiler::ast::ControlKind::Switch => "control: switch",
            crate::compiler::ast::ControlKind::Case => "control: case",
            crate::compiler::ast::ControlKind::Default => "control: default",
            crate::compiler::ast::ControlKind::Try => "control: try",
            crate::compiler::ast::ControlKind::Catch => "control: catch",
            crate::compiler::ast::ControlKind::Finally => "control: finally",
            crate::compiler::ast::ControlKind::Break => "control: break",
            crate::compiler::ast::ControlKind::Continue => "control: continue",
            crate::compiler::ast::ControlKind::Return => "control: return",
            crate::compiler::ast::ControlKind::Throw => "control: throw",
        };
        out.push(s.to_string());
    }
    out
}

// compile_block_to_program removed — IR→VM path is used instead.
/*
pub(crate) fn compile_block_to_program(block: &Block) -> vm::Program {
    let mut strings: Vec<String> = Vec::new();
    let mut code: Vec<vm::Op> = Vec::new();
    let mut locals: HashMap<String, u32> = HashMap::new();
    let mut next_local: u32 = 0;

    fn str_index(strings: &mut Vec<String>, s: &str) -> u32 {
        if let Some((i, _)) = strings.iter().enumerate().find(|(_, x)| x == &s) {
            i as u32
        } else {
            strings.push(s.to_string());
            (strings.len() - 1) as u32
        }
    }

    struct LoopCtx {
        break_patches: Vec<usize>,
        continue_target: Option<u32>,
        continue_patches: Vec<usize>,
    }

    fn local_of(locals: &mut HashMap<String, u32>, next_local: &mut u32, name: &str) -> u32 {
        if let Some(ix) = locals.get(name).copied() {
            ix
        } else {
            let ix = *next_local;
            *next_local += 1;
            locals.insert(name.to_string(), ix);
            ix
        }
    }

    fn gen_expr(
        _strings: &mut Vec<String>,
        code: &mut Vec<vm::Op>,
        locals: &mut HashMap<String, u32>,
        _next_local: &mut u32,
        e: &Expr,
    ) {
        match e {
            Expr::Int(n) => code.push(vm::Op::PushI64(*n)),
            Expr::Float(_) => code.push(vm::Op::PushI64(0)),
            Expr::Str(_) => code.push(vm::Op::PushI64(0)),
            Expr::Char(_) => code.push(vm::Op::PushI64(0)),
            Expr::Bool(b) => code.push(vm::Op::PushBool(if *b { 1 } else { 0 })),
            Expr::Await(inner) => {
                // Demo VM: evaluate inner for side effects, push dummy
                gen_expr(_strings, code, locals, _next_local, inner);
                code.push(vm::Op::PushI64(0));
            }
            Expr::Ident(id) => {
                let ix = *locals.get(&id.0).unwrap_or(&0);
                code.push(vm::Op::LocalGet(ix));
            }
            Expr::Unary(op, a) => match op {
                crate::compiler::ast::UnOp::Neg => {
                    // 0 - expr
                    code.push(vm::Op::PushI64(0));
                    gen_expr(_strings, code, locals, _next_local, a);
                    code.push(vm::Op::SubI64);
                }
                crate::compiler::ast::UnOp::Not => {
                    gen_expr(_strings, code, locals, _next_local, a);
                    code.push(vm::Op::PushBool(0));
                    code.push(vm::Op::EqI64);
                }
            },
            Expr::Call(_callee, _args) => {
                // Calls are not modeled in demo VM; push default 0
                code.push(vm::Op::PushI64(0));
            }
            Expr::Member(_, _) | Expr::Index(_, _) => {
                // Not implemented in demo VM yet; push a default 0
                code.push(vm::Op::PushI64(0));
            }
            Expr::Binary(a, op, b) => match op {
                BinOp::And => {
                    gen_expr(_strings, code, locals, _next_local, a);
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0));
                    gen_expr(_strings, code, locals, _next_local, b);
                    let end_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    let false_tgt = code.len() as u32;
                    if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                        *t = false_tgt;
                    }
                    code.push(vm::Op::PushBool(0));
                    let end = code.len() as u32;
                    if let vm::Op::Jump(ref mut t) = code[end_pos] {
                        *t = end;
                    }
                }
                BinOp::Or => {
                    gen_expr(_strings, code, locals, _next_local, a);
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0));
                    code.push(vm::Op::PushBool(1));
                    let jmp_pos = code.len();
                    code.push(vm::Op::Jump(0));
                    let rtgt = code.len() as u32;
                    if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                        *t = rtgt;
                    }
                    gen_expr(_strings, code, locals, _next_local, b);
                    let end = code.len() as u32;
                    if let vm::Op::Jump(ref mut t) = code[jmp_pos] {
                        *t = end;
                    }
                }
                _ => {
                    gen_expr(_strings, code, locals, _next_local, a);
                    gen_expr(_strings, code, locals, _next_local, b);
                    match op {
                        BinOp::Add => code.push(vm::Op::AddI64),
                        BinOp::Sub => code.push(vm::Op::SubI64),
                        BinOp::Mul => code.push(vm::Op::MulI64),
                        BinOp::Div => code.push(vm::Op::DivI64),
                        BinOp::Mod => code.push(vm::Op::ModI64),
                        BinOp::Lt => code.push(vm::Op::LtI64),
                        BinOp::Gt => {
                            // b < a
                            code.pop(); // remove pushed b
                            code.pop(); // remove pushed a
                            gen_expr(_strings, code, locals, _next_local, b);
                            gen_expr(_strings, code, locals, _next_local, a);
                            code.push(vm::Op::LtI64);
                        }
                        BinOp::Le => {
                            // !(b < a)
                            code.pop(); code.pop();
                            gen_expr(_strings, code, locals, _next_local, b);
                            gen_expr(_strings, code, locals, _next_local, a);
                            code.push(vm::Op::LtI64);
                            code.push(vm::Op::PushBool(0));
                            code.push(vm::Op::EqI64);
                        }
                        BinOp::Ge => {
                            // !(a < b)
                            code.push(vm::Op::LtI64);
                            code.push(vm::Op::PushBool(0));
                            code.push(vm::Op::EqI64);
                        }
                        BinOp::Eq => code.push(vm::Op::EqI64),
                        BinOp::Ne => {
                            code.push(vm::Op::EqI64);
                            code.push(vm::Op::PushBool(0));
                            code.push(vm::Op::EqI64);
                        }
                        BinOp::And | BinOp::Or => {
                            // And/Or should be lowered to short-circuit control flow
                            // before reaching codegen. If we get here, it's a compiler
                            // bug — emit a halt instead of panicking.
                            eprintln!("error: internal: And/Or operators should be lowered before VM codegen");
                            code.push(vm::Op::Halt);
                        }
                    }
                }
            },
        }
    }

    fn gen_block(
        strings: &mut Vec<String>,
        code: &mut Vec<vm::Op>,
        locals: &mut HashMap<String, u32>,
        next_local: &mut u32,
        blk: &Block,
        loop_stack: &mut Vec<LoopCtx>,
        switch_breaks: &mut Vec<Vec<usize>>,
    ) {
        for s in &blk.stmts {
            match s {
                Stmt::VarDecl { ty:_, name, init, .. } => {
                    if let Some(e) = init { gen_expr(strings, code, locals, next_local, e); }
                    else { code.push(vm::Op::PushI64(0)); }
                    let ix = local_of(locals, next_local, &name.0);
                    code.push(vm::Op::LocalSet(ix));
                }
                Stmt::PrintStr(t) => {
                    let ix = str_index(strings, t);
                    code.push(vm::Op::Print(ix));
                }
                Stmt::PrintExpr(e) => {
                    gen_expr(strings, code, locals, next_local, e);
                    code.push(vm::Op::PrintTop);
                }
                Stmt::If {
                    cond,
                    then_blk,
                    else_blk,
                } => {
                    gen_expr(strings, code, locals, next_local, cond);
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0));
                    gen_block(
                        strings,
                        code,
                        locals,
                        next_local,
                        then_blk,
                        loop_stack,
                        switch_breaks,
                    );
                    if let Some(eb) = else_blk {
                        let jmp_pos = code.len();
                        code.push(vm::Op::Jump(0));
                        let tgt = code.len() as u32;
                        if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                            *t = tgt;
                        }
                        gen_block(
                            strings,
                            code,
                            locals,
                            next_local,
                            eb,
                            loop_stack,
                            switch_breaks,
                        );
                        let end = code.len() as u32;
                        if let vm::Op::Jump(ref mut t) = code[jmp_pos] {
                            *t = end;
                        }
                    } else {
                        let tgt = code.len() as u32;
                        if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                            *t = tgt;
                        }
                    }
                }
                Stmt::While { cond, body } => {
                    let loop_start = code.len() as u32;
                    loop_stack.push(LoopCtx {
                        break_patches: Vec::new(),
                        continue_target: Some(loop_start),
                        continue_patches: Vec::new(),
                    });
                    gen_expr(strings, code, locals, next_local, cond);
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0));
                    gen_block(
                        strings,
                        code,
                        locals,
                        next_local,
                        body,
                        loop_stack,
                        switch_breaks,
                    );
                    code.push(vm::Op::Jump(loop_start));
                    let end = code.len() as u32;
                    if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                        *t = end;
                    }
                    if let Some(ctx) = loop_stack.pop() {
                        for bp in ctx.break_patches {
                            if let vm::Op::Jump(ref mut t) = code[bp] {
                                *t = end;
                            }
                        }
                    }
                }
                Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    if let Some(is) = init {
                        gen_stmt(
                            strings,
                            code,
                            locals,
                            next_local,
                            is,
                            loop_stack,
                            switch_breaks,
                        );
                    }
                    let loop_start = code.len() as u32;
                    loop_stack.push(LoopCtx {
                        break_patches: Vec::new(),
                        continue_target: None,
                        continue_patches: Vec::new(),
                    });
                    if let Some(c) = cond {
                        gen_expr(strings, code, locals, next_local, c);
                    } else {
                        code.push(vm::Op::PushBool(1));
                    }
                    let jif_pos = code.len();
                    code.push(vm::Op::JumpIfFalse(0));
                    gen_block(
                        strings,
                        code,
                        locals,
                        next_local,
                        body,
                        loop_stack,
                        switch_breaks,
                    );
                    let step_start = code.len() as u32;
                    if let Some(last) = loop_stack.last_mut() {
                        last.continue_target = Some(step_start);
                        for jp in last.continue_patches.drain(..) {
                            if let vm::Op::Jump(ref mut t) = code[jp] {
                                *t = step_start;
                            }
                        }
                    }
                    if let Some(st) = step {
                        gen_stmt(
                            strings,
                            code,
                            locals,
                            next_local,
                            st,
                            loop_stack,
                            switch_breaks,
                        );
                    }
                    code.push(vm::Op::Jump(loop_start));
                    let end = code.len() as u32;
                    if let vm::Op::JumpIfFalse(ref mut t) = code[jif_pos] {
                        *t = end;
                    }
                    if let Some(ctx) = loop_stack.pop() {
                        for bp in ctx.break_patches {
                            if let vm::Op::Jump(ref mut t) = code[bp] {
                                *t = end;
                            }
                        }
                    }
                }
                Stmt::Assign { name, expr } => {
                    gen_expr(strings, code, locals, next_local, expr);
                    let ix = local_of(locals, next_local, &name.0);
                    code.push(vm::Op::LocalSet(ix));
                }
                Stmt::AssignOp { name, op, expr } => {
                    let ix = local_of(locals, next_local, &name.0);
                    // load current value
                    code.push(vm::Op::LocalGet(ix));
                    // compute rhs
                    gen_expr(strings, code, locals, next_local, expr);
                    match op {
                        crate::compiler::ast::AssignOp::Add => code.push(vm::Op::AddI64),
                        crate::compiler::ast::AssignOp::Sub => code.push(vm::Op::SubI64),
                        crate::compiler::ast::AssignOp::Mul => code.push(vm::Op::MulI64),
                        crate::compiler::ast::AssignOp::Div => code.push(vm::Op::DivI64),
                        crate::compiler::ast::AssignOp::Mod => code.push(vm::Op::ModI64),
                        crate::compiler::ast::AssignOp::Shl => code.push(vm::Op::ShlI64),
                        crate::compiler::ast::AssignOp::Shr => code.push(vm::Op::ShrI64),
                    }
                    code.push(vm::Op::LocalSet(ix));
                }
                Stmt::Break(_label) => {
                    if let Some(last) = switch_breaks.last_mut() {
                        let jp = code.len();
                        code.push(vm::Op::Jump(0));
                        last.push(jp);
                    } else if let Some(last) = loop_stack.last_mut() {
                        let jp = code.len();
                        code.push(vm::Op::Jump(0));
                        last.break_patches.push(jp);
                    }
                }
                Stmt::Continue(_label) => {
                    if let Some(last) = loop_stack.last_mut() {
                        if let Some(tgt) = last.continue_target {
                            code.push(vm::Op::Jump(tgt));
                        } else {
                            let jp = code.len();
                            code.push(vm::Op::Jump(0));
                            last.continue_patches.push(jp);
                        }
                    }
                }
                Stmt::Labeled { stmt, .. } => {
                    gen_stmt(
                        strings, code, locals, next_local, stmt, loop_stack, switch_breaks,
                    );
                }
                Stmt::Switch {
                    expr,
                    cases,
                    default,
                } => {
                    let tmp_ix = *next_local;
                    *next_local += 1;
                        gen_expr(strings, code, locals, next_local, expr);
                    code.push(vm::Op::LocalSet(tmp_ix));
                    let mut end_jumps: Vec<usize> = Vec::new();
                    switch_breaks.push(Vec::new());
                    let mut next_case_label_pos: Option<usize> = None;
                    for (i, (ce, blk)) in cases.iter().enumerate() {
                        if let Some(pos) = next_case_label_pos.take() {
                            let tgt = code.len() as u32;
                            if let vm::Op::JumpIfFalse(ref mut t) = code[pos] {
                                *t = tgt;
                            }
                        }
                        code.push(vm::Op::LocalGet(tmp_ix));
                        gen_expr(strings, code, locals, next_local, ce);
                        code.push(vm::Op::EqI64);
                        let jif_pos = code.len();
                        code.push(vm::Op::JumpIfFalse(0));
                        gen_block(
                            strings,
                            code,
                            locals,
                            next_local,
                            blk,
                            loop_stack,
                            switch_breaks,
                        );
                        let jmp_end = code.len();
                        code.push(vm::Op::Jump(0));
                        end_jumps.push(jmp_end);
                        next_case_label_pos = Some(jif_pos);
                        if i == cases.len() - 1 && let Some(pos) = next_case_label_pos.take() {
                            let tgt = code.len() as u32;
                            if let vm::Op::JumpIfFalse(ref mut t) = code[pos] {
                                *t = tgt;
                            }
                        }
                    }
                    if let Some(db) = default {
                        gen_block(
                            strings,
                            code,
                            locals,
                            next_local,
                            db,
                            loop_stack,
                            switch_breaks,
                        );
                    }
                    let end = code.len() as u32;
                    for jp in end_jumps {
                        if let vm::Op::Jump(ref mut t) = code[jp] {
                            *t = end;
                        }
                    }
                    if let Some(mut brs) = switch_breaks.pop() {
                        for jp in brs.drain(..) {
                            if let vm::Op::Jump(ref mut t) = code[jp] {
                                *t = end;
                            }
                        }
                    }
                }
                Stmt::Try {
                    try_blk,
                    catches: _,
                    finally_blk,
                } => {
                    gen_block(
                        strings,
                        code,
                        locals,
                        next_local,
                        try_blk,
                        loop_stack,
                        switch_breaks,
                    );
                    if let Some(fb) = finally_blk {
                        gen_block(
                            strings,
                            code,
                            locals,
                            next_local,
                            fb,
                            loop_stack,
                            switch_breaks,
                        );
                    }
                }
                Stmt::Return(_) => {
                    code.push(vm::Op::Halt);
                }
                Stmt::Expr(e) => {
                    // Demo: recognize log.Logger level calls and format to print
                    fn format_log_call(e: &Expr) -> Option<String> {
                        use crate::compiler::ast::Expr as AE;
                        let AE::Call(callee, args) = e else { return None; };
                        let AE::Member(_obj, level_ident) = &**callee else { return None; };
                        let level = level_ident.0.as_str();
                        if !(matches!(level, "trace" | "debug" | "info" | "warn" | "error")) {
                            return None;
                        }
                        let mut event_s: Option<String> = None;
                        let mut message_s: Option<String> = None;
                        if let Some(a0) = args.get(0) { if let AE::Str(s) = a0 { event_s = Some(s.clone()); } }
                        if let Some(a1) = args.get(1) { if let AE::Str(s) = a1 { message_s = Some(s.clone()); } }
                        let mut fields_txt: Vec<String> = Vec::new();
                        if let Some(a2) = args.get(2) {
                            if let AE::Call(fcallee, fargs) = a2 {
                                if let AE::Member(obj2, name2) = &**fcallee {
                                    if name2.0 == "of" {
                                        // Accept either Fields.of(...) or log.Fields.of(...)
                                        let is_fields = match &**obj2 {
                                            AE::Ident(crate::compiler::ast::Ident(s)) if s == "Fields" => true,
                                            AE::Member(pkg, ident) => matches!((**pkg).clone(), AE::Ident(crate::compiler::ast::Ident(p)) if p=="log") && ident.0=="Fields",
                                            _ => false,
                                        };
                                        if is_fields {
                                            let mut it = fargs.iter();
                                            while let Some(k) = it.next() {
                                                let v = it.next();
                                                let key = match k { AE::Str(s)=>s.clone(), _=>"?".to_string() };
                                                let val = match v {
                                                    Some(AE::Str(s)) => s.clone(),
                                                    Some(AE::Int(n)) => n.to_string(),
                                                    Some(AE::Bool(b)) => if *b { "true".to_string() } else { "false".to_string() },
                                                    _ => "?".to_string(),
                                                };
                                                fields_txt.push(format!("{}={}", key, val));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let lvl = match level { "trace"=>"TRACE", "debug"=>"DEBUG", "info"=>"INFO", "warn"=>"WARN", _=>"ERROR" };
                        let mut line = String::new();
                        line.push_str(lvl);
                        if let Some(ev) = event_s { line.push(' '); line.push_str(&ev); }
                        if let Some(msg) = message_s { line.push_str(": "); line.push_str(&msg); }
                        if !fields_txt.is_empty() { line.push_str(" "); line.push_str(&fields_txt.join(" ")); }
                        Some(line)
                    }
                    if let Some(line) = format_log_call(e) {
                        let ix = str_index(strings, &line);
                        code.push(vm::Op::Print(ix));
                    } else {
                        // default: evaluate and drop
                        gen_expr(strings, code, locals, next_local, e);
                        code.push(vm::Op::Pop);
                    }
                }
                Stmt::Block(b) => gen_block(
                    strings,
                    code,
                    locals,
                    next_local,
                    b,
                    loop_stack,
                    switch_breaks,
                ),
            }
        }
    }

    fn gen_stmt(
        strings: &mut Vec<String>,
        code: &mut Vec<vm::Op>,
        locals: &mut HashMap<String, u32>,
        next_local: &mut u32,
        s: &Stmt,
        loop_stack: &mut Vec<LoopCtx>,
        switch_breaks: &mut Vec<Vec<usize>>,
    ) {
        let blk = Block {
            stmts: vec![s.clone()],
            span: crate::compiler::source::Span::new(0, 0),
        };
        gen_block(
            strings,
            code,
            locals,
            next_local,
            &blk,
            loop_stack,
            switch_breaks,
        );
    }

    let mut loop_stack: Vec<LoopCtx> = Vec::new();
    let mut switch_breaks: Vec<Vec<usize>> = Vec::new();
    gen_block(
        &mut strings,
        &mut code,
        &mut locals,
        &mut next_local,
        block,
        &mut loop_stack,
        &mut switch_breaks,
    );
    code.push(vm::Op::Halt);
vm::Program::new(strings, code)
}
*/
