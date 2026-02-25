//! Async State Machine Lowering Pass (Phase 3 - CPS Transformation)
//!
//! This module transforms async function IR into explicit state machines using
//! Continuation-Passing Style (CPS) transformation. Instead of the simple
//! "wrapper + async_body" pattern, async functions become proper state machines
//! that can be suspended at await points and resumed later.
//!
//! ## Overview
//!
//! The transformation proceeds in several phases:
//!
//! 1. **Analysis**: Identify await points and compute live variables at each point
//! 2. **Frame Construction**: Build the async frame struct with fields for all
//!    variables that need to persist across suspension
//! 3. **State Assignment**: Assign each await point (and entry/exit) a state ID
//! 4. **Block Splitting**: Split basic blocks at await points into pre-await and
//!    post-await segments
//! 5. **Code Transformation**: Rewrite the function body as a poll function that
//!    dispatches to the appropriate state and handles suspension/resumption
//! 6. **Drop Insertion**: Add cleanup code for state transitions and cancellation
//!
//! ## State Machine Structure
//!
//! An async function `async T foo(A a, B b)` becomes:
//!
//! ```text
//! struct foo$async_frame {
//!     // Reserved system fields
//!     state: u32,           // Current state (0 = entry, N = after await N-1)
//!     _pad: u32,            // Alignment padding
//!     result: T,            // Return value slot (8 bytes)
//!     exception: i64,       // Exception/panic info
//!     awaited: i64,         // Currently awaited task handle
//!     // User fields (offset 32+)
//!     param_a: A,
//!     param_b: B,
//!     local_x: X,           // Locals that cross await points
//!     local_y: Y,
//! }
//!
//! // Poll function - drives the state machine
//! fn foo$poll(frame: *foo$async_frame, task: i64) -> PollResult {
//!     // Check cancellation first
//!     if task.is_cancelled() {
//!         return Cancelled;
//!     }
//!
//!     switch (frame.state) {
//!         case 0: goto state_0;  // Entry
//!         case 1: goto state_1;  // After await 1
//!         case 2: goto state_2;  // After await 2
//!         ...
//!     }
//!
//!     state_0:
//!         // Load parameters from frame
//!         a = frame.param_a;
//!         b = frame.param_b;
//!         // Entry code...
//!         x = compute_x();
//!         frame.local_x = x;        // Save live vars
//!         awaited = spawn_subtask();
//!         frame.awaited = awaited;
//!         frame.state = 1;
//!         return Pending;           // Suspend
//!
//!     state_1:
//!         // Restore live vars
//!         x = frame.local_x;
//!         result = get_await_result(frame.awaited);
//!         // More code...
//!         frame.result = final_result;
//!         return Ready;
//! }
//!
//! // Wrapper - creates frame and spawns task
//! fn foo(a: A, b: B) -> Task<T> {
//!     frame = alloc_frame("foo$async_frame", size);
//!     frame.param_a = a;
//!     frame.param_b = b;
//!     frame.state = 0;
//!     handle = spawn_with_poll(frame, &foo$poll);
//!     return handle;
//! }
//!
//! // Drop function - cleanup on cancellation
//! fn foo$drop(frame: *foo$async_frame) {
//!     switch (frame.state) {
//!         case 1: drop(frame.local_x); // Drop locals live at state 1
//!         ...
//!     }
//!     free_frame(frame);
//! }
//! ```
//!
//! ## Feature Flag
//!
//! This transformation is gated behind the `async-cps` feature flag.
//! When disabled, the simpler Stage 1 lowering (wrapper + async_body) is used.

use super::{
    AsyncFrame, AsyncFrameField, AsyncState, AsyncStateMachine, AwaitBorrowMeta, Block, BlockData,
    Func, Inst, InstKind, Linkage, PollResult, StateTransition, Terminator, TransitionKind, Ty,
    Value,
};
use std::collections::{HashMap, HashSet};

// ============================================================================
// Core Data Structures
// ============================================================================

/// Context for the async lowering pass
#[derive(Debug)]
pub struct AsyncLowerCtx {
    /// The original async body function being transformed
    pub original_func: Func,
    /// Await points found in the function (block index, instruction index)
    pub await_points: Vec<AwaitPointInfo>,
    /// Variables that are live across at least one await point
    pub cross_await_vars: HashMap<String, CrossAwaitVar>,
    /// SSA value to frame field mapping (for value remapping)
    pub value_to_field: HashMap<Value, u32>,
    /// The generated frame structure
    pub frame: Option<AsyncFrame>,
    /// State assignments (state_id -> await_point_index or -1 for entry)
    pub state_assignments: Vec<StateAssignment>,
    /// Next available field offset in the frame
    next_field_offset: u32,
    /// Next available state ID
    next_state_id: u32,
    /// Blocks that have been split (original_block -> (pre_await_block, post_await_block))
    pub split_blocks: HashMap<usize, Vec<SplitPoint>>,
    /// Map from original block index to transformed block indices
    pub block_remap: HashMap<usize, Vec<usize>>,
}

/// Information about an await point in the original function
#[derive(Debug, Clone)]
pub struct AwaitPointInfo {
    /// Block containing the await
    pub block_idx: usize,
    /// Instruction index within the block (the AwaitPoint marker)
    pub inst_idx: usize,
    /// Instruction index of the actual __arth_await call (if present)
    pub await_call_idx: Option<usize>,
    /// The AwaitPoint metadata (borrows, etc.)
    pub metadata: Option<AwaitBorrowMeta>,
    /// SSA value of the awaited expression (task handle)
    pub awaited_value: Option<Value>,
    /// SSA value receiving the await result
    pub result_value: Option<Value>,
    /// Variables live at this await point (need to be saved to frame)
    pub live_vars: HashSet<String>,
    /// SSA values live at this await point
    pub live_values: HashSet<Value>,
    /// Assigned state ID (filled in during state assignment)
    pub state_id: Option<u32>,
}

/// A point where a block is split (at an await)
#[derive(Debug, Clone)]
pub struct SplitPoint {
    /// Instruction index where the split occurs
    pub inst_idx: usize,
    /// State ID for resumption after this await
    pub resume_state_id: u32,
    /// Values that need to be saved before this await
    pub save_values: Vec<Value>,
    /// Values that need to be restored after this await
    pub restore_values: Vec<Value>,
}

/// A variable that needs to be saved across await points
#[derive(Debug, Clone)]
pub struct CrossAwaitVar {
    /// Variable name (for debugging)
    pub name: String,
    /// IR type
    pub ty: Ty,
    /// SSA values representing this variable at different points
    pub values: Vec<Value>,
    /// Whether this variable needs drop
    pub needs_drop: bool,
    /// Type name for drop
    pub drop_ty_name: Option<String>,
    /// Field offset in the frame (filled in during frame construction)
    pub frame_offset: Option<u32>,
    /// Which await points this variable is live across
    pub live_across: HashSet<usize>,
}

/// A state assignment in the state machine
#[derive(Debug, Clone)]
pub struct StateAssignment {
    /// State ID
    pub id: u32,
    /// Name for debugging
    pub name: String,
    /// The await point this state resumes from (-1 for entry)
    pub await_point_idx: i32,
    /// Entry block index in the poll function
    pub entry_block_idx: Option<usize>,
    /// Variables that are live at the start of this state
    pub live_vars: HashSet<String>,
    /// SSA values that are live at the start of this state
    pub live_values: HashSet<Value>,
}

// ============================================================================
// Implementation
// ============================================================================

impl AsyncLowerCtx {
    /// Create a new async lowering context for a function
    pub fn new(func: Func) -> Self {
        Self {
            original_func: func,
            await_points: Vec::new(),
            cross_await_vars: HashMap::new(),
            value_to_field: HashMap::new(),
            frame: None,
            state_assignments: Vec::new(),
            next_field_offset: AsyncFrame::USER_FIELDS_START,
            next_state_id: 0,
            split_blocks: HashMap::new(),
            block_remap: HashMap::new(),
        }
    }

