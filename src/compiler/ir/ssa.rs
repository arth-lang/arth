use std::collections::{HashSet, VecDeque};

use super::cfg::Cfg;
use super::dom::DomInfo;
use super::{Block, Func, InstKind, Value};

// Utilities for SSA construction. These are structured so we can
// start with phi placement and plug renaming later when HIR→IR
// lowering starts producing variable definitions.

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct PhiPlacement {
    // For each block index, which variable IDs require a phi there.
    // Variable IDs are caller-defined (e.g., per local slot index).
    pub phis: Vec<Vec<usize>>,
}

impl PhiPlacement {
    #[allow(dead_code)]
    pub fn new(blocks: usize) -> Self {
        Self {
            phis: vec![Vec::new(); blocks],
        }
    }
}

// var_defs: for each variable v (index in outer vec), set of blocks that define v.
#[allow(dead_code)]
pub fn place_phis(cfg: &Cfg, dom: &DomInfo, var_defs: &[HashSet<usize>]) -> PhiPlacement {
    let n_blocks = cfg.succs.len();
    let mut placement = PhiPlacement::new(n_blocks);

    for (v, def_blocks) in var_defs.iter().enumerate() {
        let mut has_already = HashSet::<usize>::new();
        let mut work = VecDeque::<usize>::new();
        for &b in def_blocks {
            if b < n_blocks {
                work.push_back(b);
            }
        }
        while let Some(x) = work.pop_front() {
            for &y in &dom.frontier[x] {
                if has_already.insert(y) {
                    placement.phis[y].push(v);
                    // If we place a phi for v at y and y does not define v already,
                    // we need to propagate further.
                    if !def_blocks.contains(&y) {
                        work.push_back(y);
                    }
                }
            }
        }
    }
    placement
}

// Optional helper to actually insert Phi instructions at block tops based on placement.
// This assumes predecessor order is stable; Phi operands are paired with predecessor blocks.
#[allow(dead_code)]
pub fn insert_phis(func: &mut Func, cfg: &Cfg, placement: &PhiPlacement) {
    for (b, vars) in placement.phis.iter().enumerate() {
        if vars.is_empty() {
            continue;
        }
        // Insert phis at the start of block b. We do not yet have the incoming values; they
        // will be filled during renaming when we know SSA names.
        for _v in vars {
            let mut ops = Vec::with_capacity(cfg.preds[b].len());
            for &p in &cfg.preds[b] {
                // Placeholder Value(0) — to be rewritten during SSA renaming.
                ops.push((Block(p as u32), super::Value(0)));
            }
            func.blocks[b].insts.insert(
                0,
                super::Inst {
                    result: super::Value(0),
                    kind: InstKind::Phi(ops),
                    span: None,
                },
            );
        }
    }
}

