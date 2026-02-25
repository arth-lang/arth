use std::collections::HashMap;

use super::{Func, InstKind};

fn eval_bin(op: &super::BinOp, a: i64, b: i64) -> i64 {
    match op {
        super::BinOp::Add => a.wrapping_add(b),
        super::BinOp::Sub => a.wrapping_sub(b),
        super::BinOp::Mul => a.wrapping_mul(b),
        super::BinOp::Div => {
            if b == 0 {
                a
            } else {
                a / b
            }
        }
        super::BinOp::Mod => {
            if b == 0 {
                0
            } else {
                a % b
            }
        }
        super::BinOp::Shl => a.wrapping_shl((b as u32) & 63),
        super::BinOp::Shr => a.wrapping_shr((b as u32) & 63),
        super::BinOp::And => a & b,
        super::BinOp::Or => a | b,
        super::BinOp::Xor => a ^ b,
    }
}

fn eval_cmp(pred: &super::CmpPred, a: i64, b: i64) -> i64 {
    let res = match pred {
        super::CmpPred::Eq => a == b,
        super::CmpPred::Ne => a != b,
        super::CmpPred::Lt => a < b,
        super::CmpPred::Le => a <= b,
        super::CmpPred::Gt => a > b,
        super::CmpPred::Ge => a >= b,
    };
    if res { 1 } else { 0 }
}

// Simple constant folding with local value table; replaces Binary/Cmp/Copy when inputs are constants.
pub fn const_fold(func: &mut Func) -> bool {
    let mut changed = false;
    for b in &mut func.blocks {
        let mut consts: HashMap<u32, i64> = HashMap::new();
        for inst in &mut b.insts {
            match &mut inst.kind {
                InstKind::ConstI64(v) => {
                    consts.insert(inst.result.0, *v);
                }
                InstKind::ConstF64(_x) => {
                    // No folding for floats in MVP
                }
                InstKind::ConstStr(_ix) => {
                    // Do not fold string constants in arithmetic; keep for later passes.
                }
                InstKind::Copy(v) => {
                    if let Some(&c) = consts.get(&v.0) {
                        inst.kind = InstKind::ConstI64(c);
                        consts.insert(inst.result.0, c);
                        changed = true;
                    }
                }
                InstKind::Binary(op, a, c) => {
                    if let (Some(&va), Some(&vb)) = (consts.get(&a.0), consts.get(&c.0)) {
                        let out = eval_bin(op, va, vb);
                        inst.kind = InstKind::ConstI64(out);
                        consts.insert(inst.result.0, out);
                        changed = true;
                    }
                }
                InstKind::Cmp(pred, a, c) => {
                    if let (Some(&va), Some(&vb)) = (consts.get(&a.0), consts.get(&c.0)) {
                        let out = eval_cmp(pred, va, vb);
                        inst.kind = InstKind::ConstI64(out);
                        consts.insert(inst.result.0, out);
                        changed = true;
                    }
                }
                InstKind::Phi(ops) => {
                    let mut it = ops.iter();
                    if let Some((_, first)) = it.next()
                        && let Some(&c0) = consts.get(&first.0)
                    {
                        let mut all_same = true;
                        for (_, v) in it {
                            if consts.get(&v.0).copied() != Some(c0) {
                                all_same = false;
                                break;
                            }
                        }
                        if all_same {
                            inst.kind = InstKind::ConstI64(c0);
                            consts.insert(inst.result.0, c0);
                            changed = true;
                        }
                    }
                }
                InstKind::StrEq(_, _)
                | InstKind::StrConcat(_, _)
                | InstKind::Load(_)
                | InstKind::Store(_, _)
                | InstKind::Alloca
                | InstKind::Call { .. }
                | InstKind::ExternCall { .. }
                | InstKind::MakeClosure { .. }
                | InstKind::ClosureCall { .. }
                | InstKind::LandingPad { .. }
                | InstKind::SetUnwindHandler(_)
                | InstKind::ClearUnwindHandler
                | InstKind::Drop { .. }
                | InstKind::CondDrop { .. }
                | InstKind::FieldDrop { .. }
                | InstKind::RcAlloc { .. }
                | InstKind::RcInc { .. }
                | InstKind::RcDec { .. }
                | InstKind::RcLoad { .. }
                | InstKind::RcStore { .. }
                | InstKind::RcGetCount { .. }
                | InstKind::RegionEnter { .. }
                | InstKind::RegionExit { .. }
                | InstKind::GetTypeName(_)
                // Async state machine operations - no const folding
                | InstKind::AsyncFrameAlloc { .. }
                | InstKind::AsyncFrameFree { .. }
                | InstKind::AsyncFrameGetState { .. }
                | InstKind::AsyncFrameSetState { .. }
                | InstKind::AsyncFrameLoad { .. }
                | InstKind::AsyncFrameStore { .. }
                | InstKind::AsyncCheckCancelled { .. }
                | InstKind::AsyncYield { .. }
                | InstKind::AwaitPoint { .. }
                // Provider operations - no const folding
                | InstKind::ProviderNew { .. }
                | InstKind::ProviderFieldGet { .. }
                | InstKind::ProviderFieldSet { .. }
                // Native struct/enum operations - no const folding
                | InstKind::StructAlloc { .. }
                | InstKind::StructFieldGet { .. }
                | InstKind::StructFieldSet { .. }
                | InstKind::EnumAlloc { .. }
                | InstKind::EnumGetTag { .. }
                | InstKind::EnumSetTag { .. }
                | InstKind::EnumGetPayload { .. }
                | InstKind::EnumSetPayload { .. } => {}
            }
        }
    }
    changed
}