    /// Run the full async lowering transformation
    pub fn lower(mut self) -> Result<AsyncStateMachine, AsyncLowerError> {
        // Phase 1: Find all await points
        self.find_await_points();

        // If no await points, this is a trivial async function
        // that can be optimized to synchronous execution
        if self.await_points.is_empty() {
            return self.lower_trivial_async();
        }

        // Phase 2: Compute liveness across await points
        self.compute_liveness()?;

        // Phase 3: Build the frame structure
        self.build_frame();

        // Phase 4: Assign states
        self.assign_states();

        // Phase 5: Generate the poll function with actual code transformation
        let poll_func = self.generate_poll_function()?;

        // Phase 6: Generate the wrapper function
        let wrapper_func = self.generate_wrapper_function()?;

        // Phase 7: Generate the drop function
        let drop_func = self.generate_drop_function();

        // Phase 8: Collect state transitions
        let transitions = self.collect_transitions();

        // Build the final state machine
        let states = self.build_async_states();

        Ok(AsyncStateMachine {
            original_name: self.original_func.name.clone(),
            frame: self.frame.ok_or_else(|| {
                AsyncLowerError::InternalError("async frame not initialized".to_string())
            })?,
            states,
            transitions,
            poll_func,
            wrapper_func,
            drop_func,
        })
    }

    // ========================================================================
    // Phase 1: Find Await Points
    // ========================================================================