// Promote eligible stack slots (alloca/load/store pattern) to SSA using
// dominance-frontier phi placement and renaming (mem2reg-like).
// Conservative: only promotes allocas whose address is used exclusively
// by Load/Store as the pointer operand. Other memory writes are left as-is.
pub fn mem2reg_promote(func: &mut Func) {
    // Gather allocas and uses
    let mut allocas: Vec<Value> = Vec::new();
    for b in &func.blocks {
        for inst in &b.insts {
            if let InstKind::Alloca = inst.kind {
                allocas.push(inst.result);
            }
        }
    }
    if allocas.is_empty() {
        return;
    }

    // Map value -> list of (block_idx, inst_idx) where used as ptr operand
    #[derive(Clone, Copy)]
    enum UseKind {
        LoadPtr,
        StorePtr,
        Other,
    }
    let mut uses: std::collections::HashMap<u32, Vec<(usize, usize, UseKind)>> =
        std::collections::HashMap::new();
    for (bi, b) in func.blocks.iter().enumerate() {
        for (ii, inst) in b.insts.iter().enumerate() {
            match &inst.kind {
                InstKind::Load(p) => {
                    uses.entry(p.0)
                        .or_default()
                        .push((bi, ii, UseKind::LoadPtr));
                }
                InstKind::Store(p, _v) => {
                    uses.entry(p.0)
                        .or_default()
                        .push((bi, ii, UseKind::StorePtr));
                }
                InstKind::Call { args, .. } => {
                    for a in args {
                        uses.entry(a.0).or_default().push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::ExternCall { args, .. } => {
                    for a in args {
                        uses.entry(a.0).or_default().push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::MakeClosure { captures, .. } => {
                    for c in captures {
                        uses.entry(c.0).or_default().push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::ClosureCall { closure, args, .. } => {
                    uses.entry(closure.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    for a in args {
                        uses.entry(a.0).or_default().push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::Binary(_, a, b)
                | InstKind::Cmp(_, a, b)
                | InstKind::StrEq(a, b)
                | InstKind::StrConcat(a, b) => {
                    uses.entry(a.0).or_default().push((bi, ii, UseKind::Other));
                    uses.entry(b.0).or_default().push((bi, ii, UseKind::Other));
                }
                InstKind::Copy(v) => {
                    uses.entry(v.0).or_default().push((bi, ii, UseKind::Other));
                }
                InstKind::Phi(ops) => {
                    for (_bb, v) in ops {
                        uses.entry(v.0).or_default().push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::Alloca
                | InstKind::LandingPad { .. }
                | InstKind::SetUnwindHandler(_)
                | InstKind::ClearUnwindHandler
                | InstKind::ConstI64(_)
                | InstKind::ConstF64(_)
                | InstKind::ConstStr(_) => {}
                InstKind::Drop { value, .. } => {
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::CondDrop { value, flag, .. } => {
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(flag.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::FieldDrop { value, .. } => {
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcAlloc { initial_value } => {
                    uses.entry(initial_value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcInc { handle } => {
                    uses.entry(handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcDec { handle, .. } => {
                    uses.entry(handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcLoad { handle } => {
                    uses.entry(handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcStore { handle, value } => {
                    uses.entry(handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RcGetCount { handle } => {
                    uses.entry(handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::RegionEnter { .. } => {}
                InstKind::RegionExit { deinit_calls, .. } => {
                    for (value, _) in deinit_calls {
                        uses.entry(value.0)
                            .or_default()
                            .push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::GetTypeName(v) => {
                    uses.entry(v.0).or_default().push((bi, ii, UseKind::Other));
                }
                // Async state machine operations
                InstKind::AsyncFrameAlloc { .. } => {}
                InstKind::AsyncFrameFree { frame_ptr } => {
                    uses.entry(frame_ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncFrameGetState { frame_ptr } => {
                    uses.entry(frame_ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncFrameSetState { frame_ptr, .. } => {
                    uses.entry(frame_ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncFrameLoad { frame_ptr, .. } => {
                    uses.entry(frame_ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncFrameStore {
                    frame_ptr, value, ..
                } => {
                    uses.entry(frame_ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncCheckCancelled { task_handle } => {
                    uses.entry(task_handle.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AsyncYield { awaited_task } => {
                    uses.entry(awaited_task.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::AwaitPoint { awaited_task, .. } => {
                    uses.entry(awaited_task.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                // Provider operations
                InstKind::ProviderNew { values, .. } => {
                    for (_, value) in values {
                        uses.entry(value.0)
                            .or_default()
                            .push((bi, ii, UseKind::Other));
                    }
                }
                InstKind::ProviderFieldGet { obj, .. } => {
                    uses.entry(obj.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::ProviderFieldSet { obj, value, .. } => {
                    uses.entry(obj.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                // Native struct/enum operations
                InstKind::StructAlloc { .. } => {}
                InstKind::StructFieldGet { ptr, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::StructFieldSet { ptr, value, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::EnumAlloc { .. } => {}
                InstKind::EnumGetTag { ptr, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::EnumSetTag { ptr, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::EnumGetPayload { ptr, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
                InstKind::EnumSetPayload { ptr, value, .. } => {
                    uses.entry(ptr.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                    uses.entry(value.0)
                        .or_default()
                        .push((bi, ii, UseKind::Other));
                }
            }
        }
    }

    // Filter promotable allocas
    let mut promotable: Vec<Value> = Vec::new();
    for a in allocas {
        let u = uses.get(&a.0);
        let ok = match u {
            None => true,
            Some(list) => list
                .iter()
                .all(|(_, _, k)| matches!(k, UseKind::LoadPtr | UseKind::StorePtr)),
        };
        if ok {
            promotable.push(a);
        }
    }
    if promotable.is_empty() {
        return;
    }

    // Build var index mapping for promotable allocas
    let mut var_index: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for (i, v) in promotable.iter().enumerate() {
        var_index.insert(v.0, i);
    }

    // For each var, collect defining blocks (stores).
    let mut var_defs: Vec<HashSet<usize>> = vec![HashSet::new(); promotable.len()];
    for (bi, b) in func.blocks.iter().enumerate() {
        for inst in &b.insts {
            if let InstKind::Store(p, _v) = &inst.kind
                && let Some(&vi) = var_index.get(&p.0)
            {
                var_defs[vi].insert(bi);
            }
        }
    }

    let cfg = Cfg::build(func);
    let dom = DomInfo::compute(func, &cfg);
    let placement = place_phis(&cfg, &dom, &var_defs);

    // Insert phis for our vars only and capture their positions
    // We'll reuse `insert_phis` then build a lookup table.
    insert_phis(func, &cfg, &placement);

    // Map (block,var_idx) -> inst index of phi in that block
    let mut phi_pos: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    for (bi, vars) in placement.phis.iter().enumerate() {
        for (k, &vi) in vars.iter().enumerate() {
            phi_pos.insert((bi, vi), k);
        }
    }

    // Compute next fresh value id
    let mut max_id = 0u32;
    for b in &func.blocks {
        for inst in &b.insts {
            if inst.result.0 > max_id {
                max_id = inst.result.0;
            }
        }
    }
    let mut fresh = || {
        max_id += 1;
        Value(max_id)
    };

    // Stacks for current SSA value per var
    let mut stacks: Vec<Vec<Value>> = vec![Vec::new(); promotable.len()];

    // Recursive renaming over dominator tree
    #[allow(clippy::too_many_arguments)]
    fn rename_block(
        func: &mut Func,
        cfg: &Cfg,
        dom: &DomInfo,
        placement: &PhiPlacement,
        phi_pos: &std::collections::HashMap<(usize, usize), usize>,
        var_index: &std::collections::HashMap<u32, usize>,
        stacks: &mut [Vec<Value>],
        fresh: &mut dyn FnMut() -> Value,
        b: usize,
    ) {
        let mut pushed: Vec<usize> = vec![0; stacks.len()];

        // Assign new names to phis and push onto stacks
        if let Some(vars) = placement.phis.get(b) {
            for &vi in vars {
                if let Some(&pos) = phi_pos.get(&(b, vi)) {
                    let newv = fresh();
                    if let Some(inst) = func.blocks[b].insts.get_mut(pos) {
                        inst.result = newv;
                    }
                    stacks[vi].push(newv);
                    pushed[vi] += 1;
                }
            }
        }

        // Rewrite loads/stores
        let mut i = placement.phis[b].len();
        while i < func.blocks[b].insts.len() {
            let inst = &mut func.blocks[b].insts[i];
            match &mut inst.kind {
                InstKind::Load(p) => {
                    if let Some(&vi) = var_index.get(&p.0) {
                        if let Some(cur) = stacks[vi].last().copied() {
                            inst.kind = InstKind::Copy(cur);
                        } else {
                            inst.kind = InstKind::ConstI64(0);
                        }
                    }
                }
                InstKind::Store(p, v) => {
                    if let Some(&vi) = var_index.get(&p.0) {
                        // Treat as definition: push stored value
                        stacks[vi].push(*v);
                        pushed[vi] += 1;
                        // Make it a no-op copy to keep structure simple
                        inst.kind = InstKind::Copy(*v);
                    }
                }
                _ => {}
            }
            i += 1;
        }

        // Set phi operands in successors
        for &succ in &cfg.succs[b] {
            if let Some(vars) = placement.phis.get(succ) {
                for &vi in vars {
                    if let Some(&pos) = phi_pos.get(&(succ, vi)) {
                        // Determine incoming value first, to avoid borrowing conflicts
                        let incoming = if let Some(cur) = stacks[vi].last().copied() {
                            cur
                        } else {
                            let z = fresh();
                            func.blocks[b].insts.push(super::Inst {
                                result: z,
                                kind: InstKind::ConstI64(0),
                                span: None,
                            });
                            z
                        };
                        if let Some(InstKind::Phi(ops)) =
                            func.blocks[succ].insts.get_mut(pos).map(|it| &mut it.kind)
                        {
                            for (bb, valref) in ops.iter_mut() {
                                if bb.0 as usize == b {
                                    *valref = incoming;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Recurse on dom tree children
        for &child in &dom.tree[b] {
            rename_block(
                func, cfg, dom, placement, phi_pos, var_index, stacks, fresh, child,
            );
        }

        // Pop values defined in this block
        if let Some(vars) = placement.phis.get(b) {
            for &vi in vars {
                for _ in 0..pushed[vi] {}
            } // placeholder to keep structure consistent
        }
        for vi in 0..stacks.len() {
            for _ in 0..pushed[vi] {
                stacks[vi].pop();
            }
        }
    }

    let entry = cfg.rpo.first().copied().unwrap_or(0);
    rename_block(
        func,
        &cfg,
        &dom,
        &placement,
        &phi_pos,
        &var_index,
        &mut stacks,
        &mut fresh,
        entry,
    );
}

// Produce a simple SSA-oriented dump: per-block phi nodes and instruction results.
pub fn dump_ssa(func: &Func) -> String {
    let mut out = String::new();
    for (i, b) in func.blocks.iter().enumerate() {
        use std::fmt::Write as _;
        let _ = writeln!(out, "bb{}({}):", i, b.name);
        // Phis first
        for inst in &b.insts {
            if let InstKind::Phi(ops) = &inst.kind {
                let mut parts = String::new();
                for (k, (bb, v)) in ops.iter().enumerate() {
                    if k > 0 {
                        parts.push_str(", ");
                    }
                    let _ = write!(parts, "[bb{}: %{}]", bb.0, v.0);
                }
                let _ = writeln!(out, "  %{} = phi {}", inst.result.0, parts);
            } else {
                break;
            }
        }
        // All defs in this block
        for inst in &b.insts {
            let kind_s = match &inst.kind {
                InstKind::ConstI64(v) => format!("const {}", v),
                InstKind::ConstF64(v) => format!("const.f64 {}", v),
                InstKind::ConstStr(ix) => format!("const.str str@{}", ix),
                InstKind::Copy(v) => format!("copy %{}", v.0),
                InstKind::Binary(op, a, c) => format!("bin.{:?} %{}, %{}", op, a.0, c.0),
                InstKind::Cmp(pred, a, c) => format!("cmp.{:?} %{}, %{}", pred, a.0, c.0),
                InstKind::StrEq(a, c) => format!("str_eq %{}, %{}", a.0, c.0),
                InstKind::StrConcat(a, c) => format!("str_concat %{}, %{}", a.0, c.0),
                InstKind::Alloca => "alloca".to_string(),
                InstKind::Load(p) => format!("load %{}", p.0),
                InstKind::Store(p, v) => format!("store %{}, %{}", p.0, v.0),
                InstKind::Call { name, args, .. } => {
                    let args_s = args
                        .iter()
                        .map(|v| format!("%{}", v.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("call {}({})", name, args_s)
                }
                InstKind::ExternCall { name, args, .. } => {
                    let args_s = args
                        .iter()
                        .map(|v| format!("%{}", v.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("extern_call {}({})", name, args_s)
                }
                InstKind::MakeClosure { func, captures } => {
                    let caps_s = captures
                        .iter()
                        .map(|v| format!("%{}", v.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("make_closure @{}({})", func, caps_s)
                }
                InstKind::ClosureCall { closure, args, .. } => {
                    let args_s = args
                        .iter()
                        .map(|v| format!("%{}", v.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("closure_call %{}({})", closure.0, args_s)
                }
                InstKind::Phi(_) => continue,
                InstKind::LandingPad {
                    catch_types,
                    is_catch_all,
                } => {
                    let types_str = catch_types.join(", ");
                    if *is_catch_all {
                        format!("landingpad [{}] catch-all", types_str)
                    } else {
                        format!("landingpad [{}]", types_str)
                    }
                }
                InstKind::SetUnwindHandler(target) => {
                    format!("set_unwind_handler bb{}", target.0)
                }
                InstKind::ClearUnwindHandler => "clear_unwind_handler".to_string(),
                InstKind::Drop { value, ty_name } => format!("drop %{} ({})", value.0, ty_name),
                InstKind::CondDrop {
                    value,
                    flag,
                    ty_name,
                } => {
                    format!("conddrop %{}, %{} ({})", value.0, flag.0, ty_name)
                }
                InstKind::FieldDrop {
                    value,
                    field_name,
                    ty_name,
                } => {
                    format!("fielddrop %{}.{} ({})", value.0, field_name, ty_name)
                }
                InstKind::RcAlloc { initial_value } => format!("rc_alloc %{}", initial_value.0),
                InstKind::RcInc { handle } => format!("rc_inc %{}", handle.0),
                InstKind::RcDec { handle, ty_name } => {
                    if let Some(tn) = ty_name {
                        format!("rc_dec %{} ({})", handle.0, tn)
                    } else {
                        format!("rc_dec %{}", handle.0)
                    }
                }
                InstKind::RcLoad { handle } => format!("rc_load %{}", handle.0),
                InstKind::RcStore { handle, value } => {
                    format!("rc_store %{}, %{}", handle.0, value.0)
                }
                InstKind::RcGetCount { handle } => format!("rc_get_count %{}", handle.0),
                InstKind::RegionEnter { region_id } => format!("region_enter {}", region_id),
                InstKind::RegionExit {
                    region_id,
                    deinit_calls,
                } => {
                    let deinits = deinit_calls
                        .iter()
                        .map(|(v, ty)| format!("(%{}, {})", v.0, ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("region_exit {} [{}]", region_id, deinits)
                }
                InstKind::GetTypeName(v) => format!("get_type_name %{}", v.0),
                // Async state machine operations
                InstKind::AsyncFrameAlloc {
                    frame_name,
                    frame_size,
                } => format!("async_frame_alloc {} ({})", frame_name, frame_size),
                InstKind::AsyncFrameFree { frame_ptr } => {
                    format!("async_frame_free %{}", frame_ptr.0)
                }
                InstKind::AsyncFrameGetState { frame_ptr } => {
                    format!("async_frame_get_state %{}", frame_ptr.0)
                }
                InstKind::AsyncFrameSetState {
                    frame_ptr,
                    state_id,
                } => {
                    format!("async_frame_set_state %{}, {}", frame_ptr.0, state_id)
                }
                InstKind::AsyncFrameLoad {
                    frame_ptr,
                    field_offset,
                    ..
                } => format!("async_frame_load %{}, offset={}", frame_ptr.0, field_offset),
                InstKind::AsyncFrameStore {
                    frame_ptr,
                    field_offset,
                    value,
                } => format!(
                    "async_frame_store %{}, offset={}, %{}",
                    frame_ptr.0, field_offset, value.0
                ),
                InstKind::AsyncCheckCancelled { task_handle } => {
                    format!("async_check_cancelled %{}", task_handle.0)
                }
                InstKind::AsyncYield { awaited_task } => {
                    format!("async_yield %{}", awaited_task.0)
                }
                InstKind::AwaitPoint { awaited_task, .. } => {
                    format!("await_point %{}", awaited_task.0)
                }
                // Provider operations
                InstKind::ProviderNew { name, .. } => format!("provider_new {}", name),
                InstKind::ProviderFieldGet {
                    obj,
                    provider,
                    field,
                    ..
                } => format!("provider_field_get %{}, {}.{}", obj.0, provider, field),
                InstKind::ProviderFieldSet {
                    obj,
                    provider,
                    field,
                    value,
                    ..
                } => format!(
                    "provider_field_set %{}, {}.{}, %{}",
                    obj.0, provider, field, value.0
                ),
                // Native struct/enum operations
                InstKind::StructAlloc { type_name } => format!("struct_alloc {}", type_name),
                InstKind::StructFieldGet {
                    ptr,
                    type_name,
                    field_name,
                    field_index,
                } => format!(
                    "struct_field_get %{}, {}.{} (idx={})",
                    ptr.0, type_name, field_name, field_index
                ),
                InstKind::StructFieldSet {
                    ptr,
                    type_name,
                    field_name,
                    field_index,
                    value,
                } => format!(
                    "struct_field_set %{}, {}.{} (idx={}), %{}",
                    ptr.0, type_name, field_name, field_index, value.0
                ),
                InstKind::EnumAlloc { type_name } => format!("enum_alloc {}", type_name),
                InstKind::EnumGetTag { ptr, type_name } => {
                    format!("enum_get_tag %{}, {}", ptr.0, type_name)
                }
                InstKind::EnumSetTag {
                    ptr,
                    type_name,
                    tag,
                } => {
                    format!("enum_set_tag %{}, {}, tag={}", ptr.0, type_name, tag)
                }
                InstKind::EnumGetPayload {
                    ptr,
                    type_name,
                    payload_index,
                } => format!(
                    "enum_get_payload %{}, {}, payload_idx={}",
                    ptr.0, type_name, payload_index
                ),
                InstKind::EnumSetPayload {
                    ptr,
                    type_name,
                    payload_index,
                    value,
                } => format!(
                    "enum_set_payload %{}, {}, payload_idx={}, %{}",
                    ptr.0, type_name, payload_index, value.0
                ),
            };
            let _ = writeln!(out, "  %{} = {}", inst.result.0, kind_s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{BlockData, Func, Linkage, Terminator, Ty};
    use super::*;

    #[test]
    fn phi_placement_diamond() {
        // b0 -> b1, b2; b1 -> b3; b2 -> b3
        let b0 = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::CondBr {
                cond: super::super::Value(0),
                then_bb: Block(1),
                else_bb: Block(2),
            },
            span: None,
        };
        let b1 = BlockData {
            name: "b1".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b2 = BlockData {
            name: "b2".into(),
            insts: vec![],
            term: Terminator::Br(Block(3)),
            span: None,
        };
        let b3 = BlockData {
            name: "join".into(),
            insts: vec![],
            term: Terminator::Ret(None),
            span: None,
        };
        let mut f = Func {
            name: "f".into(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![b0, b1, b2, b3],
            linkage: Linkage::External,
            span: None,
        };
        let cfg = Cfg::build(&f);
        let dom = DomInfo::compute(&f, &cfg);

        // Variable 0 is defined in b1 and b2
        let mut var_defs: Vec<HashSet<usize>> = vec![HashSet::new()];
        var_defs[0].insert(1);
        var_defs[0].insert(2);

        let placement = place_phis(&cfg, &dom, &var_defs);
        // Expect a phi in join block b3 for var 0
        assert_eq!(placement.phis[3], vec![0]);

        insert_phis(&mut f, &cfg, &placement);
        // After insertion, block 3 should start with a Phi
        match f.blocks[3].insts.first() {
            Some(inst) => match inst.kind {
                InstKind::Phi(ref ops) => assert_eq!(ops.len(), 2),
                _ => panic!("expected phi"),
            },
            None => panic!("missing phi"),
        }
    }
}