fn side_effect_free(k: &InstKind) -> bool {
    match k {
        InstKind::ConstI64(_)
        | InstKind::ConstF64(_)
        | InstKind::ConstStr(_)
        | InstKind::Copy(_)
        | InstKind::Binary(_, _, _)
        | InstKind::Cmp(_, _, _)
        | InstKind::StrEq(_, _)
        | InstKind::StrConcat(_, _)
        | InstKind::Load(_)
        | InstKind::Alloca
        | InstKind::Phi(_) => true,
        InstKind::Store(_, _)
        | InstKind::Call { .. }
        | InstKind::ExternCall { .. }
        | InstKind::MakeClosure { .. }
        | InstKind::ClosureCall { .. }
        | InstKind::LandingPad { .. }
        | InstKind::SetUnwindHandler(_)
        | InstKind::ClearUnwindHandler
        | InstKind::Drop { .. }
        | InstKind::CondDrop { .. }
        | InstKind::FieldDrop { .. }
        // RC operations have side effects (memory allocation/deallocation)
        | InstKind::RcAlloc { .. }
        | InstKind::RcInc { .. }
        | InstKind::RcDec { .. }
        | InstKind::RcStore { .. }
        // Region operations have side effects (region allocation/deallocation)
        | InstKind::RegionEnter { .. }
        | InstKind::RegionExit { .. }
        // Async operations have side effects (frame allocation, state changes)
        | InstKind::AsyncFrameAlloc { .. }
        | InstKind::AsyncFrameFree { .. }
        | InstKind::AsyncFrameSetState { .. }
        | InstKind::AsyncFrameStore { .. }
        | InstKind::AsyncYield { .. }
        | InstKind::AwaitPoint { .. }
        // Provider operations have side effects (global state)
        | InstKind::ProviderNew { .. }
        | InstKind::ProviderFieldGet { .. }
        | InstKind::ProviderFieldSet { .. }
        // Native struct/enum operations with side effects
        | InstKind::StructAlloc { .. }
        | InstKind::StructFieldSet { .. }
        | InstKind::EnumAlloc { .. }
        | InstKind::EnumSetTag { .. }
        | InstKind::EnumSetPayload { .. } => false,
        // RcLoad, RcGetCount, and GetTypeName are read-only
        InstKind::RcLoad { .. } | InstKind::RcGetCount { .. } | InstKind::GetTypeName(_) => true,
        // Native struct/enum read operations are side-effect-free
        InstKind::StructFieldGet { .. }
        | InstKind::EnumGetTag { .. }
        | InstKind::EnumGetPayload { .. } => true,
        // AsyncFrameGetState, AsyncFrameLoad, and AsyncCheckCancelled are read-only
        InstKind::AsyncFrameGetState { .. }
        | InstKind::AsyncFrameLoad { .. }
        | InstKind::AsyncCheckCancelled { .. } => true,
    }
}