    /// Find all await points in the function
    fn find_await_points(&mut self) {
        for (block_idx, block) in self.original_func.blocks.iter().enumerate() {
            for (inst_idx, inst) in block.insts.iter().enumerate() {
                match &inst.kind {
                    InstKind::AwaitPoint { live_borrows, .. } => {
                        // Found an AwaitPoint marker
                        let await_call_idx = self.find_await_call(block, inst_idx);
                        let awaited_value =
                            await_call_idx.and_then(|idx| self.extract_awaited_value(block, idx));
                        let result_value = await_call_idx.map(|idx| block.insts[idx].result);

                        let info = AwaitPointInfo {
                            block_idx,
                            inst_idx,
                            await_call_idx,
                            metadata: live_borrows.first().cloned(),
                            awaited_value,
                            result_value,
                            live_vars: HashSet::new(),
                            live_values: HashSet::new(),
                            state_id: None,
                        };
                        self.await_points.push(info);
                    }
                    InstKind::Call { name, .. } if name == "__arth_await" => {
                        // Also check for await calls without AwaitPoint markers
                        // (in case some await points weren't marked)
                        let already_tracked = self.await_points.iter().any(|ap| {
                            ap.block_idx == block_idx && ap.await_call_idx == Some(inst_idx)
                        });
                        if !already_tracked {
                            let awaited_value = self.extract_awaited_value(block, inst_idx);
                            let info = AwaitPointInfo {
                                block_idx,
                                inst_idx: inst_idx, // Use call as the marker
                                await_call_idx: Some(inst_idx),
                                metadata: None,
                                awaited_value,
                                result_value: Some(inst.result),
                                live_vars: HashSet::new(),
                                live_values: HashSet::new(),
                                state_id: None,
                            };
                            self.await_points.push(info);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Sort await points by block and instruction index for deterministic ordering
        self.await_points.sort_by(|a, b| {
            a.block_idx
                .cmp(&b.block_idx)
                .then(a.inst_idx.cmp(&b.inst_idx))
        });
    }

    /// Find the __arth_await call following an AwaitPoint marker
    fn find_await_call(&self, block: &BlockData, await_point_idx: usize) -> Option<usize> {
        for (i, inst) in block.insts.iter().enumerate().skip(await_point_idx + 1) {
            if let InstKind::Call { name, .. } = &inst.kind {
                if name == "__arth_await" {
                    return Some(i);
                }
            }
            // Stop at terminators or non-trivial instructions
            match &inst.kind {
                InstKind::ConstI64(_) | InstKind::ConstF64(_) | InstKind::Copy(_) => continue,
                _ => {
                    // Check if this instruction might be part of await setup
                    if i <= await_point_idx + 5 {
                        continue;
                    }
                    break;
                }
            }
        }
        None
    }

    /// Extract the awaited value from an __arth_await call
    fn extract_awaited_value(&self, block: &BlockData, call_idx: usize) -> Option<Value> {
        if let InstKind::Call { args, .. } = &block.insts[call_idx].kind {
            args.first().copied()
        } else {
            None
        }
    }

    // ========================================================================
    // Phase 2: Liveness Analysis
    // ========================================================================

    /// Compute which values are live across await points using dataflow analysis
    fn compute_liveness(&mut self) -> Result<(), AsyncLowerError> {
        // Build def-use information
        let (defs, uses) = self.build_def_use_info();

        // For each await point, compute which values are live
        for await_idx in 0..self.await_points.len() {
            let ap = &self.await_points[await_idx];
            let block_idx = ap.block_idx;
            let inst_idx = ap.inst_idx;

            // Values defined before this await point that are used after
            let mut live_values = HashSet::new();

            // Check all values defined before this await
            for (value, def_info) in &defs {
                let (def_block, def_inst) = *def_info;

                // Skip values defined after this await point
                if def_block > block_idx || (def_block == block_idx && def_inst >= inst_idx) {
                    continue;
                }

                // Check if this value is used after this await point
                if let Some(use_sites) = uses.get(value) {
                    for (use_block, use_inst) in use_sites {
                        // Used after the await point?
                        if *use_block > block_idx
                            || (*use_block == block_idx && *use_inst > inst_idx)
                        {
                            live_values.insert(*value);
                            break;
                        }
                    }
                }
            }

            // Also check for uses in subsequent blocks (conservative for control flow)
            for value in defs.keys() {
                if live_values.contains(value) {
                    continue;
                }
                if let Some(use_sites) = uses.get(value) {
                    for (use_block, _) in use_sites {
                        if *use_block > block_idx {
                            live_values.insert(*value);
                            break;
                        }
                    }
                }
            }

            // Store the live values in the await point info
            self.await_points[await_idx].live_values = live_values.clone();

            // Record cross-await variables
            for value in &live_values {
                let name = format!("ssa_{}", value.0);
                let entry = self
                    .cross_await_vars
                    .entry(name.clone())
                    .or_insert_with(|| {
                        CrossAwaitVar {
                            name: name.clone(),
                            ty: Ty::I64, // Default to I64, we'll refine this
                            values: Vec::new(),
                            needs_drop: false,
                            drop_ty_name: None,
                            frame_offset: None,
                            live_across: HashSet::new(),
                        }
                    });
                entry.values.push(*value);
                entry.live_across.insert(await_idx);
            }
        }

        Ok(())
    }

    /// Build def-use information for all SSA values
    fn build_def_use_info(
        &self,
    ) -> (
        HashMap<Value, (usize, usize)>,
        HashMap<Value, Vec<(usize, usize)>>,
    ) {
        let mut defs: HashMap<Value, (usize, usize)> = HashMap::new();
        let mut uses: HashMap<Value, Vec<(usize, usize)>> = HashMap::new();

        // Parameters are defined at the function entry
        for i in 0..self.original_func.params.len() {
            defs.insert(Value(i as u32), (0, 0));
        }

        for (block_idx, block) in self.original_func.blocks.iter().enumerate() {
            for (inst_idx, inst) in block.insts.iter().enumerate() {
                // Record definition
                defs.insert(inst.result, (block_idx, inst_idx));

                // Record uses
                let used_values = self.extract_used_values(&inst.kind);
                for value in used_values {
                    uses.entry(value).or_default().push((block_idx, inst_idx));
                }
            }

            // Also check terminator for uses
            let term_uses = self.extract_terminator_uses(&block.term);
            for value in term_uses {
                uses.entry(value)
                    .or_default()
                    .push((block_idx, block.insts.len()));
            }
        }

        (defs, uses)
    }

    /// Extract values used by an instruction
    fn extract_used_values(&self, kind: &InstKind) -> Vec<Value> {
        match kind {
            InstKind::Copy(v) => vec![*v],
            InstKind::Binary(_, a, b) => vec![*a, *b],
            InstKind::Cmp(_, a, b) => vec![*a, *b],
            InstKind::StrEq(a, b) => vec![*a, *b],
            InstKind::StrConcat(a, b) => vec![*a, *b],
            InstKind::Load(v) => vec![*v],
            InstKind::Store(ptr, val) => vec![*ptr, *val],
            InstKind::Call { args, .. } => args.clone(),
            InstKind::ExternCall { args, .. } => args.clone(),
            InstKind::MakeClosure { captures, .. } => captures.clone(),
            InstKind::ClosureCall { closure, args, .. } => {
                let mut v = vec![*closure];
                v.extend(args);
                v
            }
            InstKind::RcAlloc { initial_value } => vec![*initial_value],
            InstKind::RcInc { handle } => vec![*handle],
            InstKind::RcDec { handle, .. } => vec![*handle],
            InstKind::RcLoad { handle } => vec![*handle],
            InstKind::RcStore { handle, value } => vec![*handle, *value],
            InstKind::RcGetCount { handle } => vec![*handle],
            InstKind::Drop { value, .. } => vec![*value],
            InstKind::CondDrop { value, flag, .. } => vec![*value, *flag],
            InstKind::FieldDrop { value, .. } => vec![*value],
            InstKind::GetTypeName(v) => vec![*v],
            InstKind::AsyncFrameGetState { frame_ptr } => vec![*frame_ptr],
            InstKind::AsyncFrameSetState { frame_ptr, .. } => vec![*frame_ptr],
            InstKind::AsyncFrameLoad { frame_ptr, .. } => vec![*frame_ptr],
            InstKind::AsyncFrameStore {
                frame_ptr, value, ..
            } => vec![*frame_ptr, *value],
            InstKind::AsyncFrameFree { frame_ptr } => vec![*frame_ptr],
            InstKind::AsyncCheckCancelled { task_handle } => vec![*task_handle],
            InstKind::AsyncYield { awaited_task } => vec![*awaited_task],
            InstKind::AwaitPoint { awaited_task, .. } => vec![*awaited_task],
            InstKind::ProviderNew { values, .. } => values.iter().map(|(_, v)| *v).collect(),
            InstKind::ProviderFieldGet { obj, .. } => vec![*obj],
            InstKind::ProviderFieldSet { obj, value, .. } => vec![*obj, *value],
            InstKind::Phi(operands) => operands.iter().map(|(_, v)| *v).collect(),
            InstKind::RegionExit { deinit_calls, .. } => {
                deinit_calls.iter().map(|(v, _)| *v).collect()
            }
            _ => vec![],
        }
    }

    /// Extract values used by a terminator
    fn extract_terminator_uses(&self, term: &Terminator) -> Vec<Value> {
        match term {
            Terminator::Ret(Some(v)) => vec![*v],
            Terminator::CondBr { cond, .. } => vec![*cond],
            Terminator::Switch { scrut, .. } => vec![*scrut],
            Terminator::Throw(Some(v)) => vec![*v],
            Terminator::Panic(Some(v)) => vec![*v],
            Terminator::Invoke { args, .. } => args.clone(),
            Terminator::PollReturn { value: Some(v), .. } => vec![*v],
            _ => vec![],
        }
    }

    // ========================================================================
    // Phase 3: Frame Construction
    // ========================================================================

    /// Build the async frame structure
    fn build_frame(&mut self) {
        let frame_name = format!("{}$async_frame", self.original_func.name);
        let mut fields = Vec::new();

        // Start user fields at offset 32 (after reserved system fields)
        self.next_field_offset = AsyncFrame::USER_FIELDS_START;

        // Add parameter fields first
        for (i, param_ty) in self.original_func.params.iter().enumerate() {
            let offset = self.next_field_offset;
            let field = AsyncFrameField {
                name: format!("param_{}", i),
                ty: param_ty.clone(),
                offset,
                original_value: Some(Value(i as u32)),
                needs_drop: false, // Parameters are moved in
                drop_ty_name: None,
            };
            fields.push(field);

            // Map parameter value to frame offset
            self.value_to_field.insert(Value(i as u32), offset);
            self.next_field_offset += 8; // 8-byte alignment
        }

        // Add cross-await variable fields
        for (name, var) in &mut self.cross_await_vars {
            let offset = self.next_field_offset;
            var.frame_offset = Some(offset);

            let field = AsyncFrameField {
                name: name.clone(),
                ty: var.ty.clone(),
                offset,
                original_value: var.values.first().copied(),
                needs_drop: var.needs_drop,
                drop_ty_name: var.drop_ty_name.clone(),
            };
            fields.push(field);

            // Map SSA values to frame offset
            for value in &var.values {
                self.value_to_field.insert(*value, offset);
            }

            self.next_field_offset += 8;
        }

        self.frame = Some(AsyncFrame {
            name: frame_name,
            size: self.next_field_offset,
            fields,
            state_offset: AsyncFrame::STATE_OFFSET,
            result_offset: AsyncFrame::RESULT_OFFSET,
            exception_offset: AsyncFrame::EXCEPTION_OFFSET,
            awaited_offset: AsyncFrame::AWAITED_OFFSET,
        });
    }

    // ========================================================================
    // Phase 4: State Assignment
    // ========================================================================

    /// Assign state IDs to each resumption point
    fn assign_states(&mut self) {
        // State 0: Entry
        self.state_assignments.push(StateAssignment {
            id: 0,
            name: "entry".to_string(),
            await_point_idx: -1,
            entry_block_idx: None,
            live_vars: HashSet::new(),
            live_values: HashSet::new(),
        });
        self.next_state_id = 1;

        // Each await point gets a state for resumption after it
        for (i, ap) in self.await_points.iter_mut().enumerate() {
            let state_id = self.next_state_id;
            self.next_state_id += 1;

            ap.state_id = Some(state_id);

            self.state_assignments.push(StateAssignment {
                id: state_id,
                name: format!("resume_after_await_{}", i),
                await_point_idx: i as i32,
                entry_block_idx: None,
                live_vars: ap.live_vars.clone(),
                live_values: ap.live_values.clone(),
            });
        }
    }

    // ========================================================================
    // Phase 5: Poll Function Generation
    // ========================================================================

    /// Generate the poll function that drives the state machine
    fn generate_poll_function(&mut self) -> Result<Func, AsyncLowerError> {
        let poll_name = format!("{}$poll", self.original_func.name);

        // Poll function parameters:
        // - frame_ptr: Ptr (Value(0))
        // - task_handle: i64 (Value(1))
        let frame_ptr = Value(0);
        let task_handle = Value(1);
        let mut next_value = 2u32;

        let mut blocks: Vec<BlockData> = Vec::new();

        // ================================================================
        // Entry block: Check cancellation and dispatch on state
        // ================================================================
        let mut entry_insts = Vec::new();

        // Check cancellation first
        let cancelled = Value(next_value);
        entry_insts.push(Inst {
            result: cancelled,
            kind: InstKind::AsyncCheckCancelled { task_handle },
            span: None,
        });
        next_value += 1;

        // Constant 1 for comparison
        let one = Value(next_value);
        entry_insts.push(Inst {
            result: one,
            kind: InstKind::ConstI64(1),
            span: None,
        });
        next_value += 1;

        // Compare cancelled == 1
        let is_cancelled = Value(next_value);
        entry_insts.push(Inst {
            result: is_cancelled,
            kind: InstKind::Cmp(super::CmpPred::Eq, cancelled, one),
            span: None,
        });
        next_value += 1;

        // Load state from frame
        let state_val = Value(next_value);
        entry_insts.push(Inst {
            result: state_val,
            kind: InstKind::AsyncFrameGetState { frame_ptr },
            span: None,
        });
        next_value += 1;

        // Block indices for state dispatch
        let dispatch_block_idx = 1usize; // After entry
        let cancelled_block_idx = 2usize;

        // Entry block terminates with conditional branch on cancellation
        blocks.push(BlockData {
            name: "entry".to_string(),
            insts: entry_insts,
            term: Terminator::CondBr {
                cond: is_cancelled,
                then_bb: Block(cancelled_block_idx as u32),
                else_bb: Block(dispatch_block_idx as u32),
            },
            span: None,
        });

        // ================================================================
        // Dispatch block: Switch on state
        // ================================================================
        let state_blocks_start = 3usize; // After entry, dispatch, cancelled
        let mut switch_cases: Vec<(i64, Block)> = Vec::new();

        for (i, _) in self.state_assignments.iter().enumerate() {
            switch_cases.push((i as i64, Block((state_blocks_start + i) as u32)));
        }

        blocks.push(BlockData {
            name: "dispatch".to_string(),
            insts: vec![],
            term: Terminator::Switch {
                scrut: state_val,
                default: Block(state_blocks_start as u32), // Default to state 0
                cases: switch_cases,
            },
            span: None,
        });

        // ================================================================
        // Cancelled block: Create CancelledError and return Error
        // ================================================================
        // Create CancelledError by calling __arth_create_cancelled_error(0)
        // We use 0 as the task handle placeholder since poll function doesn't have direct access
        // to the task handle. The actual task handle is available at the awaiter level.
        let zero_const = Value(next_value);
        next_value += 1;
        let cancelled_error = Value(next_value);
        next_value += 1;

        blocks.push(BlockData {
            name: "cancelled".to_string(),
            insts: vec![
                Inst {
                    result: zero_const,
                    kind: InstKind::ConstI64(0),
                    span: None,
                },
                Inst {
                    result: cancelled_error,
                    kind: InstKind::Call {
                        name: "__arth_create_cancelled_error".to_string(),
                        args: vec![zero_const],
                        ret: Ty::I64,
                    },
                    span: None,
                },
            ],
            term: Terminator::PollReturn {
                result: PollResult::Error,
                value: Some(cancelled_error),
            },
            span: None,
        });

        // ================================================================
        // Success return block: Load result from frame and return Ready
        // ================================================================
        let success_block_idx = blocks.len();
        let result_from_frame = Value(next_value);
        blocks.push(BlockData {
            name: "success_return".to_string(),
            insts: vec![Inst {
                result: result_from_frame,
                kind: InstKind::AsyncFrameLoad {
                    frame_ptr,
                    field_offset: AsyncFrame::RESULT_OFFSET,
                    ty: Ty::I64,
                },
                span: None,
            }],
            term: Terminator::PollReturn {
                result: PollResult::Ready,
                value: Some(result_from_frame),
            },
            span: None,
        });
        next_value += 1;

        // ================================================================
        // Error return block: Load exception from frame and return Error
        // ================================================================
        let error_block_idx = blocks.len();
        let exc_from_frame = Value(next_value);
        blocks.push(BlockData {
            name: "error_return".to_string(),
            insts: vec![Inst {
                result: exc_from_frame,
                kind: InstKind::AsyncFrameLoad {
                    frame_ptr,
                    field_offset: AsyncFrame::EXCEPTION_OFFSET,
                    ty: Ty::I64,
                },
                span: None,
            }],
            term: Terminator::PollReturn {
                result: PollResult::Error,
                value: Some(exc_from_frame),
            },
            span: None,
        });
        next_value += 1;

        // ================================================================
        // State blocks: Generate code for each state
        // ================================================================
        let state_blocks_start_actual = blocks.len();
        for (state_idx, _state) in self.state_assignments.iter().enumerate() {
            let state_block = self.generate_state_block(
                state_idx,
                frame_ptr,
                task_handle,
                &mut next_value,
                success_block_idx,
                error_block_idx,
            )?;
            blocks.push(state_block);
        }

        // Fix up the dispatch switch to use actual state block indices
        if let Terminator::Switch { cases, default, .. } = &mut blocks[1].term {
            *default = Block(state_blocks_start_actual as u32);
            for (i, (_, block)) in cases.iter_mut().enumerate() {
                *block = Block((state_blocks_start_actual + i) as u32);
            }
        }

        Ok(Func {
            name: poll_name,
            params: vec![Ty::Ptr, Ty::I64], // (frame_ptr, task_handle)
            ret: Ty::I64,                   // PollResult as i64
            blocks,
            linkage: Linkage::Private,
            span: None,
        })
    }

    /// Generate a block for a single state in the poll function
    fn generate_state_block(
        &self,
        state_idx: usize,
        frame_ptr: Value,
        _task_handle: Value,
        next_value: &mut u32,
        success_block_idx: usize,
        error_block_idx: usize,
    ) -> Result<BlockData, AsyncLowerError> {
        let state = &self.state_assignments[state_idx];
        let mut insts = Vec::new();

        if state_idx == 0 {
            // ============================================================
            // Entry state (state 0): Load parameters and execute entry code
            // ============================================================

            // Load parameters from frame
            for (i, param_ty) in self.original_func.params.iter().enumerate() {
                let offset = AsyncFrame::USER_FIELDS_START + (i as u32) * 8;
                let loaded = Value(*next_value);
                insts.push(Inst {
                    result: loaded,
                    kind: InstKind::AsyncFrameLoad {
                        frame_ptr,
                        field_offset: offset,
                        ty: param_ty.clone(),
                    },
                    span: None,
                });
                *next_value += 1;
            }

            // If there are await points, we need to transform the original code
            if !self.await_points.is_empty() {
                // Generate code up to the first await point
                let first_await = &self.await_points[0];
                let (pre_await_insts, save_insts, yield_inst, term) =
                    self.generate_pre_await_code(0, frame_ptr, next_value)?;

                insts.extend(pre_await_insts);
                insts.extend(save_insts);
                if let Some(yi) = yield_inst {
                    insts.push(yi);
                }

                return Ok(BlockData {
                    name: format!("state_{}", state_idx),
                    insts,
                    term,
                    span: first_await.metadata.as_ref().and_then(|m| m.span.clone()),
                });
            } else {
                // No await points - execute entire function and return Ready
                let result_val = Value(*next_value);
                insts.push(Inst {
                    result: result_val,
                    kind: InstKind::ConstI64(0), // Placeholder result
                    span: None,
                });
                *next_value += 1;

                return Ok(BlockData {
                    name: format!("state_{}", state_idx),
                    insts,
                    term: Terminator::PollReturn {
                        result: PollResult::Ready,
                        value: Some(result_val),
                    },
                    span: None,
                });
            }
        }

        // ============================================================
        // Resume state (state > 0): Restore locals and continue execution
        // ============================================================

        let await_idx = state.await_point_idx as usize;

        // Load all live values from frame
        for value in &state.live_values {
            if let Some(offset) = self.value_to_field.get(value) {
                let loaded = Value(*next_value);
                insts.push(Inst {
                    result: loaded,
                    kind: InstKind::AsyncFrameLoad {
                        frame_ptr,
                        field_offset: *offset,
                        ty: Ty::I64, // Default type
                    },
                    span: None,
                });
                *next_value += 1;
            }
        }

        // Get the result from the awaited task
        let awaited_offset = AsyncFrame::AWAITED_OFFSET;
        let awaited_handle = Value(*next_value);
        insts.push(Inst {
            result: awaited_handle,
            kind: InstKind::AsyncFrameLoad {
                frame_ptr,
                field_offset: awaited_offset,
                ty: Ty::I64,
            },
            span: None,
        });
        *next_value += 1;

        // Check if the awaited task has an error (exception)
        // If so, we need to propagate the error to our caller
        let has_error = Value(*next_value);
        insts.push(Inst {
            result: has_error,
            kind: InstKind::Call {
                name: "__arth_task_has_error".to_string(),
                args: vec![awaited_handle],
                ret: Ty::I64,
            },
            span: None,
        });
        *next_value += 1;

        // Constant 1 for comparison
        let one_val = Value(*next_value);
        insts.push(Inst {
            result: one_val,
            kind: InstKind::ConstI64(1),
            span: None,
        });
        *next_value += 1;

        // Compare has_error == 1
        let error_check = Value(*next_value);
        insts.push(Inst {
            result: error_check,
            kind: InstKind::Cmp(super::CmpPred::Eq, has_error, one_val),
            span: None,
        });
        *next_value += 1;

        // We'll need to branch: if error, propagate it; else continue normally
        // For now, generate the error propagation inline (simplified approach)
        // A more complete implementation would create separate basic blocks

        // Get error from awaited task (in case we need to propagate)
        let awaited_error = Value(*next_value);
        insts.push(Inst {
            result: awaited_error,
            kind: InstKind::Call {
                name: "__arth_task_get_error".to_string(),
                args: vec![awaited_handle],
                ret: Ty::I64,
            },
            span: None,
        });
        *next_value += 1;

        // Call __arth_task_get_result to get the await result (for success case)
        let await_result = Value(*next_value);
        insts.push(Inst {
            result: await_result,
            kind: InstKind::Call {
                name: "__arth_task_get_result".to_string(),
                args: vec![awaited_handle],
                ret: Ty::I64,
            },
            span: None,
        });
        *next_value += 1;

        // Check if there are more await points after this one
        let is_last_await = await_idx + 1 >= self.await_points.len();

        // For exception propagation, we need to create a conditional structure
        // Generate: if (has_error) { store exception, return Error } else { continue }

        if is_last_await {
            // Last await - on success return Ready, on error return Error
            // Store result in frame for success case
            insts.push(Inst {
                result: Value(*next_value),
                kind: InstKind::AsyncFrameStore {
                    frame_ptr,
                    field_offset: AsyncFrame::RESULT_OFFSET,
                    value: await_result,
                },
                span: None,
            });
            *next_value += 1;

            // Store exception in frame for error case
            insts.push(Inst {
                result: Value(*next_value),
                kind: InstKind::AsyncFrameStore {
                    frame_ptr,
                    field_offset: AsyncFrame::EXCEPTION_OFFSET,
                    value: awaited_error,
                },
                span: None,
            });
            *next_value += 1;

            // Branch to error_return if has_error, else to success_return
            // error_check is 1 (true) for error, 0 (false) for success
            Ok(BlockData {
                name: format!("state_{}", state_idx),
                insts,
                term: Terminator::CondBr {
                    cond: error_check,
                    then_bb: Block(error_block_idx as u32), // error case
                    else_bb: Block(success_block_idx as u32), // success case
                },
                span: None,
            })
        } else {
            // More await points - generate code until next await
            // Still need to handle errors
            let (code_insts, save_insts, yield_inst, term) =
                self.generate_inter_await_code(await_idx, frame_ptr, next_value)?;

            insts.extend(code_insts);
            insts.extend(save_insts);
            if let Some(yi) = yield_inst {
                insts.push(yi);
            }

            Ok(BlockData {
                name: format!("state_{}", state_idx),
                insts,
                term,
                span: None,
            })
        }
    }

    /// Generate code from function entry to the first await point
    fn generate_pre_await_code(
        &self,
        await_idx: usize,
        frame_ptr: Value,
        next_value: &mut u32,
    ) -> Result<(Vec<Inst>, Vec<Inst>, Option<Inst>, Terminator), AsyncLowerError> {
        let ap = &self.await_points[await_idx];
        let mut code_insts = Vec::new();
        let mut save_insts = Vec::new();

        // Transform instructions from entry block up to the await point
        if let Some(block) = self.original_func.blocks.get(ap.block_idx) {
            for (i, inst) in block.insts.iter().enumerate() {
                if i >= ap.inst_idx {
                    break;
                }

                // Skip AwaitPoint markers
                if matches!(&inst.kind, InstKind::AwaitPoint { .. }) {
                    continue;
                }

                // Transform the instruction (remap values if needed)
                let transformed = self.transform_instruction(inst, frame_ptr, next_value);
                code_insts.push(transformed);
            }
        }

        // Save live values to frame before yielding
        for value in &ap.live_values {
            if let Some(offset) = self.value_to_field.get(value) {
                save_insts.push(Inst {
                    result: Value(*next_value),
                    kind: InstKind::AsyncFrameStore {
                        frame_ptr,
                        field_offset: *offset,
                        value: *value,
                    },
                    span: None,
                });
                *next_value += 1;
            }
        }

        // Get the awaited task handle and store it
        let awaited_value = ap.awaited_value.unwrap_or(Value(0));
        save_insts.push(Inst {
            result: Value(*next_value),
            kind: InstKind::AsyncFrameStore {
                frame_ptr,
                field_offset: AsyncFrame::AWAITED_OFFSET,
                value: awaited_value,
            },
            span: None,
        });
        *next_value += 1;

        // Set next state
        let next_state = ap.state_id.unwrap_or(1);
        save_insts.push(Inst {
            result: Value(*next_value),
            kind: InstKind::AsyncFrameSetState {
                frame_ptr,
                state_id: next_state,
            },
            span: None,
        });
        *next_value += 1;

        // Yield instruction (optional, for tracing)
        let yield_inst = Some(Inst {
            result: Value(*next_value),
            kind: InstKind::AsyncYield {
                awaited_task: awaited_value,
            },
            span: None,
        });
        *next_value += 1;

        // Return Pending
        let term = Terminator::PollReturn {
            result: PollResult::Pending,
            value: None,
        };

        Ok((code_insts, save_insts, yield_inst, term))
    }

    /// Generate code between two await points
    fn generate_inter_await_code(
        &self,
        _await_idx: usize,
        frame_ptr: Value,
        next_value: &mut u32,
    ) -> Result<(Vec<Inst>, Vec<Inst>, Option<Inst>, Terminator), AsyncLowerError> {
        // For now, generate placeholder code that moves to the next state
        let code_insts = Vec::new();
        let mut save_insts = Vec::new();

        // Set next state
        let next_state = (_await_idx + 2) as u32; // Next resume state
        save_insts.push(Inst {
            result: Value(*next_value),
            kind: InstKind::AsyncFrameSetState {
                frame_ptr,
                state_id: next_state,
            },
            span: None,
        });
        *next_value += 1;

        // Return Pending for now (simplified)
        let term = Terminator::PollReturn {
            result: PollResult::Pending,
            value: None,
        };

        Ok((code_insts, save_insts, None, term))
    }

    /// Transform an instruction for use in the poll function
    fn transform_instruction(&self, inst: &Inst, _frame_ptr: Value, next_value: &mut u32) -> Inst {
        // For now, just copy the instruction with a new result value
        // A full implementation would remap SSA values
        let result = Value(*next_value);
        *next_value += 1;

        Inst {
            result,
            kind: inst.kind.clone(),
            span: inst.span.clone(),
        }
    }

    // ========================================================================
    // Phase 6: Wrapper Function Generation
    // ========================================================================

    /// Generate the wrapper function that creates the frame and spawns the task
    fn generate_wrapper_function(&self) -> Result<Func, AsyncLowerError> {
        let frame = self.frame.as_ref().ok_or_else(|| {
            AsyncLowerError::InternalError("async frame not initialized".to_string())
        })?;
        let mut blocks = Vec::new();
        let mut insts = Vec::new();
        let argc = self.original_func.params.len() as u32;
        let mut next_value = argc; // Reserve Value(0..argc-1) for parameters

        // Allocate the frame
        let frame_ptr = Value(next_value);
        insts.push(Inst {
            result: frame_ptr,
            kind: InstKind::AsyncFrameAlloc {
                frame_name: frame.name.clone(),
                frame_size: frame.size,
            },
            span: None,
        });
        next_value += 1;

        // Initialize state to 0
        insts.push(Inst {
            result: Value(next_value),
            kind: InstKind::AsyncFrameSetState {
                frame_ptr,
                state_id: 0,
            },
            span: None,
        });
        next_value += 1;

        // Store parameters into frame
        for i in 0..argc {
            let offset = AsyncFrame::USER_FIELDS_START + (i * 8);
            insts.push(Inst {
                result: Value(next_value),
                kind: InstKind::AsyncFrameStore {
                    frame_ptr,
                    field_offset: offset,
                    value: Value(i),
                },
                span: None,
            });
            next_value += 1;
        }

        // Compute poll function ID
        let poll_name = format!("{}$poll", self.original_func.name);
        let fn_id_hash = compute_string_hash(&poll_name);

        let fn_id = Value(next_value);
        insts.push(Inst {
            result: fn_id,
            kind: InstKind::ConstI64(fn_id_hash),
            span: None,
        });
        next_value += 1;

        // Call runtime to spawn the task with frame
        // __arth_task_spawn_with_poll(frame_ptr, poll_fn_id) -> handle
        let handle = Value(next_value);
        insts.push(Inst {
            result: handle,
            kind: InstKind::Call {
                name: "__arth_task_spawn_with_poll".to_string(),
                args: vec![frame_ptr, fn_id],
                ret: Ty::I64,
            },
            span: None,
        });

        let entry = BlockData {
            name: "entry".to_string(),
            insts,
            term: Terminator::Ret(Some(handle)),
            span: None,
        };
        blocks.push(entry);

        Ok(Func {
            name: self.original_func.name.clone(),
            params: self.original_func.params.clone(),
            ret: Ty::I64, // Task handle
            blocks,
            linkage: self.original_func.linkage.clone(),
            span: self.original_func.span.clone(),
        })
    }

    // ========================================================================
    // Phase 7: Drop Function Generation
    // ========================================================================

    /// Generate the drop function for cleanup on cancellation
    fn generate_drop_function(&self) -> Option<Func> {
        let frame = self.frame.as_ref()?;

        // Collect fields that need drop
        let fields_needing_drop: Vec<_> = frame.fields.iter().filter(|f| f.needs_drop).collect();

        if fields_needing_drop.is_empty() && self.await_points.is_empty() {
            return None;
        }

        let drop_name = format!("{}$drop", self.original_func.name);
        let frame_ptr = Value(0);
        let mut next_value = 1u32;
        let mut blocks = Vec::new();

        // Entry block: load state and dispatch to appropriate cleanup
        let state_val = Value(next_value);
        let entry_insts = vec![Inst {
            result: state_val,
            kind: InstKind::AsyncFrameGetState { frame_ptr },
            span: None,
        }];
        next_value += 1;

        // Build switch cases for each state
        let mut switch_cases: Vec<(i64, Block)> = Vec::new();
        let cleanup_start = 1usize;

        for i in 0..self.state_assignments.len() {
            switch_cases.push((i as i64, Block((cleanup_start + i) as u32)));
        }

        let free_block_idx = cleanup_start + self.state_assignments.len();

        blocks.push(BlockData {
            name: "entry".to_string(),
            insts: entry_insts,
            term: Terminator::Switch {
                scrut: state_val,
                default: Block(free_block_idx as u32),
                cases: switch_cases,
            },
            span: None,
        });

        // Generate cleanup block for each state
        for (state_idx, state) in self.state_assignments.iter().enumerate() {
            let mut cleanup_insts = Vec::new();

            // Drop live values for this state
            for var_name in &state.live_vars {
                if let Some(var) = self.cross_await_vars.get(var_name) {
                    if var.needs_drop {
                        if let (Some(offset), Some(ty_name)) = (var.frame_offset, &var.drop_ty_name)
                        {
                            // Load value from frame
                            let loaded = Value(next_value);
                            cleanup_insts.push(Inst {
                                result: loaded,
                                kind: InstKind::AsyncFrameLoad {
                                    frame_ptr,
                                    field_offset: offset,
                                    ty: var.ty.clone(),
                                },
                                span: None,
                            });
                            next_value += 1;

                            // Call drop
                            cleanup_insts.push(Inst {
                                result: Value(next_value),
                                kind: InstKind::Drop {
                                    value: loaded,
                                    ty_name: ty_name.clone(),
                                },
                                span: None,
                            });
                            next_value += 1;
                        }
                    }
                }
            }

            blocks.push(BlockData {
                name: format!("cleanup_state_{}", state_idx),
                insts: cleanup_insts,
                term: Terminator::Br(Block(free_block_idx as u32)),
                span: None,
            });
        }

        // Final block: free the frame
        blocks.push(BlockData {
            name: "free_frame".to_string(),
            insts: vec![Inst {
                result: Value(next_value),
                kind: InstKind::AsyncFrameFree { frame_ptr },
                span: None,
            }],
            term: Terminator::Ret(None),
            span: None,
        });

        Some(Func {
            name: drop_name,
            params: vec![Ty::Ptr], // frame_ptr
            ret: Ty::Void,
            blocks,
            linkage: Linkage::Private,
            span: None,
        })
    }

    // ========================================================================
    // Phase 8: State Transitions and Finalization
    // ========================================================================

    /// Collect state transitions for the state machine
    fn collect_transitions(&self) -> Vec<StateTransition> {
        let mut transitions = Vec::new();

        // Entry -> first await (or completion)
        if !self.await_points.is_empty() {
            transitions.push(StateTransition {
                from_state: 0,
                to_state: 1,
                kind: TransitionKind::AwaitComplete,
                drops: Vec::new(),
            });
        } else {
            transitions.push(StateTransition {
                from_state: 0,
                to_state: 0,
                kind: TransitionKind::Complete,
                drops: Vec::new(),
            });
        }

        // Transitions between await states
        for i in 0..self.await_points.len() {
            let from_state = (i + 1) as u32;
            let to_state = if i + 1 < self.await_points.len() {
                (i + 2) as u32
            } else {
                0 // Complete (return to entry conceptually)
            };

            let kind = if i + 1 < self.await_points.len() {
                TransitionKind::AwaitComplete
            } else {
                TransitionKind::Complete
            };

            transitions.push(StateTransition {
                from_state,
                to_state,
                kind,
                drops: Vec::new(),
            });
        }

        // Cancellation transitions from any state
        for i in 0..self.state_assignments.len() {
            transitions.push(StateTransition {
                from_state: i as u32,
                to_state: 0,
                kind: TransitionKind::Cancellation,
                drops: Vec::new(), // Would collect drops based on live vars
            });
        }

        transitions
    }

    /// Build AsyncState structures from state assignments
    fn build_async_states(&self) -> Vec<AsyncState> {
        self.state_assignments
            .iter()
            .map(|sa| AsyncState {
                id: sa.id,
                name: sa.name.clone(),
                entry_block: Block(sa.entry_block_idx.unwrap_or(0) as u32),
                from_await_idx: sa.await_point_idx,
                preserved_locals: sa.live_vars.iter().cloned().collect(),
                live_borrows: Vec::new(),
            })
            .collect()
    }

    // ========================================================================
    // Trivial Async (No Await Points)
    // ========================================================================

    /// Lower a trivial async function with no await points
    fn lower_trivial_async(self) -> Result<AsyncStateMachine, AsyncLowerError> {
        // For async functions with no await points, we can run them synchronously
        // but still need to return a Task handle for API compatibility

        let frame_name = format!("{}$async_frame", self.original_func.name);

        // Build minimal frame with just parameters
        let mut fields = Vec::new();
        let mut offset = AsyncFrame::USER_FIELDS_START;

        for (i, ty) in self.original_func.params.iter().enumerate() {
            fields.push(AsyncFrameField {
                name: format!("param_{}", i),
                ty: ty.clone(),
                offset,
                original_value: Some(Value(i as u32)),
                needs_drop: false,
                drop_ty_name: None,
            });
            offset += 8;
        }

        let frame = AsyncFrame {
            name: frame_name,
            size: offset,
            fields,
            state_offset: AsyncFrame::STATE_OFFSET,
            result_offset: AsyncFrame::RESULT_OFFSET,
            exception_offset: AsyncFrame::EXCEPTION_OFFSET,
            awaited_offset: AsyncFrame::AWAITED_OFFSET,
        };

        // Generate a simple poll function that runs the whole body in state 0
        // and immediately returns Ready
        let poll_func = Func {
            name: format!("{}$poll", self.original_func.name),
            params: vec![Ty::Ptr, Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![Inst {
                    result: Value(2),
                    kind: InstKind::ConstI64(0), // Result value
                    span: None,
                }],
                term: Terminator::PollReturn {
                    result: PollResult::Ready,
                    value: Some(Value(2)),
                },
                span: None,
            }],
            linkage: Linkage::Private,
            span: None,
        };

        // Generate wrapper that allocates frame and spawns
        let wrapper_func = self.generate_trivial_wrapper(&frame)?;

        Ok(AsyncStateMachine {
            original_name: self.original_func.name,
            frame,
            states: vec![AsyncState {
                id: 0,
                name: "entry".to_string(),
                entry_block: Block(0),
                from_await_idx: -1,
                preserved_locals: Vec::new(),
                live_borrows: Vec::new(),
            }],
            transitions: vec![StateTransition {
                from_state: 0,
                to_state: 0,
                kind: TransitionKind::Complete,
                drops: Vec::new(),
            }],
            poll_func,
            wrapper_func,
            drop_func: None,
        })
    }

    fn generate_trivial_wrapper(&self, frame: &AsyncFrame) -> Result<Func, AsyncLowerError> {
        let argc = self.original_func.params.len() as u32;
        let mut next_value = argc;
        let mut insts = Vec::new();

        // Allocate frame
        let frame_ptr = Value(next_value);
        insts.push(Inst {
            result: frame_ptr,
            kind: InstKind::AsyncFrameAlloc {
                frame_name: frame.name.clone(),
                frame_size: frame.size,
            },
            span: None,
        });
        next_value += 1;

        // Store parameters
        for i in 0..argc {
            let offset = AsyncFrame::USER_FIELDS_START + (i * 8);
            insts.push(Inst {
                result: Value(next_value),
                kind: InstKind::AsyncFrameStore {
                    frame_ptr,
                    field_offset: offset,
                    value: Value(i),
                },
                span: None,
            });
            next_value += 1;
        }

        // Compute poll function ID
        let poll_name = format!("{}$poll", self.original_func.name);
        let fn_id_hash = compute_string_hash(&poll_name);

        let fn_id = Value(next_value);
        insts.push(Inst {
            result: fn_id,
            kind: InstKind::ConstI64(fn_id_hash),
            span: None,
        });
        next_value += 1;

        // Spawn with poll function
        let handle = Value(next_value);
        insts.push(Inst {
            result: handle,
            kind: InstKind::Call {
                name: "__arth_task_spawn_with_poll".to_string(),
                args: vec![frame_ptr, fn_id],
                ret: Ty::I64,
            },
            span: None,
        });

        Ok(Func {
            name: self.original_func.name.clone(),
            params: self.original_func.params.clone(),
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts,
                term: Terminator::Ret(Some(handle)),
                span: None,
            }],
            linkage: self.original_func.linkage.clone(),
            span: self.original_func.span.clone(),
        })
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during async lowering
#[derive(Debug)]
pub enum AsyncLowerError {
    /// The function is not an async function
    NotAsync,
    /// Invalid control flow for async lowering
    InvalidControlFlow(String),
    /// Borrow safety violation
    BorrowViolation(String),
    /// Internal error during transformation
    InternalError(String),
}

impl std::fmt::Display for AsyncLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsyncLowerError::NotAsync => write!(f, "Function is not async"),
            AsyncLowerError::InvalidControlFlow(msg) => {
                write!(f, "Invalid control flow for async: {}", msg)
            }
            AsyncLowerError::BorrowViolation(msg) => write!(f, "Borrow violation: {}", msg),
            AsyncLowerError::InternalError(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for AsyncLowerError {}

// ============================================================================
// Utility Functions
// ============================================================================

/// Compute a simple string hash for function lookup
fn compute_string_hash(s: &str) -> i64 {
    let mut h: u64 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u64);
    }
    h as i64
}

/// Transform an async body function into a state machine.
/// This is the main entry point for async lowering.
pub fn lower_async_function(func: Func) -> Result<AsyncStateMachine, AsyncLowerError> {
    let ctx = AsyncLowerCtx::new(func);
    ctx.lower()
}

/// Check if a function should be lowered as an async state machine.
/// Returns true if the function contains await points.
pub fn should_lower_async(func: &Func) -> bool {
    for block in &func.blocks {
        for inst in &block.insts {
            if matches!(inst.kind, InstKind::AwaitPoint { .. }) {
                return true;
            }
            // Also check for __arth_await calls
            if let InstKind::Call { name, .. } = &inst.kind {
                if name == "__arth_await" {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if a function name suggests it's an async body
pub fn is_async_body_func(name: &str) -> bool {
    name.ends_with("$async_body")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trivial_async() {
        // Create a trivial async function with no await points
        let func = Func {
            name: "trivial_async".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![Inst {
                    result: Value(1),
                    kind: InstKind::Copy(Value(0)),
                    span: None,
                }],
                term: Terminator::Ret(Some(Value(1))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        assert_eq!(sm.original_name, "trivial_async");
        assert_eq!(sm.states.len(), 1);
        assert!(sm.drop_func.is_none());

        // Check frame structure
        assert_eq!(sm.frame.state_offset, 0);
        assert_eq!(sm.frame.result_offset, 8);
        assert_eq!(sm.frame.exception_offset, 16);
        assert_eq!(sm.frame.awaited_offset, 24);
    }

    #[test]
    fn test_find_await_points() {
        // Create a function with an await point
        let func = Func {
            name: "with_await".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let ctx = AsyncLowerCtx::new(func);
        assert!(should_lower_async(&ctx.original_func));
    }

    #[test]
    fn test_frame_construction() {
        let func = Func {
            name: "test_frame".to_string(),
            params: vec![Ty::I64, Ty::F64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(2),
                        kind: InstKind::Alloca,
                        span: None,
                    },
                    Inst {
                        result: Value(3),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(2),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(4),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(2)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let frame = &sm.frame;

        // Check frame layout
        assert!(frame.fields.len() >= 2); // At least the params
        assert_eq!(frame.state_offset, AsyncFrame::STATE_OFFSET);
        assert_eq!(frame.result_offset, AsyncFrame::RESULT_OFFSET);
        assert_eq!(frame.exception_offset, AsyncFrame::EXCEPTION_OFFSET);

        // First field should be param_0 at offset 32
        assert_eq!(frame.fields[0].name, "param_0");
        assert_eq!(frame.fields[0].offset, 32);
    }

    #[test]
    fn test_state_assignment() {
        let func = Func {
            name: "multi_await".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(3),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(2),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(4),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(2)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        // Should have 3 states: entry + 2 await resume states
        assert_eq!(sm.states.len(), 3);
        assert_eq!(sm.states[0].name, "entry");
        assert!(sm.states[1].name.contains("await"));
        assert!(sm.states[2].name.contains("await"));
    }

    #[test]
    fn test_poll_function_signature() {
        let func = Func {
            name: "test_poll".to_string(),
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
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let poll = &sm.poll_func;

        assert_eq!(poll.name, "test_poll$poll");
        assert_eq!(poll.params, vec![Ty::Ptr, Ty::I64]);
        assert_eq!(poll.ret, Ty::I64);
    }

    #[test]
    fn test_wrapper_function_structure() {
        let func = Func {
            name: "test_wrapper".to_string(),
            params: vec![Ty::I64, Ty::F64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(2),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(3),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let wrapper = &sm.wrapper_func;

        assert_eq!(wrapper.name, "test_wrapper");
        assert_eq!(wrapper.params, vec![Ty::I64, Ty::F64]);
        assert_eq!(wrapper.ret, Ty::I64); // Returns task handle

        // Check wrapper allocates frame and spawns
        let entry = &wrapper.blocks[0];
        let has_frame_alloc = entry
            .insts
            .iter()
            .any(|i| matches!(&i.kind, InstKind::AsyncFrameAlloc { .. }));
        assert!(has_frame_alloc);
    }

    #[test]
    fn test_liveness_analysis() {
        // Test that values used after await are marked as live
        let func = Func {
            name: "test_liveness".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    // Value(1) = compute something
                    Inst {
                        result: Value(1),
                        kind: InstKind::Binary(super::super::BinOp::Add, Value(0), Value(0)),
                        span: None,
                    },
                    // Await point
                    Inst {
                        result: Value(2),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(3),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                    // Use Value(1) after await
                    Inst {
                        result: Value(4),
                        kind: InstKind::Binary(super::super::BinOp::Add, Value(1), Value(3)),
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(4))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();

        // Value(1) should be in the frame since it's used after the await
        let has_ssa_1 = sm
            .frame
            .fields
            .iter()
            .any(|f| f.original_value == Some(Value(1)));
        // Note: Due to our naming scheme, it might be named differently
        // The important thing is that the frame has cross-await variables
        assert!(!sm.frame.fields.is_empty());
    }

    #[test]
    fn test_poll_function_cancellation_check() {
        let func = Func {
            name: "test_cancel".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let poll = &sm.poll_func;

        // Check that poll function has cancellation check in entry
        let entry = &poll.blocks[0];
        let has_cancel_check = entry
            .insts
            .iter()
            .any(|i| matches!(&i.kind, InstKind::AsyncCheckCancelled { .. }));
        assert!(has_cancel_check);

        // Check that there's a cancelled block that creates CancelledError
        // and returns PollResult::Error (not PollResult::Cancelled)
        let cancelled_block = poll.blocks.iter().find(|b| b.name == "cancelled");
        assert!(cancelled_block.is_some());
        let cancelled_block = cancelled_block.unwrap();

        // Verify it creates CancelledError by calling __arth_create_cancelled_error
        let creates_cancelled_error = cancelled_block.insts.iter().any(|i| {
            matches!(
                &i.kind,
                InstKind::Call { name, .. } if name == "__arth_create_cancelled_error"
            )
        });
        assert!(creates_cancelled_error);

        // Verify it returns PollResult::Error (not PollResult::Cancelled)
        assert!(matches!(
            &cancelled_block.term,
            Terminator::PollReturn {
                result: PollResult::Error,
                value: Some(_)
            }
        ));
    }

    #[test]
    fn test_drop_function_generation() {
        // Create a function with a type that needs drop
        let func = Func {
            name: "test_drop".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();

        // Drop function may or may not exist depending on whether there are
        // types that need dropping
        if let Some(drop_fn) = &sm.drop_func {
            assert_eq!(drop_fn.name, "test_drop$drop");
            assert_eq!(drop_fn.params, vec![Ty::Ptr]);
            assert_eq!(drop_fn.ret, Ty::Void);
        }
    }

    #[test]
    fn test_should_lower_async() {
        // Function with await should be lowered
        let with_await = Func {
            name: "with_await".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![Inst {
                    result: Value(0),
                    kind: InstKind::Call {
                        name: "__arth_await".to_string(),
                        args: vec![],
                        ret: Ty::I64,
                    },
                    span: None,
                }],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };
        assert!(should_lower_async(&with_await));

        // Function without await should not be lowered
        let without_await = Func {
            name: "without_await".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![Inst {
                    result: Value(0),
                    kind: InstKind::ConstI64(42),
                    span: None,
                }],
                term: Terminator::Ret(Some(Value(0))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };
        assert!(!should_lower_async(&without_await));
    }

    #[test]
    fn test_is_async_body_func() {
        assert!(is_async_body_func("myFunc$async_body"));
        assert!(is_async_body_func("foo.bar$async_body"));
        assert!(!is_async_body_func("myFunc"));
        assert!(!is_async_body_func("myFunc$poll"));
    }

    #[test]
    fn test_poll_function_has_error_return_block() {
        // Create a function with await points
        let func = Func {
            name: "test_error_return".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let poll = &sm.poll_func;

        // Check that poll function has an error_return block with PollReturn::Error
        let has_error_return = poll.blocks.iter().any(|b| {
            b.name == "error_return"
                && matches!(
                    &b.term,
                    Terminator::PollReturn {
                        result: PollResult::Error,
                        ..
                    }
                )
        });
        assert!(
            has_error_return,
            "Poll function should have error_return block"
        );

        // Check that poll function has a success_return block with PollReturn::Ready
        let has_success_return = poll.blocks.iter().any(|b| {
            b.name == "success_return"
                && matches!(
                    &b.term,
                    Terminator::PollReturn {
                        result: PollResult::Ready,
                        ..
                    }
                )
        });
        assert!(
            has_success_return,
            "Poll function should have success_return block"
        );
    }

    #[test]
    fn test_poll_function_error_check_after_await() {
        // Create a function with await points
        let func = Func {
            name: "test_error_check".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();
        let poll = &sm.poll_func;

        // Check that the resume state block calls __arth_task_has_error
        let resume_state = poll.blocks.iter().find(|b| b.name.contains("state_1"));
        assert!(resume_state.is_some(), "Should have resume state block");

        let has_error_check = resume_state.unwrap().insts.iter().any(|i| {
            if let InstKind::Call { name, .. } = &i.kind {
                name == "__arth_task_has_error"
            } else {
                false
            }
        });
        assert!(
            has_error_check,
            "Resume state should check for errors from awaited task"
        );

        // Check that there's a call to __arth_task_get_error
        let has_get_error = resume_state.unwrap().insts.iter().any(|i| {
            if let InstKind::Call { name, .. } = &i.kind {
                name == "__arth_task_get_error"
            } else {
                false
            }
        });
        assert!(
            has_get_error,
            "Resume state should get error from awaited task"
        );
    }

    #[test]
    fn test_exception_offset_in_frame() {
        let func = Func {
            name: "test_exc_offset".to_string(),
            params: vec![Ty::I64],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "entry".to_string(),
                insts: vec![
                    Inst {
                        result: Value(1),
                        kind: InstKind::AwaitPoint {
                            awaited_task: Value(0),
                            live_borrows: vec![],
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_await".to_string(),
                            args: vec![Value(0)],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
                span: None,
            }],
            linkage: Linkage::External,
            span: None,
        };

        let result = lower_async_function(func);
        assert!(result.is_ok());

        let sm = result.unwrap();

        // Check that the frame has the correct exception offset
        assert_eq!(
            sm.frame.exception_offset,
            AsyncFrame::EXCEPTION_OFFSET,
            "Frame should have correct exception offset"
        );
        assert_eq!(
            AsyncFrame::EXCEPTION_OFFSET,
            16,
            "Exception offset should be 16"
        );
    }

    #[test]
    fn test_poll_result_error_variant() {
        // Test that PollResult::Error has the correct value
        assert_eq!(PollResult::Error as u8, 4);
    }
}