// Simple dead-code elimination for side-effect-free instructions.
pub fn dce(func: &mut Func) -> bool {
    let mut used: HashMap<u32, usize> = HashMap::new();
    // Count uses in insts and terminators
    for b in &func.blocks {
        for inst in &b.insts {
            match &inst.kind {
                InstKind::ConstI64(_) => {}
                InstKind::ConstF64(_) => {}
                InstKind::ConstStr(_) => {}
                InstKind::Copy(v) => {
                    *used.entry(v.0).or_default() += 1;
                }
                InstKind::Binary(_, a, c)
                | InstKind::Cmp(_, a, c)
                | InstKind::StrEq(a, c)
                | InstKind::StrConcat(a, c) => {
                    *used.entry(a.0).or_default() += 1;
                    *used.entry(c.0).or_default() += 1;
                }
                InstKind::Load(p) => {
                    *used.entry(p.0).or_default() += 1;
                }
                InstKind::Store(p, v) => {
                    *used.entry(p.0).or_default() += 1;
                    *used.entry(v.0).or_default() += 1;
                }
                InstKind::Alloca => {}
                InstKind::Call { args, .. } => {
                    for a in args {
                        *used.entry(a.0).or_default() += 1;
                    }
                }
                InstKind::ExternCall { args, .. } => {
                    for a in args {
                        *used.entry(a.0).or_default() += 1;
                    }
                }
                InstKind::MakeClosure { captures, .. } => {
                    for c in captures {
                        *used.entry(c.0).or_default() += 1;
                    }
                }
                InstKind::ClosureCall { closure, args, .. } => {
                    *used.entry(closure.0).or_default() += 1;
                    for a in args {
                        *used.entry(a.0).or_default() += 1;
                    }
                }
                InstKind::Phi(ops) => {
                    for (_bb, v) in ops {
                        *used.entry(v.0).or_default() += 1;
                    }
                }
                InstKind::LandingPad { .. } => {}
                InstKind::SetUnwindHandler(_) => {}
                InstKind::ClearUnwindHandler => {}
                InstKind::Drop { value, .. } => {
                    *used.entry(value.0).or_default() += 1;
                }
                InstKind::CondDrop { value, flag, .. } => {
                    *used.entry(value.0).or_default() += 1;
                    *used.entry(flag.0).or_default() += 1;
                }
                InstKind::FieldDrop { value, .. } => {
                    *used.entry(value.0).or_default() += 1;
                }
                InstKind::RcAlloc { initial_value } => {
                    *used.entry(initial_value.0).or_default() += 1;
                }
                InstKind::RcInc { handle } => {
                    *used.entry(handle.0).or_default() += 1;
                }
                InstKind::RcDec { handle, .. } => {
                    *used.entry(handle.0).or_default() += 1;
                }
                InstKind::RcLoad { handle } => {
                    *used.entry(handle.0).or_default() += 1;
                }
                InstKind::RcStore { handle, value } => {
                    *used.entry(handle.0).or_default() += 1;
                    *used.entry(value.0).or_default() += 1;
                }
                InstKind::RcGetCount { handle } => {
                    *used.entry(handle.0).or_default() += 1;
                }
                InstKind::RegionEnter { .. } => {}
                InstKind::RegionExit { deinit_calls, .. } => {
                    for (value, _) in deinit_calls {
                        *used.entry(value.0).or_default() += 1;
                    }
                }
                InstKind::GetTypeName(v) => {
                    *used.entry(v.0).or_default() += 1;
                }
                // Async state machine operations
                InstKind::AsyncFrameAlloc { .. } => {}
                InstKind::AsyncFrameFree { frame_ptr } => {
                    *used.entry(frame_ptr.0).or_default() += 1;
                }
                InstKind::AsyncFrameGetState { frame_ptr } => {
                    *used.entry(frame_ptr.0).or_default() += 1;
                }
                InstKind::AsyncFrameSetState { frame_ptr, .. } => {
                    *used.entry(frame_ptr.0).or_default() += 1;
                }
                InstKind::AsyncFrameLoad { frame_ptr, .. } => {
                    *used.entry(frame_ptr.0).or_default() += 1;
                }
                InstKind::AsyncFrameStore {
                    frame_ptr, value, ..
                } => {
                    *used.entry(frame_ptr.0).or_default() += 1;
                    *used.entry(value.0).or_default() += 1;
                }
                InstKind::AsyncCheckCancelled { task_handle } => {
                    *used.entry(task_handle.0).or_default() += 1;
                }
                InstKind::AsyncYield { awaited_task } => {
                    *used.entry(awaited_task.0).or_default() += 1;
                }
                InstKind::AwaitPoint { awaited_task, .. } => {
                    *used.entry(awaited_task.0).or_default() += 1;
                }
                // Provider operations
                InstKind::ProviderNew { values, .. } => {
                    for (_, value) in values {
                        *used.entry(value.0).or_default() += 1;
                    }
                }
                InstKind::ProviderFieldGet { obj, .. } => {
                    *used.entry(obj.0).or_default() += 1;
                }
                InstKind::ProviderFieldSet { obj, value, .. } => {
                    *used.entry(obj.0).or_default() += 1;
                    *used.entry(value.0).or_default() += 1;
                }
                // Native struct/enum operations
                InstKind::StructAlloc { .. } => {}
                InstKind::StructFieldGet { ptr, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                }
                InstKind::StructFieldSet { ptr, value, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                    *used.entry(value.0).or_default() += 1;
                }
                InstKind::EnumAlloc { .. } => {}
                InstKind::EnumGetTag { ptr, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                }
                InstKind::EnumSetTag { ptr, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                }
                InstKind::EnumGetPayload { ptr, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                }
                InstKind::EnumSetPayload { ptr, value, .. } => {
                    *used.entry(ptr.0).or_default() += 1;
                    *used.entry(value.0).or_default() += 1;
                }
            }
        }
        match &b.term {
            super::Terminator::Ret(Some(v)) => {
                *used.entry(v.0).or_default() += 1;
            }
            super::Terminator::Ret(None) => {}
            super::Terminator::Br(_) => {}
            super::Terminator::CondBr { cond, .. } => {
                *used.entry(cond.0).or_default() += 1;
            }
            super::Terminator::Switch { scrut, .. } => {
                *used.entry(scrut.0).or_default() += 1;
            }
            super::Terminator::Unreachable => {}
            super::Terminator::Throw(Some(v)) => {
                *used.entry(v.0).or_default() += 1;
            }
            super::Terminator::Throw(None) => {}
            super::Terminator::Panic(Some(v)) => {
                *used.entry(v.0).or_default() += 1;
            }
            super::Terminator::Panic(None) => {}
            super::Terminator::Invoke { args, .. } => {
                for a in args {
                    *used.entry(a.0).or_default() += 1;
                }
            }
            super::Terminator::PollReturn { value, .. } => {
                if let Some(v) = value {
                    *used.entry(v.0).or_default() += 1;
                }
            }
        }
    }

    let mut changed = false;
    for b in &mut func.blocks {
        // Worklist DCE in reverse
        let mut i = b.insts.len();
        while i > 0 {
            i -= 1;
            let remove = {
                let inst = &b.insts[i];
                let u = used.get(&inst.result.0).copied().unwrap_or(0);
                u == 0 && side_effect_free(&inst.kind)
            };
            if remove {
                // Decrement uses of operands
                match &b.insts[i].kind {
                    InstKind::Copy(v) => {
                        *used.entry(v.0).or_default() =
                            used.get(&v.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    InstKind::Binary(_, a, c)
                    | InstKind::Cmp(_, a, c)
                    | InstKind::StrEq(a, c)
                    | InstKind::StrConcat(a, c) => {
                        *used.entry(a.0).or_default() =
                            used.get(&a.0).copied().unwrap_or(0).saturating_sub(1);
                        *used.entry(c.0).or_default() =
                            used.get(&c.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    InstKind::Load(p) => {
                        *used.entry(p.0).or_default() =
                            used.get(&p.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    InstKind::Phi(ops) => {
                        for (_bb, v) in ops {
                            *used.entry(v.0).or_default() =
                                used.get(&v.0).copied().unwrap_or(0).saturating_sub(1);
                        }
                    }
                    InstKind::ConstI64(_)
                    | InstKind::ConstF64(_)
                    | InstKind::ConstStr(_)
                    | InstKind::Alloca => {}
                    InstKind::Store(_, _)
                    | InstKind::Call { .. }
                    | InstKind::ExternCall { .. }
                    | InstKind::MakeClosure { .. }
                    | InstKind::ClosureCall { .. }
                    | InstKind::LandingPad { .. }
                    | InstKind::SetUnwindHandler(_)
                    | InstKind::ClearUnwindHandler
                    | InstKind::Drop { .. }
                    | InstKind::CondDrop { .. }
                    | InstKind::FieldDrop { .. }
                    | InstKind::RcAlloc { .. }
                    | InstKind::RcInc { .. }
                    | InstKind::RcDec { .. }
                    | InstKind::RcStore { .. }
                    | InstKind::RegionEnter { .. }
                    | InstKind::RegionExit { .. }
                    // Async ops with side effects (cannot be removed by DCE)
                    | InstKind::AsyncFrameAlloc { .. }
                    | InstKind::AsyncFrameFree { .. }
                    | InstKind::AsyncFrameSetState { .. }
                    | InstKind::AsyncFrameStore { .. }
                    | InstKind::AsyncYield { .. }
                    | InstKind::AwaitPoint { .. } => {}
                    // Handle RcLoad and RcGetCount decrements (they are side-effect-free)
                    InstKind::RcLoad { handle } | InstKind::RcGetCount { handle } => {
                        *used.entry(handle.0).or_default() =
                            used.get(&handle.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    // GetTypeName is side-effect-free and uses one value
                    InstKind::GetTypeName(v) => {
                        *used.entry(v.0).or_default() =
                            used.get(&v.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    // AsyncFrameGetState, AsyncFrameLoad, AsyncCheckCancelled are side-effect-free
                    InstKind::AsyncFrameGetState { frame_ptr }
                    | InstKind::AsyncFrameLoad { frame_ptr, .. } => {
                        *used.entry(frame_ptr.0).or_default() =
                            used.get(&frame_ptr.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    InstKind::AsyncCheckCancelled { task_handle } => {
                        *used.entry(task_handle.0).or_default() =
                            used.get(&task_handle.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    // Provider operations have side effects
                    InstKind::ProviderNew { .. }
                    | InstKind::ProviderFieldGet { .. }
                    | InstKind::ProviderFieldSet { .. } => {}
                    // Native struct/enum read operations (side-effect-free)
                    InstKind::StructFieldGet { ptr, .. }
                    | InstKind::EnumGetTag { ptr, .. }
                    | InstKind::EnumGetPayload { ptr, .. } => {
                        *used.entry(ptr.0).or_default() =
                            used.get(&ptr.0).copied().unwrap_or(0).saturating_sub(1);
                    }
                    // Native struct/enum write operations have side effects
                    InstKind::StructAlloc { .. }
                    | InstKind::StructFieldSet { .. }
                    | InstKind::EnumAlloc { .. }
                    | InstKind::EnumSetTag { .. }
                    | InstKind::EnumSetPayload { .. } => {}
                }
                b.insts.remove(i);
                changed = true;
            }
        }
    }
    changed
}

pub fn run_simple_opts(func: &mut Func) {
    // A couple of rounds of fold + DCE should suffice for small graphs
    for _ in 0..2 {
        let mut ch = false;
        ch |= const_fold(func);
        ch |= dce(func);
        if !ch {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BlockData, Linkage, Terminator};
    use super::*;
    use crate::compiler::ir::{Ty, Value};

    #[test]
    fn const_fold_and_dce_simplifies_block() {
        // entry: %1 = const 4; %2 = const 2; %3 = add %1, %2; ret %3
        let mut f = Func {
            name: "f".into(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".into(),
                span: None,
                insts: vec![
                    super::super::Inst {
                        result: Value(1),
                        kind: InstKind::ConstI64(4),
                        span: None,
                    },
                    super::super::Inst {
                        result: Value(2),
                        kind: InstKind::ConstI64(2),
                        span: None,
                    },
                    super::super::Inst {
                        result: Value(3),
                        kind: InstKind::Binary(super::super::BinOp::Add, Value(1), Value(2)),
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(3))),
            }],
            linkage: Linkage::External,
            span: None,
        };
        run_simple_opts(&mut f);
        let b = &f.blocks[0];
        // Expect only one const remaining with value 6, and ret uses it
        assert_eq!(b.insts.len(), 1);
        match b.insts[0].kind {
            InstKind::ConstI64(v) => assert_eq!(v, 6),
            _ => panic!("expected const"),
        }
        match b.term {
            Terminator::Ret(Some(v)) => assert_eq!(v.0, b.insts[0].result.0),
            _ => panic!("expected ret value"),
        }
    }

    #[test]
    fn dce_removes_dead_alloca() {
        let mut f = Func {
            name: "f".into(),
            params: vec![],
            ret: Ty::Void,
            blocks: vec![BlockData {
                name: "entry".into(),
                span: None,
                insts: vec![super::super::Inst {
                    result: Value(1),
                    kind: InstKind::Alloca,
                    span: None,
                }],
                term: Terminator::Ret(None),
            }],
            linkage: Linkage::External,
            span: None,
        };
        let removed = dce(&mut f);
        assert!(removed);
        assert!(f.blocks[0].insts.is_empty());
    }
}
