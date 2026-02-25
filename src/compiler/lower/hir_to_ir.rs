use std::collections::{HashMap, HashSet};

use crate::compiler::hir::core::Span as HirSpan;
use crate::compiler::hir::{
    HirAssignOp, HirBinOp, HirBlock, HirExpr, HirFunc, HirPattern, HirStmt, HirType, HirUnOp,
};
use crate::compiler::ir;
use std::collections::BTreeMap;

#[allow(dead_code)]
#[derive(Default)]
struct VarInfo {
    index: usize,
}

/// Allocation strategy for a variable (from escape analysis)
#[derive(Clone, Debug, PartialEq, Eq)]
enum AllocStrategy {
    /// Stack allocation - value doesn't escape, deterministic drop
    Stack,
    /// Reference counted - value escapes, needs RC for cleanup
    RefCounted,
    /// Unique ownership - single owner, move semantics
    UniqueOwned,
    /// Region allocation - value lives for a known region (loop body)
    Region(u32),
}

impl Default for AllocStrategy {
    fn default() -> Self {
        AllocStrategy::Stack
    }
}

/// Information about a variable that needs drop
#[derive(Clone, Debug)]
struct DropInfo {
    /// Variable name
    #[allow(dead_code)]
    name: String,
    /// IR value (slot) holding this variable
    slot: ir::Value,
    /// Qualified type name for drop resolution (e.g., "pkg.TypeName")
    ty_name: String,
    /// Declaration order (for reverse-order drops)
    decl_order: u32,
    /// Allocation strategy (from escape analysis)
    alloc_strategy: AllocStrategy,
    /// Move state at end of scope - determines drop strategy
    move_state: crate::compiler::typeck::escape_results::MoveState,
    /// Drop flag slot (allocated for ConditionallyMoved values)
    drop_flag_slot: Option<ir::Value>,
    /// Per-field drop info for partial moves
    field_drop_info:
        std::collections::HashMap<String, crate::compiler::typeck::escape_results::FieldDropInfo>,
    /// Whether this type has an explicit deinit function
    has_explicit_deinit: bool,
}

struct LowerState<'a> {
    blocks: Vec<ir::BlockData>,
    cur: usize,
    next_val: u32,
    #[allow(dead_code)]
    next_block_seq: u32,
    // locals: name -> stack slot (ptr value)
    locals: HashMap<String, ir::Value>,
    // which locals are declared as `shared`
    shared_locals: HashSet<String>,
    // struct field storage: (object-name, field) -> stack slot (ptr value)
    field_slots: HashMap<(String, String), ir::Value>,
    // names of fields declared `shared` in any provider
    shared_field_names: HashSet<String>,
    // try-unwind target (landing pad block index)
    unwind_target: Option<usize>,
    // loop stack: (break_target, continue_target, drop_scope_depth, region_id)
    loops: Vec<(usize, usize, usize, u32)>,
    // optional labels for each loop on the stack (aligned with `loops`)
    loop_labels: Vec<Option<String>>,
    // a pending label applied to the next loop lowered (set by HirStmt::Labeled)
    pending_loop_label: Option<String>,
    // variable name -> info (kept for potential SSA later)
    #[allow(dead_code)]
    vars: HashMap<String, VarInfo>,
    // var index -> set of blocks that define it (for SSA phi placement later)
    #[allow(dead_code)]
    defs: HashMap<usize, HashSet<usize>>,
    // interned string literals for this lowering unit
    strings: Vec<String>,
    // enum name -> (variant name -> tag)
    enum_tags: Option<BTreeMap<String, BTreeMap<String, i64>>>,
    // lambda functions accumulated during lowering
    lambda_funcs: Vec<ir::Func>,
    // type aliases visible to this lowering unit (for lambda type mapping)
    type_aliases: BTreeMap<String, Vec<String>>,
    // Drop/RAII: stack of drop scopes, each containing variables that need drops
    // Inner Vec is the current scope's variables needing drop
    drop_scopes: Vec<Vec<DropInfo>>,
    // Counter for declaration order
    decl_order_counter: u32,
    // Types that need drop: (pkg, type_name) -> deinit module name
    types_needing_drop: BTreeMap<String, String>,
    // Escape analysis results from type checking (optional)
    escape_info: Option<&'a crate::compiler::typeck::FunctionEscapeInfo>,
    // Active loop regions stack: region_id -> Vec<(slot, ty_name)> for deinit on exit
    // Each entry is a region_id and the variables allocated in that region
    loop_regions: Vec<(u32, Vec<(ir::Value, String)>)>,
    // Next region ID for lowering (monotonically increasing)
    next_region_id: u32,
    // Structs with @derive(JsonCodec): struct_name -> JsonCodecMeta
    json_codec_structs: BTreeMap<String, JsonCodecMeta>,
    // Names of all providers visible to this lowering unit
    provider_names: HashSet<String>,
    // Local variable types: name -> HirType (for string coercion detection)
    local_types: HashMap<String, HirType>,
    // Struct field types: (struct_name, field_name) -> field_type_name
    // Used to determine if field access results in a provider type
    struct_field_types: HashMap<(String, String), String>,
    // Extern functions (FFI): name -> ABI signature for lowering extern calls.
    extern_funcs: HashMap<String, ExternSig>,
    // Stack of pending finally blocks - when return is inside try-finally,
    // these blocks must be executed before the actual return
    finally_scopes: Vec<HirBlock>,
    // Whether to emit native struct/enum instructions (for LLVM backend)
    // When false, emits runtime calls (for VM backend)
    use_native_structs: bool,
    // Struct definitions: struct_name -> Vec<(field_name, field_type)>
    // Used for native struct field index lookup
    struct_defs: HashMap<String, Vec<(String, String)>>,
}

impl Default for LowerState<'_> {
    fn default() -> Self {
        LowerState {
            blocks: Vec::new(),
            cur: 0,
            next_val: 0,
            next_block_seq: 0,
            locals: HashMap::new(),
            shared_locals: HashSet::new(),
            field_slots: HashMap::new(),
            shared_field_names: HashSet::new(),
            unwind_target: None,
            loops: Vec::new(),
            loop_labels: Vec::new(),
            pending_loop_label: None,
            vars: HashMap::new(),
            defs: HashMap::new(),
            strings: Vec::new(),
            enum_tags: None,
            lambda_funcs: Vec::new(),
            type_aliases: BTreeMap::new(),
            drop_scopes: Vec::new(),
            decl_order_counter: 0,
            types_needing_drop: BTreeMap::new(),
            escape_info: None,
            loop_regions: Vec::new(),
            next_region_id: 0,
            json_codec_structs: BTreeMap::new(),
            provider_names: HashSet::new(),
            local_types: HashMap::new(),
            struct_field_types: HashMap::new(),
            extern_funcs: HashMap::new(),
            finally_scopes: Vec::new(),
            use_native_structs: false,
            struct_defs: HashMap::new(),
        }
    }
}

impl LowerState<'_> {
    /// Look up the field index for a struct field.
    /// Returns Some(index) if found, None otherwise.
    fn get_field_index(&self, struct_name: &str, field_name: &str) -> Option<u32> {
        self.struct_defs.get(struct_name).and_then(|fields| {
            fields
                .iter()
                .position(|(name, _)| name == field_name)
                .map(|idx| idx as u32)
        })
    }

    fn new_block(&mut self, base: &str, span: Option<ir::Span>) -> usize {
        let idx = self.blocks.len();
        let name = format!("{}_{idx}", base);
        self.blocks.push(ir::BlockData {
            name,
            insts: vec![],
            term: ir::Terminator::Ret(None),
            span,
        });
        idx
    }

    fn set_cur(&mut self, idx: usize) {
        self.cur = idx;
    }

    fn fresh_value(&mut self) -> ir::Value {
        let v = ir::Value(self.next_val);
        self.next_val += 1;
        v
    }

    fn emit(&mut self, kind: ir::InstKind, span: Option<ir::Span>) -> ir::Value {
        let res = self.fresh_value();
        self.blocks[self.cur].insts.push(ir::Inst {
            result: res,
            kind,
            span,
        });
        res
    }

    fn set_term(&mut self, term: ir::Terminator) {
        self.blocks[self.cur].term = term;
    }

    /// Returns true if the current block has an explicit terminator (not just the default Ret(None)).
    fn is_block_terminated(&self) -> bool {
        let cur_term = &self.blocks[self.cur].term;
        matches!(
            cur_term,
            ir::Terminator::Ret(Some(_))
                | ir::Terminator::Throw(_)
                | ir::Terminator::Panic(_)
                | ir::Terminator::Unreachable
                | ir::Terminator::Br(_)
                | ir::Terminator::CondBr { .. }
        )
    }

    /// Set terminator only if the block hasn't already been terminated by
    /// a return, throw, panic, branch (break/continue), or unreachable.
    /// This prevents overwriting explicit control flow in if/else branches.
    fn set_term_if_fallthrough(&mut self, term: ir::Terminator) {
        let cur_term = &self.blocks[self.cur].term;
        // Only set if current terminator is the default Ret(None) placeholder.
        // All other terminators are explicit and should not be overwritten.
        let is_explicit_terminator = matches!(
            cur_term,
            ir::Terminator::Ret(Some(_))
                | ir::Terminator::Throw(_)
                | ir::Terminator::Panic(_)
                | ir::Terminator::Unreachable
                | ir::Terminator::Br(_)
        );
        if !is_explicit_terminator {
            self.blocks[self.cur].term = term;
        }
    }

    /// Determine if an expression evaluates to a provider type.
    /// Returns Some(provider_name) if the expression is a provider type, None otherwise.
    /// This handles:
    /// - Simple identifiers: look up type in local_types
    /// - Field access (Member): look up field type in struct_field_types
    fn get_expr_provider_type(&self, expr: &HirExpr) -> Option<String> {
        match expr {
            HirExpr::Ident { name, .. } => {
                // For identifiers, check local_types
                if let Some(var_ty) = self.local_types.get(name) {
                    let type_name = match var_ty {
                        HirType::Name { path } => path.last().cloned(),
                        HirType::Generic { path, .. } => path.last().cloned(),
                        _ => None,
                    };
                    if let Some(ref tname) = type_name {
                        if self.provider_names.contains(tname) {
                            return Some(tname.clone());
                        }
                    }
                }
                None
            }
            HirExpr::Member { object, member, .. } => {
                // For field access, first determine the object's type, then look up the field type
                if let Some(obj_type) = self.get_expr_type_name(object) {
                    if let Some(field_type) =
                        self.struct_field_types.get(&(obj_type, member.clone()))
                    {
                        if self.provider_names.contains(field_type) {
                            return Some(field_type.clone());
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Get the type name of an expression (for struct types).
    /// Returns the simple type name (e.g., "Providers", "Counter").
    fn get_expr_type_name(&self, expr: &HirExpr) -> Option<String> {
        match expr {
            HirExpr::Ident { name, .. } => {
                if let Some(var_ty) = self.local_types.get(name) {
                    match var_ty {
                        HirType::Name { path } => path.last().cloned(),
                        HirType::Generic { path, .. } => path.last().cloned(),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            HirExpr::Member { object, member, .. } => {
                // Get the object's type name, then look up the field's type
                if let Some(obj_type) = self.get_expr_type_name(object) {
                    self.struct_field_types
                        .get(&(obj_type, member.clone()))
                        .cloned()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check if an expression is of String type.
    /// Used to emit StrLen for `.length` on strings instead of struct field access.
    fn is_expr_string_type(&self, expr: &HirExpr) -> bool {
        match self.get_expr_type_name(expr) {
            Some(ty_name) => ty_name == "String" || ty_name == "string",
            None => false,
        }
    }

    #[allow(dead_code)]
    fn def_var_in_block(&mut self, name: &str, b: usize) {
        let vindex = self.ensure_var(name);
        self.defs.entry(vindex).or_default().insert(b);
    }

    #[allow(dead_code)]
    fn ensure_var(&mut self, name: &str) -> usize {
        if let Some(v) = self.vars.get(name) {
            return v.index;
        }
        let idx = self.vars.len();
        self.vars.insert(name.to_string(), VarInfo { index: idx });
        idx
    }

    // --- Drop/RAII scope management ---

    /// Push a new drop scope (entering a block/loop/etc)
    fn push_drop_scope(&mut self) {
        self.drop_scopes.push(Vec::new());
    }

    /// Pop a drop scope and emit drops for all variables in it (in reverse declaration order)
    fn pop_drop_scope(&mut self, span: Option<ir::Span>) {
        if let Some(mut scope) = self.drop_scopes.pop() {
            // Sort by decl_order descending (reverse declaration order)
            scope.sort_by(|a, b| b.decl_order.cmp(&a.decl_order));
            for drop_info in scope {
                self.emit_drop_for(&drop_info, span.clone());
            }
        }
    }

    /// Register a variable that needs drop in the current scope
    fn register_drop(&mut self, name: &str, slot: ir::Value, ty_name: &str) {
        // Get allocation strategy from escape analysis if available
        let alloc_strategy = self.get_alloc_strategy_for(name);
        self.register_drop_with_strategy(name, slot, ty_name, alloc_strategy);
    }

    /// Register an FFI-owned value for cleanup.
    /// This forces `has_explicit_deinit = true` to ensure a Drop instruction is emitted
    /// even though the FFI type doesn't have a real deinit function.
    fn register_ffi_owned_drop(&mut self, name: &str, slot: ir::Value, ty_name: &str) {
        use crate::compiler::typeck::escape_results::MoveState;

        let decl_order = self.decl_order_counter;
        self.decl_order_counter += 1;

        let drop_info = DropInfo {
            name: name.to_string(),
            slot,
            ty_name: ty_name.to_string(),
            decl_order,
            alloc_strategy: AllocStrategy::Stack,
            move_state: MoveState::Available,
            drop_flag_slot: None,
            field_drop_info: HashMap::new(),
            has_explicit_deinit: true, // Force true to ensure Drop is emitted
        };

        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(drop_info);
        }
    }

    /// Get allocation strategy for a variable from escape analysis results
    fn get_alloc_strategy_for(&self, name: &str) -> AllocStrategy {
        if let Some(escape_info) = &self.escape_info {
            match escape_info.get_alloc_strategy(name) {
                crate::compiler::typeck::AllocStrategy::Stack => AllocStrategy::Stack,
                crate::compiler::typeck::AllocStrategy::RefCounted => AllocStrategy::RefCounted,
                crate::compiler::typeck::AllocStrategy::UniqueOwned => AllocStrategy::UniqueOwned,
                crate::compiler::typeck::AllocStrategy::Region(id) => AllocStrategy::Region(id),
            }
        } else {
            AllocStrategy::Stack // Default to stack when no escape info available
        }
    }

    /// Determine whether a variable should actually be dropped, based on escape analysis.
    /// Defaults to true when no escape info is available for this function/local.
    fn should_drop_var(&self, name: &str) -> bool {
        if let Some(escape_info) = &self.escape_info {
            if let Some(info) = escape_info.get_local(name) {
                return info.needs_drop;
            }
        }
        true
    }

    /// Get move state for a variable from escape analysis results
    fn get_move_state_for(&self, name: &str) -> crate::compiler::typeck::escape_results::MoveState {
        use crate::compiler::typeck::escape_results::MoveState;
        if let Some(escape_info) = &self.escape_info {
            if let Some(info) = escape_info.get_local(name) {
                return info.move_state.clone();
            }
        }
        MoveState::Available // Default to Available when no escape info
    }

    /// Get field drop info for a variable from escape analysis results
    fn get_field_drop_info_for(
        &self,
        name: &str,
    ) -> std::collections::HashMap<String, crate::compiler::typeck::escape_results::FieldDropInfo>
    {
        if let Some(escape_info) = &self.escape_info {
            if let Some(info) = escape_info.get_local(name) {
                return info.field_drop_info.clone();
            }
        }
        std::collections::HashMap::new()
    }

    /// Check if a variable's type has an explicit deinit function
    fn has_explicit_deinit_for(&self, name: &str) -> bool {
        if let Some(escape_info) = &self.escape_info {
            if let Some(info) = escape_info.get_local(name) {
                return info.has_explicit_deinit;
            }
        }
        false
    }

    /// Register a variable that needs drop with a specific allocation strategy
    fn register_drop_with_strategy(
        &mut self,
        name: &str,
        slot: ir::Value,
        ty_name: &str,
        alloc_strategy: AllocStrategy,
    ) {
        use crate::compiler::typeck::escape_results::MoveState;

        // For region-allocated variables, register with the region for bulk cleanup
        if let AllocStrategy::Region(region_id) = &alloc_strategy {
            self.register_region_variable(*region_id, slot, ty_name);
            // Don't add to regular drop scopes - region exit handles cleanup
            return;
        }

        let decl_order = self.decl_order_counter;
        self.decl_order_counter += 1;

        // Get move state from escape analysis
        let move_state = self.get_move_state_for(name);

        // Get field drop info and explicit deinit status
        let field_drop_info = self.get_field_drop_info_for(name);
        let has_explicit_deinit = self.has_explicit_deinit_for(name);

        // Allocate drop flag if conditionally moved
        let drop_flag_slot = match &move_state {
            MoveState::ConditionallyMoved => {
                // Allocate a slot for the drop flag and initialize to 0 (not moved)
                let flag_slot = self.emit(ir::InstKind::Alloca, None);
                let zero = self.emit(ir::InstKind::ConstI64(0), None);
                self.emit(ir::InstKind::Store(flag_slot, zero), None);
                Some(flag_slot)
            }
            _ => None,
        };

        if let Some(scope) = self.drop_scopes.last_mut() {
            scope.push(DropInfo {
                name: name.to_string(),
                slot,
                ty_name: ty_name.to_string(),
                decl_order,
                alloc_strategy,
                move_state,
                drop_flag_slot,
                field_drop_info,
                has_explicit_deinit,
            });
        }
    }

    /// Check if a type needs drop based on its path
    fn needs_drop_ty(&self, ty: &HirType) -> Option<String> {
        match ty {
            HirType::Name { path } | HirType::Generic { path, .. } => {
                if path.is_empty() {
                    return None;
                }
                let type_name = path.last().unwrap().clone();
                let pkg = if path.len() > 1 {
                    path[..path.len() - 1].join(".")
                } else {
                    String::new()
                };
                let key = format!("{}.{}", pkg, type_name);
                if self.types_needing_drop.contains_key(&key) {
                    Some(key)
                } else {
                    // Also try without package prefix for local types
                    if self.types_needing_drop.contains_key(&type_name) {
                        Some(type_name)
                    } else {
                        None
                    }
                }
            }
            // Type parameters need drop determination at instantiation time
            HirType::TypeParam { .. } => None,
        }
    }

    /// Emit a drop instruction for a single variable
    fn emit_drop_for(&mut self, drop_info: &DropInfo, span: Option<ir::Span>) {
        use crate::compiler::typeck::escape_results::MoveState;

        // Check move state - fully moved values don't need dropping
        match &drop_info.move_state {
            MoveState::FullyMoved => {
                // Value was definitely moved on all paths; no drop needed
                return;
            }
            MoveState::PartiallyMoved(moved_fields) => {
                // Per-field drops for partial moves
                // Types with explicit deinit should be rejected at typecheck;
                // for types without explicit deinit, drop each unmoved field
                if drop_info.has_explicit_deinit {
                    // Should have been rejected at typecheck
                    return;
                }

                // Emit drops for unmoved fields that need dropping
                for (field_name, field_info) in &drop_info.field_drop_info {
                    if field_info.needs_drop && !moved_fields.contains(field_name) {
                        if let Some(drop_ty_name) = &field_info.drop_ty_name {
                            // Load the struct value from slot
                            let struct_val =
                                self.emit(ir::InstKind::Load(drop_info.slot), span.clone());
                            // Get field value (this is a simplified representation;
                            // actual implementation would need field offset info)
                            // For now, emit a FieldDrop instruction that the codegen will handle
                            self.emit(
                                ir::InstKind::FieldDrop {
                                    value: struct_val,
                                    field_name: field_name.clone(),
                                    ty_name: drop_ty_name.clone(),
                                },
                                span.clone(),
                            );
                        }
                    }
                }
                return;
            }
            MoveState::Available | MoveState::ConditionallyMoved => {
                // Continue with normal drop handling
            }
        }

        // Load the value from the slot
        let val = self.emit(ir::InstKind::Load(drop_info.slot), span.clone());

        // Handle conditional drops (value may or may not be moved)
        if let (MoveState::ConditionallyMoved, Some(flag_slot)) =
            (&drop_info.move_state, drop_info.drop_flag_slot)
        {
            // Load the drop flag
            let flag = self.emit(ir::InstKind::Load(flag_slot), span.clone());

            if drop_info.has_explicit_deinit {
                // Type has explicit deinit - emit conditional drop
                self.emit(
                    ir::InstKind::CondDrop {
                        value: val,
                        flag,
                        ty_name: drop_info.ty_name.clone(),
                    },
                    span,
                );
            } else if !drop_info.field_drop_info.is_empty() {
                // Type has synthetic deinit - emit conditional field drops.
                // This currently uses one condition per field with the same move flag.
                let mut fields_to_drop: Vec<_> = drop_info
                    .field_drop_info
                    .iter()
                    .filter(|(_, info)| info.needs_drop && info.drop_ty_name.is_some())
                    .collect();
                fields_to_drop.reverse();

                for (field_name, field_info) in fields_to_drop {
                    if let Some(ref drop_ty_name) = field_info.drop_ty_name {
                        // Create conditional field drop block pattern
                        let skip_label = format!("skip_field_drop_{}", self.next_val);
                        let drop_label = format!("do_field_drop_{}", self.next_val);
                        let cont_label = format!("cont_field_drop_{}", self.next_val);

                        // Jump to skip if flag is true (moved)
                        let skip_block_idx = self.new_block(&skip_label, span.clone());
                        let drop_block_idx = self.new_block(&drop_label, span.clone());
                        let cont_block_idx = self.new_block(&cont_label, span.clone());

                        // Branch based on flag
                        self.blocks[self.cur].term = ir::Terminator::CondBr {
                            cond: flag,
                            then_bb: ir::Block(skip_block_idx as u32), // If moved (flag=true), skip drop
                            else_bb: ir::Block(drop_block_idx as u32), // If not moved (flag=false), do drop
                        };

                        // Skip block - just continue
                        self.cur = skip_block_idx;
                        self.blocks[self.cur].term =
                            ir::Terminator::Br(ir::Block(cont_block_idx as u32));

                        // Drop block - emit field drop
                        self.cur = drop_block_idx;
                        self.emit(
                            ir::InstKind::FieldDrop {
                                value: val,
                                field_name: field_name.clone(),
                                ty_name: drop_ty_name.clone(),
                            },
                            span.clone(),
                        );
                        self.blocks[self.cur].term =
                            ir::Terminator::Br(ir::Block(cont_block_idx as u32));

                        // Continue from cont block
                        self.cur = cont_block_idx;
                    }
                }
            }
            return;
        }

        // Handle different allocation strategies for unconditional drops
        match drop_info.alloc_strategy {
            AllocStrategy::Stack | AllocStrategy::UniqueOwned => {
                // Stack and unique owned values: direct drop with deterministic cleanup
                if drop_info.has_explicit_deinit {
                    // Type has explicit deinit function - call it
                    self.emit(
                        ir::InstKind::Drop {
                            value: val,
                            ty_name: drop_info.ty_name.clone(),
                        },
                        span,
                    );
                } else if !drop_info.field_drop_info.is_empty() {
                    // Type has synthetic deinit (droppable fields but no explicit deinit)
                    // Emit FieldDrop for each droppable field in reverse order
                    let mut fields_to_drop: Vec<_> = drop_info
                        .field_drop_info
                        .iter()
                        .filter(|(_, info)| info.needs_drop && info.drop_ty_name.is_some())
                        .collect();
                    // Reverse for proper drop order (last declared field dropped first)
                    fields_to_drop.reverse();

                    for (field_name, field_info) in fields_to_drop {
                        if let Some(ref drop_ty_name) = field_info.drop_ty_name {
                            self.emit(
                                ir::InstKind::FieldDrop {
                                    value: val,
                                    field_name: field_name.clone(),
                                    ty_name: drop_ty_name.clone(),
                                },
                                span.clone(),
                            );
                        }
                    }
                }
                // If neither explicit nor synthetic deinit, no drop needed
            }
            AllocStrategy::RefCounted => {
                // Reference counted values: decrement ref count and drop if zero
                // For now, we emit a regular drop - runtime will handle RC decrement
                // (future enhancement: dedicated IR op for explicit RC decref)
                if drop_info.has_explicit_deinit {
                    self.emit(
                        ir::InstKind::Drop {
                            value: val,
                            ty_name: drop_info.ty_name.clone(),
                        },
                        span,
                    );
                } else if !drop_info.field_drop_info.is_empty() {
                    // Synthetic deinit for RC values
                    let mut fields_to_drop: Vec<_> = drop_info
                        .field_drop_info
                        .iter()
                        .filter(|(_, info)| info.needs_drop && info.drop_ty_name.is_some())
                        .collect();
                    fields_to_drop.reverse();

                    for (field_name, field_info) in fields_to_drop {
                        if let Some(ref drop_ty_name) = field_info.drop_ty_name {
                            self.emit(
                                ir::InstKind::FieldDrop {
                                    value: val,
                                    field_name: field_name.clone(),
                                    ty_name: drop_ty_name.clone(),
                                },
                                span.clone(),
                            );
                        }
                    }
                }
            }
            AllocStrategy::Region(_id) => {
                // Region-allocated values are cleaned up at RegionExit; no per-scope drop here.
            }
        }
    }

    /// Emit drops for all scopes from current to target depth (for break/continue/return)
    /// This emits drops in reverse order for all scopes from the innermost to the target.
    fn emit_drops_to_depth(&mut self, target_depth: usize, span: Option<ir::Span>) {
        // Collect all drop infos from scopes that will be exited
        let mut all_drops: Vec<DropInfo> = Vec::new();
        for scope_idx in (target_depth..self.drop_scopes.len()).rev() {
            if let Some(scope) = self.drop_scopes.get(scope_idx) {
                all_drops.extend(scope.iter().cloned());
            }
        }
        // Sort by decl_order descending (reverse declaration order)
        all_drops.sort_by(|a, b| b.decl_order.cmp(&a.decl_order));
        for drop_info in &all_drops {
            self.emit_drop_for(drop_info, span.clone());
        }
    }

    /// Emit drops for all active scopes (for return statements)
    fn emit_all_drops(&mut self, span: Option<ir::Span>) {
        self.emit_drops_to_depth(0, span);
    }

    /// Get the depth of the current drop scope stack (for loop targets)
    fn drop_scope_depth(&self) -> usize {
        self.drop_scopes.len()
    }

    /// Enter a loop region - emits RegionEnter and returns the region_id
    fn enter_loop_region(&mut self) -> u32 {
        let region_id = self.next_region_id;
        self.next_region_id += 1;
        self.loop_regions.push((region_id, Vec::new()));
        // Emit RegionEnter instruction (result value is unused)
        let _ = self.emit(ir::InstKind::RegionEnter { region_id }, None);
        region_id
    }

    /// Exit a loop region - emits RegionExit with deinit calls
    fn exit_loop_region(&mut self, expected_id: u32) {
        if let Some((id, deinit_values)) = self.loop_regions.pop() {
            if id == expected_id {
                // Build deinit_calls list
                let deinit_calls: Vec<(ir::Value, String)> = deinit_values;
                // Emit RegionExit instruction with deinit info (result value is unused)
                let _ = self.emit(
                    ir::InstKind::RegionExit {
                        region_id: id,
                        deinit_calls,
                    },
                    None,
                );
            } else {
                // Put it back if not matching
                self.loop_regions.push((id, Vec::new()));
            }
        }
    }

    /// Exit all active loop regions from innermost to outermost.
    ///
    /// This is used for non-local control flow (return/throw/panic) so region
    /// allocations are released on both normal and exceptional exits.
    fn exit_all_loop_regions(&mut self) {
        while let Some((id, deinit_values)) = self.loop_regions.pop() {
            let _ = self.emit(
                ir::InstKind::RegionExit {
                    region_id: id,
                    deinit_calls: deinit_values,
                },
                None,
            );
        }
    }

    /// Register a region-allocated variable for deinit on region exit
    fn register_region_variable(&mut self, region_id: u32, slot: ir::Value, ty_name: &str) {
        for (id, vars) in &mut self.loop_regions {
            if *id == region_id {
                vars.push((slot, ty_name.to_string()));
                return;
            }
        }
    }

    /// Get the current active loop region ID (if any)
    fn current_loop_region(&self) -> Option<u32> {
        self.loop_regions.last().map(|(id, _)| *id)
    }
}

// Context for lowering enums: map of enum name to its variants (name -> integer tag)
pub struct EnumLowerContext {
    pub tags: BTreeMap<String, BTreeMap<String, i64>>,
    // Names of fields that are declared `shared` inside provider declarations.
    pub shared_field_names: HashSet<String>,
    // Type aliases visible to this lowering unit: alias name -> fully qualified target path
    pub type_aliases: BTreeMap<String, Vec<String>>,
    // Types that need drop: qualified type name -> deinit module name
    pub types_needing_drop: BTreeMap<String, String>,
    /// Structs with @derive(JsonCodec): struct_name -> JsonCodecMeta
    /// Each entry contains: field metadata string for serialization
    pub json_codec_structs: BTreeMap<String, JsonCodecMeta>,
    /// Names of all providers visible to this lowering unit
    pub provider_names: HashSet<String>,
    /// Extern functions (FFI): name -> ABI signature for lowering extern calls.
    pub extern_funcs: HashMap<String, ExternSig>,
    /// Struct field types: (struct_name, field_name) -> field_type_name
    /// Used to determine if field access results in a provider type
    pub struct_field_types: HashMap<(String, String), String>,
}

/// FFI ownership semantics for extern function returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FfiOwnership {
    /// No explicit ownership attribute - default behavior (no cleanup)
    #[default]
    None,
    /// @ffi_owned - Arth takes ownership of returned value (must cleanup)
    Owned,
    /// @ffi_borrowed - Value is borrowed (read-only, no cleanup)
    Borrowed,
    /// @ffi_transfers - Arth transfers ownership to C (moved, no cleanup)
    Transfers,
}

/// Signature information needed to lower an extern call correctly.
#[derive(Clone, Debug)]
pub struct ExternSig {
    pub params: Vec<ir::Ty>,
    pub ret: ir::Ty,
    /// FFI ownership semantics for the return value
    pub return_ownership: FfiOwnership,
    /// Return type name (for drop resolution when @ffi_owned)
    pub ret_type_name: Option<String>,
}

/// Metadata for a struct with @derive(JsonCodec)
#[derive(Clone, Debug, Default)]
pub struct JsonCodecMeta {
    /// Field metadata string: "name:idx,name:idx,...;flags"
    /// - name:idx pairs map JSON field name to struct field index
    /// - fields with @JsonIgnore are omitted
    /// - flags: 'I' = ignoreUnknown
    pub field_meta: String,
}

/// Infer the return type of a function from its lowered IR blocks.
/// Looks for Ret terminators and infers the type from the returned value.
fn infer_return_type_from_blocks(blocks: &[ir::BlockData]) -> ir::Ty {
    // Build a map from value indices to their inferred types
    let mut value_types: HashMap<u32, ir::Ty> = HashMap::new();

    for block in blocks {
        for inst in &block.insts {
            let ty = match &inst.kind {
                ir::InstKind::ConstI64(_) => ir::Ty::I64,
                ir::InstKind::ConstF64(_) => ir::Ty::F64,
                ir::InstKind::ConstStr(_) => ir::Ty::Ptr,
                ir::InstKind::Binary(_, _, _) => ir::Ty::I64, // Assume i64 for binary ops
                ir::InstKind::Cmp(_, _, _) => ir::Ty::I1,
                ir::InstKind::Alloca => ir::Ty::Ptr,
                ir::InstKind::Load(_) => ir::Ty::I64, // Default load to i64
                ir::InstKind::Copy(src) => value_types.get(&src.0).cloned().unwrap_or(ir::Ty::I64),
                ir::InstKind::Call { ret, .. } => ret.clone(),
                ir::InstKind::StrConcat(_, _) => ir::Ty::Ptr,
                ir::InstKind::StrEq(_, _) => ir::Ty::I1,
                ir::InstKind::MakeClosure { .. } => ir::Ty::I64, // Closure handle is i64
                ir::InstKind::ClosureCall { ret, .. } => ret.clone(),
                ir::InstKind::StructAlloc { .. } => ir::Ty::Ptr,
                ir::InstKind::StructFieldGet { .. } => ir::Ty::I64, // Default field to i64
                ir::InstKind::EnumAlloc { .. } => ir::Ty::Ptr,
                ir::InstKind::EnumGetTag { .. } => ir::Ty::I64,
                ir::InstKind::EnumGetPayload { .. } => ir::Ty::I64,
                _ => ir::Ty::I64, // Default to i64 for unknown
            };
            value_types.insert(inst.result.0, ty);
        }
    }

    // Look for the first Ret terminator with a value
    for block in blocks {
        if let ir::Terminator::Ret(Some(val)) = &block.term {
            return value_types.get(&val.0).cloned().unwrap_or(ir::Ty::I64);
        }
    }

    // No return value found - void
    ir::Ty::Void
}

/// Extract FFI ownership attribute from HIR attributes.
pub fn extract_ffi_ownership(attrs: &[crate::compiler::hir::HirAttr]) -> FfiOwnership {
    for attr in attrs {
        match attr.name.as_str() {
            "ffi_owned" => return FfiOwnership::Owned,
            "ffi_borrowed" => return FfiOwnership::Borrowed,
            "ffi_transfers" => return FfiOwnership::Transfers,
            _ => {}
        }
    }
    FfiOwnership::None
}

/// Extract return type name from HIR type for drop resolution.
fn extract_ret_type_name(ret: &Option<HirType>) -> Option<String> {
    ret.as_ref().map(|t| match t {
        HirType::Name { path } => path.join("."),
        HirType::Generic { path, .. } => path.join("."),
        HirType::TypeParam { name } => name.clone(),
    })
}

/// Lower an external function declaration (FFI) to IR.
/// Extern functions have no body - they're declarations for linking.
pub fn lower_hir_extern_func_to_ir(
    ef: &crate::compiler::hir::HirExternFunc,
) -> crate::compiler::ir::ExternFunc {
    /// Map HIR type to IR type for FFI-safe types
    fn map_ffi_ty_to_ir(t: &HirType) -> ir::Ty {
        let base = match t {
            HirType::Name { path } | HirType::Generic { path, .. } => path
                .last()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default(),
            HirType::TypeParam { name } => name.to_ascii_lowercase(),
        };
        match base.as_str() {
            "int" | "i32" | "long" | "i64" => ir::Ty::I64,
            "float" | "f32" | "double" | "f64" => ir::Ty::F64,
            "bool" => ir::Ty::I1,
            "void" => ir::Ty::Void,
            // Pointer types for FFI handles
            "ptr" | "pointer" => ir::Ty::Ptr,
            // Default to i64 for other numeric types
            "byte" | "u8" | "i8" | "short" | "i16" | "u16" | "u32" | "u64" => ir::Ty::I64,
            // Unrecognized types default to i64 (type checker should catch invalid FFI types)
            _ => ir::Ty::I64,
        }
    }

    let params: Vec<ir::Ty> = ef.params.iter().map(|p| map_ffi_ty_to_ir(&p.ty)).collect();
    let ret = ef
        .ret
        .as_ref()
        .map(map_ffi_ty_to_ir)
        .unwrap_or(ir::Ty::Void);

    crate::compiler::ir::ExternFunc {
        name: ef.name.clone(),
        abi: ef.abi.clone(),
        params,
        ret,
    }
}

/// Create an ExternSig from a HirExternFunc, including FFI ownership info.
pub fn make_extern_sig(ef: &crate::compiler::hir::HirExternFunc) -> ExternSig {
    let ir_extern = lower_hir_extern_func_to_ir(ef);
    ExternSig {
        params: ir_extern.params,
        ret: ir_extern.ret,
        return_ownership: extract_ffi_ownership(&ef.attrs),
        ret_type_name: extract_ret_type_name(&ef.ret),
    }
}

/// Map a HIR type to an IR type for struct/enum layout.
/// Handles nested structs, enums, optionals, and strings properly.
fn map_hir_type_to_ir_ty(
    t: &HirType,
    struct_names: &std::collections::HashSet<String>,
    enum_names: &std::collections::HashSet<String>,
) -> ir::Ty {
    match t {
        HirType::Name { path } => {
            let full_name = path.join(".");
            let base = path
                .last()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();

            match base.as_str() {
                "int" | "i32" | "long" | "i64" => ir::Ty::I64,
                "float" | "f32" | "double" | "f64" => ir::Ty::F64,
                "bool" | "boolean" => ir::Ty::I1,
                "void" => ir::Ty::Void,
                "ptr" | "pointer" => ir::Ty::Ptr,
                "string" => ir::Ty::String,
                _ => {
                    // Check if this is a known struct or enum type
                    if struct_names.contains(&full_name)
                        || struct_names.contains(path.last().unwrap_or(&String::new()))
                    {
                        ir::Ty::Struct(full_name)
                    } else if enum_names.contains(&full_name)
                        || enum_names.contains(path.last().unwrap_or(&String::new()))
                    {
                        ir::Ty::Enum(full_name)
                    } else {
                        // Unknown type - default to Ptr
                        ir::Ty::Ptr
                    }
                }
            }
        }
        HirType::Generic { path, args, .. } => {
            let base = path
                .last()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();

            // Handle Optional<T>
            if base == "optional" && args.len() == 1 {
                let inner = map_hir_type_to_ir_ty(&args[0], struct_names, enum_names);
                return ir::Ty::Optional(Box::new(inner));
            }

            // For other generics (List, Map, etc.), use Ptr for now
            // These are typically managed by the VM's heap
            ir::Ty::Ptr
        }
        HirType::TypeParam { name } => {
            // Type parameters are erased to Ptr
            let base = name.to_ascii_lowercase();
            match base.as_str() {
                "int" | "i32" | "long" | "i64" => ir::Ty::I64,
                "float" | "f32" | "double" | "f64" => ir::Ty::F64,
                "bool" | "boolean" => ir::Ty::I1,
                "void" => ir::Ty::Void,
                "string" => ir::Ty::String,
                _ => ir::Ty::Ptr,
            }
        }
    }
}

/// Lower a HIR struct definition to IR struct definition for LLVM codegen.
pub fn lower_hir_struct_to_ir(
    st: &crate::compiler::hir::HirStruct,
    struct_names: &std::collections::HashSet<String>,
    enum_names: &std::collections::HashSet<String>,
) -> ir::StructDef {
    ir::StructDef {
        name: st.name.clone(),
        fields: st
            .fields
            .iter()
            .map(|f| ir::StructFieldDef {
                name: f.name.clone(),
                ty: map_hir_type_to_ir_ty(&f.ty, struct_names, enum_names),
            })
            .collect(),
    }
}

/// Lower a HIR enum definition to IR enum definition for LLVM codegen.
pub fn lower_hir_enum_to_ir(
    en: &crate::compiler::hir::HirEnum,
    struct_names: &std::collections::HashSet<String>,
    enum_names: &std::collections::HashSet<String>,
) -> ir::EnumDef {
    use crate::compiler::hir::HirEnumVariant;

    ir::EnumDef {
        name: en.name.clone(),
        variants: en
            .variants
            .iter()
            .map(|v| match v {
                HirEnumVariant::Unit { name, .. } => ir::EnumVariantDef {
                    name: name.clone(),
                    payload_types: vec![],
                },
                HirEnumVariant::Tuple { name, types, .. } => ir::EnumVariantDef {
                    name: name.clone(),
                    payload_types: types
                        .iter()
                        .map(|t| map_hir_type_to_ir_ty(t, struct_names, enum_names))
                        .collect(),
                },
            })
            .collect(),
    }
}

pub fn lower_hir_provider_to_ir(pv: &crate::compiler::hir::HirProvider) -> ir::Provider {
    fn map_ty(t: &HirType) -> ir::Ty {
        let base = match t {
            HirType::Name { path } | HirType::Generic { path, .. } => path
                .last()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default(),
            HirType::TypeParam { name } => name.to_ascii_lowercase(),
        };
        match base.as_str() {
            "int" | "i32" | "long" | "i64" => ir::Ty::I64,
            "float" | "f32" | "double" | "f64" => ir::Ty::F64,
            "bool" => ir::Ty::I1,
            "void" => ir::Ty::Void,
            "ptr" | "pointer" => ir::Ty::Ptr,
            _ => ir::Ty::I64,
        }
    }

    ir::Provider {
        name: pv.name.clone(),
        fields: pv
            .fields
            .iter()
            .map(|f| ir::ProviderField {
                name: f.name.clone(),
                ty: map_ty(&f.ty),
                is_shared: f.is_shared,
            })
            .collect(),
    }
}

// HIR → IR lowering using stack slots for locals (alloca/load/store).
// This defers SSA construction to later passes.
#[allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::get_first,
    clippy::len_zero
)]
/// Options for controlling HIR→IR lowering behavior
#[derive(Default, Clone)]
pub struct LoweringOptions {
    /// Enable native struct/enum instructions for LLVM backend
    pub use_native_structs: bool,
    /// Struct definitions: struct_name -> Vec<(field_name, field_type)>
    pub struct_defs: HashMap<String, Vec<(String, String)>>,
}

pub fn lower_hir_func_to_ir_demo(
    hf: &HirFunc,
    enum_ctx: Option<&EnumLowerContext>,
) -> (Vec<ir::Func>, Vec<String>) {
    lower_hir_func_to_ir_with_escape(hf, enum_ctx, None)
}

/// Lower HIR function to IR with native struct support (for LLVM backend)
#[allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::get_first,
    clippy::len_zero
)]
pub fn lower_hir_func_to_ir_native(
    hf: &HirFunc,
    enum_ctx: Option<&EnumLowerContext>,
    escape_info: Option<&crate::compiler::typeck::FunctionEscapeInfo>,
    options: &LoweringOptions,
) -> (Vec<ir::Func>, Vec<String>) {
    lower_hir_func_to_ir_internal(hf, enum_ctx, escape_info, Some(options))
}

/// Lower HIR function to IR with optional escape analysis results
#[allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::get_first,
    clippy::len_zero
)]
pub fn lower_hir_func_to_ir_with_escape(
    hf: &HirFunc,
    enum_ctx: Option<&EnumLowerContext>,
    escape_info: Option<&crate::compiler::typeck::FunctionEscapeInfo>,
) -> (Vec<ir::Func>, Vec<String>) {
    lower_hir_func_to_ir_internal(hf, enum_ctx, escape_info, None)
}

/// Internal implementation of HIR→IR lowering
#[allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::get_first,
    clippy::len_zero
)]
fn lower_hir_func_to_ir_internal(
    hf: &HirFunc,
    enum_ctx: Option<&EnumLowerContext>,
    escape_info: Option<&crate::compiler::typeck::FunctionEscapeInfo>,
    options: Option<&LoweringOptions>,
) -> (Vec<ir::Func>, Vec<String>) {
    fn map_hir_ty_to_ir(
        t: &HirType,
        aliases: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> ir::Ty {
        // Very coarse mapping: ints/bools -> I64/I1; floats -> F64; aliases respected when pointing to primitives
        let (base, last) = match t {
            HirType::Name { path } | HirType::Generic { path, .. } => (
                path.last()
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default(),
                path.last().cloned(),
            ),
            HirType::TypeParam { name } => (name.to_ascii_lowercase(), Some(name.clone())),
        };
        let mut b = base;
        if let Some(name) = last
            && let Some(target) = aliases.get(&name)
            && let Some(tl) = target.last()
        {
            b = tl.to_ascii_lowercase();
        }
        match b.as_str() {
            "float" | "f64" | "double" => ir::Ty::F64,
            "bool" => ir::Ty::I1,
            _ => ir::Ty::I64,
        }
    }
    // Non-async: single lowered function (plus any lambda functions)
    if !hf.sig.is_async {
        let mut st = LowerState::default();
        st.escape_info = escape_info; // Set escape analysis results
        // Apply native struct options if provided
        if let Some(opts) = options {
            st.use_native_structs = opts.use_native_structs;
            st.struct_defs = opts.struct_defs.clone();
        }
        if let Some(ctx) = enum_ctx {
            st.enum_tags = Some(ctx.tags.clone());
            st.shared_field_names = ctx.shared_field_names.clone();
            st.type_aliases = ctx.type_aliases.clone();
            st.types_needing_drop = ctx.types_needing_drop.clone();
            st.json_codec_structs = ctx.json_codec_structs.clone();
            st.provider_names = ctx.provider_names.clone();
            st.extern_funcs = ctx.extern_funcs.clone();
            st.struct_field_types = ctx.struct_field_types.clone();
        }
        let entry = st.new_block("entry", None);
        st.cur = entry;
        // Initialize function-level drop scope
        st.push_drop_scope();
        // Reserve parameter value ids and materialize them into local slots
        let mut param_tys: Vec<ir::Ty> = Vec::new();
        let alias_map = enum_ctx.map(|c| c.type_aliases.clone()).unwrap_or_default();
        for p in &hf.sig.params {
            param_tys.push(map_hir_ty_to_ir(&p.ty, &alias_map));
        }
        let argc = param_tys.len() as u32;
        st.next_val = argc; // reserve Value(0..argc-1) as incoming params
        for (i, p) in hf.sig.params.iter().enumerate() {
            let slot = st.emit(ir::InstKind::Alloca, None);
            st.locals.insert(p.name.clone(), slot);
            // Track parameter type for string concat detection
            st.local_types.insert(p.name.clone(), p.ty.clone());
            let _ = st.emit(ir::InstKind::Store(slot, ir::Value(i as u32)), None);
            // Register parameters that need drop
            if let Some(ty_name) = st.needs_drop_ty(&p.ty) {
                if st.should_drop_var(&p.name) {
                    st.register_drop(&p.name, slot, &ty_name);
                }
            }
        }
        if let Some(body) = &hf.body {
            lower_block(&mut st, body);
        }
        let ret_ty = hf
            .sig
            .ret
            .as_ref()
            .map(|t| map_hir_ty_to_ir(t, &alias_map))
            .unwrap_or(ir::Ty::Void);
        let main_func = ir::Func {
            name: hf.sig.name.clone(),
            params: param_tys,
            ret: ret_ty,
            blocks: st.blocks,
            linkage: ir::Linkage::External, // Currently emitted external; visibility mapping can refine this later.
            span: Some(hf.span.clone()),
        };
        // Collect all functions: main function first, then lambda functions
        let mut funcs = vec![main_func];
        funcs.extend(st.lambda_funcs);
        return (funcs, st.strings);
    }

    // Async: produce a wrapper and a separate inner body function (plus lambda functions)
    // The async function is split into:
    //   1. A wrapper function that packages arguments and spawns the task
    //   2. An inner body function that receives arguments and executes the async logic
    let body_name = format!("{}$async_body", hf.sig.name);
    let alias_map = enum_ctx.map(|c| c.type_aliases.clone()).unwrap_or_default();

    // Build parameter type list for both wrapper and inner body
    let mut param_tys: Vec<ir::Ty> = Vec::new();
    for p in &hf.sig.params {
        param_tys.push(map_hir_ty_to_ir(&p.ty, &alias_map));
    }
    let argc = param_tys.len() as u32;

    // 1) Inner body function (`name$async_body`)
    // The inner body receives the same parameters as the original async function.
    // At runtime, these are passed via the task spawn mechanism.
    let mut st_body = LowerState::default();
    st_body.escape_info = escape_info; // Set escape analysis results
    // Apply native struct options if provided
    if let Some(opts) = options {
        st_body.use_native_structs = opts.use_native_structs;
        st_body.struct_defs = opts.struct_defs.clone();
    }
    if let Some(ctx) = enum_ctx {
        st_body.enum_tags = Some(ctx.tags.clone());
        st_body.shared_field_names = ctx.shared_field_names.clone();
        st_body.type_aliases = ctx.type_aliases.clone();
        st_body.types_needing_drop = ctx.types_needing_drop.clone();
        st_body.json_codec_structs = ctx.json_codec_structs.clone();
    }
    let entry_b = st_body.new_block("entry", None);
    st_body.cur = entry_b;

    // Initialize function-level drop scope for inner body
    st_body.push_drop_scope();

    // Reserve parameter value ids and materialize them into local slots
    // Same as non-async function parameter handling
    st_body.next_val = argc; // reserve Value(0..argc-1) as incoming params
    for (i, p) in hf.sig.params.iter().enumerate() {
        let slot = st_body.emit(ir::InstKind::Alloca, None);
        st_body.locals.insert(p.name.clone(), slot);
        // Track parameter type for string concat detection
        st_body.local_types.insert(p.name.clone(), p.ty.clone());
        let _ = st_body.emit(ir::InstKind::Store(slot, ir::Value(i as u32)), None);
        // Register parameters that need drop
        if let Some(ty_name) = st_body.needs_drop_ty(&p.ty) {
            if st_body.should_drop_var(&p.name) {
                st_body.register_drop(&p.name, slot, &ty_name);
            }
        }
    }

    if let Some(body) = &hf.body {
        lower_block(&mut st_body, body);
    }

    // Determine inner body return type: async functions return their declared type
    // wrapped in Task<T>, but the inner body returns the raw T.
    let inner_ret_ty = hf
        .sig
        .ret
        .as_ref()
        .map(|t| map_hir_ty_to_ir(t, &alias_map))
        .unwrap_or(ir::Ty::Void);

    let inner_func = ir::Func {
        name: body_name.clone(),
        params: param_tys.clone(),
        ret: inner_ret_ty.clone(),
        blocks: st_body.blocks,
        linkage: ir::Linkage::Private, // Inner async body is private
        span: Some(hf.span.clone()),
    };

    // 2) Wrapper that schedules the inner body and returns a Task handle
    // The wrapper receives the same parameters as the original function.
    // It packages them and calls the runtime to spawn the async task.
    let mut st = LowerState::default();
    let entry = st.new_block("entry", None);
    st.cur = entry;

    // Reserve parameter value ids for wrapper
    st.next_val = argc;

    // Compute function ID as a simple hash of the body function name.
    // This allows the runtime to look up the function to execute.
    // NOTE: Use the unqualified body name (strip module prefix) to match VM codegen lookup.
    // VM codegen strips the module prefix when building async_body_hashes.
    let unqualified_body_name = if let Some(dot_pos) = body_name.find('.') {
        &body_name[dot_pos + 1..]
    } else {
        &body_name
    };
    let fn_id_hash = compute_string_hash(unqualified_body_name);
    let fn_id = st.emit(ir::InstKind::ConstI64(fn_id_hash), None);
    let argc_val = st.emit(ir::InstKind::ConstI64(argc as i64), None);

    // Package arguments: if there are arguments, pass them through the runtime.
    // The runtime __arth_task_spawn_fn now receives (fn_id, argc, ...args).
    let mut spawn_args = vec![fn_id, argc_val];

    // Forward all incoming parameters to the spawn call
    for i in 0..argc {
        spawn_args.push(ir::Value(i));
    }

    if options.map(|o| o.use_native_structs).unwrap_or(false) {
        // Native backend phase-1 synchronous async mode: call the lowered body directly.
        let mut body_args = Vec::new();
        for i in 0..argc {
            body_args.push(ir::Value(i));
        }
        let direct = st.emit(
            ir::InstKind::Call {
                name: body_name.clone(),
                args: body_args,
                ret: inner_ret_ty.clone(),
            },
            None,
        );
        st.set_term(ir::Terminator::Ret(Some(direct)));
    } else {
        let handle = st.emit(
            ir::InstKind::Call {
                name: "__arth_task_spawn_fn".to_string(),
                args: spawn_args,
                ret: ir::Ty::I64,
            },
            None,
        );
        st.set_term(ir::Terminator::Ret(Some(handle)));
    }
    let wrapper = ir::Func {
        name: hf.sig.name.clone(),
        params: param_tys,
        ret: ir::Ty::I64, // Returns Task handle (i64)
        blocks: st.blocks,
        linkage: ir::Linkage::External, // Currently emitted external; visibility mapping can refine this later.
        span: Some(hf.span.clone()),
    };

    // Collect all functions: wrapper, inner body, then lambda functions
    let mut funcs = vec![wrapper, inner_func];
    funcs.extend(st_body.lambda_funcs);
    funcs.extend(st.lambda_funcs);

    // Merge string pools (preserve order; naive append)
    let mut strings = st_body.strings;
    strings.extend(st.strings);
    (funcs, strings)
}

fn lower_block(st: &mut LowerState, blk: &HirBlock) {
    // Push a new drop scope for this block
    st.push_drop_scope();
    for s in &blk.stmts {
        lower_stmt(st, s);
        // Stop processing if the block has been terminated (e.g., by throw, return, panic)
        // This prevents unreachable code from being emitted into the same block
        if st.is_block_terminated() {
            break;
        }
    }
    // Pop drop scope and emit drops at block exit (only if not already terminated)
    if !st.is_block_terminated() {
        st.pop_drop_scope(Some(blk.span.clone()));
    }
}

fn lower_stmt(st: &mut LowerState, s: &HirStmt) {
    match s {
        HirStmt::PrintStr { text, span, .. } => {
            // Lower println of a string literal via a VM helper; other backends can ignore it.
            let ix = intern_str(st, text);
            let sval = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
            let _ = st.emit(
                ir::InstKind::Call {
                    name: "__arth_vm_print_str".to_string(),
                    args: vec![sval],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );
        }
        HirStmt::PrintExpr { expr, span, .. } => {
            // Handle println of expressions for VM:
            //  - If it's a string literal: print string
            //  - If it's a concatenation chain starting with a string literal,
            //    print all parts without newline and then emit a single newline.
            //  - Else: evaluate and print numeric/bool value

            // Helper to check if a HirType is String
            fn is_string_type(ty: &HirType) -> bool {
                matches!(ty, HirType::Name { path } if path.len() == 1 && path[0] == "String")
            }

            // Helper: detect if the expression is a string or a string concatenation chain
            fn starts_with_str_concat(e: &HirExpr, local_types: &HashMap<String, HirType>) -> bool {
                match e {
                    HirExpr::Str { .. } => true,
                    HirExpr::Ident { name, .. } => {
                        // Check if the identifier is typed as String
                        local_types.get(name).map(is_string_type).unwrap_or(false)
                    }
                    HirExpr::Binary {
                        left,
                        op: HirBinOp::Add,
                        ..
                    } => starts_with_str_concat(left, local_types),
                    _ => false,
                }
            }

            // Helper: flatten a left-associative '+' chain into ordered segments
            enum Seg<'a> {
                Str(&'a str),
                Expr(&'a HirExpr),
            }
            fn flatten<'a>(e: &'a HirExpr, out: &mut Vec<Seg<'a>>) {
                match e {
                    HirExpr::Str { value, .. } => out.push(Seg::Str(value)),
                    HirExpr::Binary {
                        left,
                        op: HirBinOp::Add,
                        right,
                        ..
                    } => {
                        flatten(left, out);
                        flatten(right, out);
                    }
                    other => out.push(Seg::Expr(other)),
                }
            }

            if starts_with_str_concat(expr, &st.local_types) {
                // Expand into raw prints of each segment, followed by a newline.
                let mut segs: Vec<Seg> = Vec::new();
                flatten(expr, &mut segs);
                // Pre-intern an empty string for value-only segments
                let empty_ix = intern_str(st, "");
                let empty_v = st.emit(ir::InstKind::ConstStr(empty_ix), Some(span.clone()));
                for s in segs {
                    match s {
                        Seg::Str(t) => {
                            let ix = intern_str(st, t);
                            let sv = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_raw".to_string(),
                                    args: vec![sv],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        Seg::Expr(e) => {
                            let v = lower_expr(st, e);
                            // Print value without newline by using raw_str_val with empty prefix
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_raw_str_val".to_string(),
                                    args: vec![empty_v, v],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
                // Newline at end
                let _ = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_vm_print_ln".to_string(),
                        args: vec![],
                        ret: ir::Ty::I64,
                    },
                    Some(span.clone()),
                );
            } else {
                match expr {
                    HirExpr::Str { value, .. } => {
                        let ix = intern_str(st, value);
                        let sval = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
                        let _ = st.emit(
                            ir::InstKind::Call {
                                name: "__arth_vm_print_str".to_string(),
                                args: vec![sval],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                    HirExpr::Binary {
                        left,
                        op: HirBinOp::Add,
                        right,
                        ..
                    } => {
                        if let HirExpr::Str { value, .. } = &**left {
                            let ix = intern_str(st, value);
                            let sval = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
                            let v = lower_expr(st, right);
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_str_val".to_string(),
                                    args: vec![sval, v],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        } else {
                            let v = lower_expr(st, expr);
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_val".to_string(),
                                    args: vec![v],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                    _ => {
                        let v = lower_expr(st, expr);
                        let _ = st.emit(
                            ir::InstKind::Call {
                                name: "__arth_vm_print_val".to_string(),
                                args: vec![v],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }
        }
        HirStmt::PrintRawStr { text, span, .. } => {
            // Print string without newline
            let ix = intern_str(st, text);
            let sval = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
            let _ = st.emit(
                ir::InstKind::Call {
                    name: "__arth_vm_print_raw".to_string(),
                    args: vec![sval],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );
        }
        HirStmt::PrintRawExpr { expr, span, .. } => {
            // Print expression without newline
            // Helper to check if a HirType is String
            fn is_string_type(ty: &HirType) -> bool {
                matches!(ty, HirType::Name { path } if path.len() == 1 && path[0] == "String")
            }

            // Helper: detect if the expression is a string or a string concatenation chain
            fn starts_with_str_concat(e: &HirExpr, local_types: &HashMap<String, HirType>) -> bool {
                match e {
                    HirExpr::Str { .. } => true,
                    HirExpr::Ident { name, .. } => {
                        local_types.get(name).map(is_string_type).unwrap_or(false)
                    }
                    HirExpr::Binary {
                        left,
                        op: HirBinOp::Add,
                        ..
                    } => starts_with_str_concat(left, local_types),
                    _ => false,
                }
            }

            // Helper: flatten a left-associative '+' chain into ordered segments
            enum Seg<'a> {
                Str(&'a str),
                Expr(&'a HirExpr),
            }
            fn flatten<'a>(e: &'a HirExpr, out: &mut Vec<Seg<'a>>) {
                match e {
                    HirExpr::Str { value, .. } => out.push(Seg::Str(value)),
                    HirExpr::Binary {
                        left,
                        op: HirBinOp::Add,
                        right,
                        ..
                    } => {
                        flatten(left, out);
                        flatten(right, out);
                    }
                    other => out.push(Seg::Expr(other)),
                }
            }

            if starts_with_str_concat(expr, &st.local_types) {
                // Expand into raw prints of each segment (no newline)
                let mut segs: Vec<Seg> = Vec::new();
                flatten(expr, &mut segs);
                let empty_ix = intern_str(st, "");
                let empty_v = st.emit(ir::InstKind::ConstStr(empty_ix), Some(span.clone()));
                for s in segs {
                    match s {
                        Seg::Str(t) => {
                            let ix = intern_str(st, t);
                            let sv = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_raw".to_string(),
                                    args: vec![sv],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        Seg::Expr(e) => {
                            let v = lower_expr(st, e);
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_vm_print_raw_str_val".to_string(),
                                    args: vec![empty_v, v],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
                // NO newline at end for raw print
            } else {
                match expr {
                    HirExpr::Str { value, .. } => {
                        let ix = intern_str(st, value);
                        let sval = st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()));
                        let _ = st.emit(
                            ir::InstKind::Call {
                                name: "__arth_vm_print_raw".to_string(),
                                args: vec![sval],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                    _ => {
                        let v = lower_expr(st, expr);
                        let empty_ix = intern_str(st, "");
                        let empty_v = st.emit(ir::InstKind::ConstStr(empty_ix), Some(span.clone()));
                        let _ = st.emit(
                            ir::InstKind::Call {
                                name: "__arth_vm_print_raw_str_val".to_string(),
                                args: vec![empty_v, v],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }
        }
        HirStmt::VarDecl {
            name,
            init,
            is_shared,
            ty,
            ..
        } => {
            // Region-allocated locals use region allocator; others use stack alloca.
            // Region slots are still pointer-compatible for regular Load/Store lowering.
            let slot = match st.get_alloc_strategy_for(name) {
                AllocStrategy::Region(_) => {
                    let slot_size = st.emit(ir::InstKind::ConstI64(8), None);
                    st.emit(
                        ir::InstKind::Call {
                            name: "__arth_region_alloc".to_string(),
                            args: vec![slot_size],
                            ret: ir::Ty::I64,
                        },
                        None,
                    )
                }
                _ => st.emit(ir::InstKind::Alloca, None),
            };
            st.locals.insert(name.clone(), slot);
            // Track local variable type for string concat detection
            st.local_types.insert(name.clone(), ty.clone());
            // Register for drop if the type needs it
            if let Some(ty_name) = st.needs_drop_ty(ty) {
                st.register_drop(name, slot, &ty_name);
            }
            if *is_shared {
                st.shared_locals.insert(name.clone());
                let h = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_shared_new".to_string(),
                        args: vec![],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
                let _ = st.emit(ir::InstKind::Store(slot, h), None);
                if let Some(e) = init {
                    let v = lower_expr(st, e);
                    let hcur = st.emit(ir::InstKind::Load(slot), None);
                    let _ = st.emit(
                        ir::InstKind::Call {
                            name: "__arth_shared_store".to_string(),
                            args: vec![hcur, v],
                            ret: ir::Ty::I64,
                        },
                        None,
                    );
                }
            } else if let Some(e) = init {
                let v = lower_expr(st, e);
                let _ = st.emit(ir::InstKind::Store(slot, v), None);
            }
            // Register locals that need drop, consulting escape analysis (drop flags)
            if let Some(ty_name) = st.needs_drop_ty(ty) {
                if st.should_drop_var(name) {
                    st.register_drop(name, slot, &ty_name);
                }
            }
        }
        HirStmt::Assign {
            name, expr, span, ..
        } => {
            if let Some(&slot) = st.locals.get(name) {
                let v = lower_expr(st, expr);
                if st.shared_locals.contains(name) {
                    let hcur = st.emit(ir::InstKind::Load(slot), Some(span.clone()));
                    let _ = st.emit(
                        ir::InstKind::Call {
                            name: "__arth_shared_store".to_string(),
                            args: vec![hcur, v],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                } else {
                    let _ = st.emit(ir::InstKind::Store(slot, v), Some(span.clone()));
                }
            }
        }
        HirStmt::AssignOp {
            name,
            op,
            expr,
            span,
            ..
        } => {
            if let Some(&slot) = st.locals.get(name) {
                let curv = st.emit(ir::InstKind::Load(slot), Some(span.clone()));
                let rhs = lower_expr(st, expr);
                let bop = match op {
                    HirAssignOp::Add => ir::BinOp::Add,
                    HirAssignOp::Sub => ir::BinOp::Sub,
                    HirAssignOp::Mul => ir::BinOp::Mul,
                    HirAssignOp::Div => ir::BinOp::Div,
                    HirAssignOp::Mod => ir::BinOp::Mod,
                    HirAssignOp::Shl => ir::BinOp::Shl,
                    HirAssignOp::Shr => ir::BinOp::Shr,
                    HirAssignOp::And => ir::BinOp::And,
                    HirAssignOp::Or => ir::BinOp::Or,
                    HirAssignOp::Xor => ir::BinOp::Xor,
                };
                let res = st.emit(ir::InstKind::Binary(bop, curv, rhs), Some(span.clone()));
                let _ = st.emit(ir::InstKind::Store(slot, res), Some(span.clone()));
            }
        }
        HirStmt::FieldAssign {
            object,
            field,
            expr,
            span,
            ..
        } => {
            // Provider singleton field assignment: ProviderName.field = value
            if let HirExpr::Ident {
                name: obj_ident, ..
            } = object
            {
                if st.provider_names.contains(obj_ident) {
                    let v = lower_expr(st, expr);
                    return {
                        st.emit(
                            ir::InstKind::ProviderFieldSet {
                                obj: ir::Value(0), // Singleton
                                provider: obj_ident.clone(),
                                field: field.clone(),
                                value: v,
                            },
                            Some(span.clone()),
                        );
                    };
                }
            }

            // Check if the object expression evaluates to a provider type.
            // This handles both simple identifiers (c.count) and nested access (bundle.counter.value).
            if let Some(pname) = st.get_expr_provider_type(object) {
                let obj_val = lower_expr(st, object);
                let v = lower_expr(st, expr);
                st.emit(
                    ir::InstKind::ProviderFieldSet {
                        obj: obj_val,
                        provider: pname,
                        field: field.clone(),
                        value: v,
                    },
                    Some(span.clone()),
                );
                return;
            }

            // Non-provider instance field assignment.
            // In native mode, use direct typed field access for known structs.
            if st.use_native_structs
                && let Some(struct_name) = st.get_expr_type_name(object)
                && st.struct_defs.contains_key(&struct_name)
                && let Some(field_index) = st.get_field_index(&struct_name, field)
            {
                let obj_val = lower_expr(st, object);
                let v = lower_expr(st, expr);
                let _ = st.emit(
                    ir::InstKind::StructFieldSet {
                        ptr: obj_val,
                        type_name: struct_name,
                        field_name: field.clone(),
                        field_index,
                        value: v,
                    },
                    Some(span.clone()),
                );
                return;
            }

            // Fallback: dynamic runtime field set.
            let obj_val = lower_expr(st, object);
            let v = lower_expr(st, expr);
            let field_ix = intern_str(st, field);
            let field_key = st.emit(ir::InstKind::ConstStr(field_ix), Some(span.clone()));
            let _ = st.emit(
                ir::InstKind::Call {
                    name: "__arth_struct_set_named".to_string(),
                    args: vec![obj_val, field_key, v],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );
        }
        HirStmt::If {
            cond,
            then_blk,
            else_blk,
            span,
            ..
        } => {
            let then_b = st.new_block("then", Some(span.clone()));
            let else_b = st.new_block("else", Some(span.clone()));
            let join_b = st.new_block("join", Some(span.clone()));
            let cval = lower_cond(st, cond);
            let cur = st.cur;
            st.blocks[cur].term = ir::Terminator::CondBr {
                cond: cval,
                then_bb: ir::Block(then_b as u32),
                else_bb: ir::Block(else_b as u32),
            };

            st.set_cur(then_b);
            lower_block(st, then_blk);
            // Only branch to join if the block didn't end with return/throw/etc.
            st.set_term_if_fallthrough(ir::Terminator::Br(ir::Block(join_b as u32)));

            st.set_cur(else_b);
            if let Some(eb) = else_blk {
                lower_block(st, eb);
            }
            // Only branch to join if the block didn't end with return/throw/etc.
            st.set_term_if_fallthrough(ir::Terminator::Br(ir::Block(join_b as u32)));

            st.set_cur(join_b);
        }
        HirStmt::While {
            cond, body, span, ..
        } => {
            let header = st.new_block("loop_header", Some(span.clone()));
            let lbody = st.new_block("loop_body", Some(span.clone()));
            let exit = st.new_block("loop_exit", Some(span.clone()));
            // jump to header
            st.set_term(ir::Terminator::Br(ir::Block(header as u32)));
            // header evaluates condition
            st.set_cur(header);
            let cval = lower_cond(st, cond);
            st.set_term(ir::Terminator::CondBr {
                cond: cval,
                then_bb: ir::Block(lbody as u32),
                else_bb: ir::Block(exit as u32),
            });
            // body - enter loop region for region-based allocation (before push_drop_scope)
            st.set_cur(lbody);
            let region_id = st.enter_loop_region();
            // push loop with drop scope depth and region_id
            let drop_depth = st.drop_scope_depth();
            st.loops.push((exit, header, drop_depth, region_id));
            // attach label if any
            st.loop_labels.push(st.pending_loop_label.take());
            // Push a new drop scope for the loop body
            st.push_drop_scope();
            lower_block(st, body);
            // Pop drop scope and emit drops before continuing to header
            st.pop_drop_scope(Some(span.clone()));
            // Exit loop region (emits deinit calls and bulk deallocation)
            st.exit_loop_region(region_id);
            st.set_term(ir::Terminator::Br(ir::Block(header as u32)));
            st.loops.pop();
            let _ = st.loop_labels.pop();
            // continue after loop
            st.set_cur(exit);
        }
        HirStmt::Labeled { label, stmt, .. } => {
            // Set pending label for the next loop we lower
            st.pending_loop_label = Some(label.clone());
            lower_stmt(st, stmt);
            // If the inner wasn't a loop, pending label stays; clear it to avoid leaking to next stmt
            st.pending_loop_label = None;
        }
        HirStmt::For { .. } => {
            // 'for' is desugared in HIR to While/Block; we shouldn't see it here.
        }
        HirStmt::Switch {
            expr,
            cases,
            pattern_cases,
            default,
            ..
        } => {
            // Check if this is enum pattern matching
            let has_pattern_cases = !pattern_cases.is_empty();

            // Determine if this is an integer switch or a string switch
            let is_string_switch = cases
                .iter()
                .any(|(cexpr, _)| matches!(cexpr, HirExpr::Str { .. }));

            let join = st.new_block("switch_join", None);

            if has_pattern_cases {
                // Enum pattern matching: lower to tag comparisons and payload extraction
                let head = st.cur;

                // First pass: collect pattern info and declare bindings in vars map
                // This ensures variables are available when lowering block bodies
                struct PatternInfo {
                    tag: i64,                       // -1 for wildcard, -2 for binding, >= 0 for variant
                    bindings: Vec<(String, usize)>, // (var_name, payload_index)
                    span: HirSpan,
                }
                let mut patterns_with_blocks: Vec<(PatternInfo, &HirBlock)> = Vec::new();

                for (pat, blk) in pattern_cases {
                    match pat {
                        HirPattern::Variant {
                            enum_name,
                            variant_name,
                            payloads,
                            span,
                            ..
                        } => {
                            let tag = st
                                .enum_tags
                                .as_ref()
                                .and_then(|tags| tags.get(enum_name))
                                .and_then(|vmap| vmap.get(variant_name).copied())
                                .unwrap_or(-1);

                            let mut bindings: Vec<(String, usize)> = Vec::new();
                            for (idx, sub_pat) in payloads.iter().enumerate() {
                                if let HirPattern::Binding { name, .. } = sub_pat {
                                    // Pre-declare the variable so it's available during block lowering
                                    // Allocate a stack slot for this binding
                                    if !st.locals.contains_key(name) {
                                        let slot = st.emit(ir::InstKind::Alloca, None);
                                        st.locals.insert(name.clone(), slot);
                                    }
                                    bindings.push((name.clone(), idx));
                                }
                            }

                            patterns_with_blocks.push((
                                PatternInfo {
                                    tag,
                                    bindings,
                                    span: span.clone(),
                                },
                                blk,
                            ));
                        }
                        HirPattern::Wildcard { span, .. } => {
                            patterns_with_blocks.push((
                                PatternInfo {
                                    tag: -1,
                                    bindings: vec![],
                                    span: span.clone(),
                                },
                                blk,
                            ));
                        }
                        HirPattern::Binding { name, span, .. } => {
                            // Pre-declare binding variable
                            if !st.locals.contains_key(name) {
                                let slot = st.emit(ir::InstKind::Alloca, None);
                                st.locals.insert(name.clone(), slot);
                            }
                            patterns_with_blocks.push((
                                PatternInfo {
                                    tag: -2,
                                    bindings: vec![(name.clone(), usize::MAX)],
                                    span: span.clone(),
                                },
                                blk,
                            ));
                        }
                        HirPattern::Literal { .. } => {
                            // Literal patterns in enum matching are unusual, skip for now
                        }
                    }
                }

                // Second pass: create case blocks and lower block bodies
                let mut pattern_case_info: Vec<(i64, Vec<(String, usize)>, usize)> = Vec::new();

                for (info, blk) in &patterns_with_blocks {
                    let b = st.new_block(
                        match info.tag {
                            -1 => "case_wildcard",
                            -2 => "case_binding",
                            _ => "case_pattern",
                        },
                        Some(info.span.clone()),
                    );
                    st.set_cur(b);
                    lower_block(st, blk);
                    // Only add Br if block wasn't terminated by return/throw/etc
                    if !st.is_block_terminated() {
                        st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                    }
                    pattern_case_info.push((info.tag, info.bindings.clone(), b));
                }

                // Handle default block
                let def_b = if let Some(db) = default {
                    let b = st.new_block("default", Some(db.span.clone()));
                    st.set_cur(b);
                    lower_block(st, db);
                    // Only add Br if block wasn't terminated by return/throw/etc
                    if !st.is_block_terminated() {
                        st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                    }
                    b
                } else {
                    // Check if there's a wildcard or binding pattern
                    pattern_case_info
                        .iter()
                        .find(|(tag, _, _)| *tag < 0)
                        .map(|(_, _, b)| *b)
                        .unwrap_or(join)
                };

                // Return to head block to emit scrutinee evaluation and conditional branches
                st.set_cur(head);
                let scrut_v = lower_expr(st, expr);

                // Extract tag from scrutinee using native enum operation
                let tag_v = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_enum_get_tag".to_string(),
                        args: vec![scrut_v],
                        ret: ir::Ty::I64,
                    },
                    None,
                );

                // For each variant pattern, generate: if (tag == expected_tag) goto case_block else continue
                // We need to insert payload bindings at the start of each case block
                let variant_cases: Vec<(i64, Vec<(String, usize)>, usize)> = pattern_case_info
                    .iter()
                    .filter(|(tag, _, _)| *tag >= 0)
                    .cloned()
                    .collect();

                let mut remaining = variant_cases.into_iter().peekable();
                while let Some((expected_tag, bindings, case_block)) = remaining.next() {
                    let tag_const = st.emit(ir::InstKind::ConstI64(expected_tag), None);
                    let cmp = st.emit(ir::InstKind::Cmp(ir::CmpPred::Eq, tag_v, tag_const), None);

                    // If there are payload bindings, we need to insert them at start of case block
                    if !bindings.is_empty() {
                        // Create an intermediate block for binding setup
                        let setup_block = st.new_block("case_setup", None);
                        let next_check = if remaining.peek().is_some() {
                            st.new_block("pattern_check", None)
                        } else {
                            def_b
                        };

                        st.set_term(ir::Terminator::CondBr {
                            cond: cmp,
                            then_bb: ir::Block(setup_block as u32),
                            else_bb: ir::Block(next_check as u32),
                        });

                        // In setup block, extract payloads and store as locals, then jump to case
                        st.set_cur(setup_block);
                        for (name, payload_idx) in &bindings {
                            // Extract payload using native enum operation (0-based index)
                            let idx_val =
                                st.emit(ir::InstKind::ConstI64(*payload_idx as i64), None);
                            let payload_v = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_enum_get_payload".to_string(),
                                    args: vec![scrut_v, idx_val],
                                    ret: ir::Ty::I64,
                                },
                                None,
                            );
                            // Store to the pre-allocated local slot
                            if let Some(&slot) = st.locals.get(name) {
                                st.emit(ir::InstKind::Store(slot, payload_v), None);
                            }
                        }
                        st.set_term(ir::Terminator::Br(ir::Block(case_block as u32)));

                        if remaining.peek().is_some() {
                            st.set_cur(next_check);
                        }
                    } else {
                        // No bindings, just branch directly
                        let next_check = if remaining.peek().is_some() {
                            st.new_block("pattern_check", None)
                        } else {
                            def_b
                        };

                        st.set_term(ir::Terminator::CondBr {
                            cond: cmp,
                            then_bb: ir::Block(case_block as u32),
                            else_bb: ir::Block(next_check as u32),
                        });

                        if remaining.peek().is_some() {
                            st.set_cur(next_check);
                        }
                    }
                }

                // If no variant patterns, branch to default
                if pattern_case_info.iter().all(|(tag, _, _)| *tag < 0) {
                    st.set_term(ir::Terminator::Br(ir::Block(def_b as u32)));
                }

                st.set_cur(join);
            } else if is_string_switch {
                // String switch: lower to if-else chain with string comparisons
                // Save the head block where we'll evaluate the scrutinee and emit comparisons
                let head = st.cur;

                // First, collect case string values and create case blocks
                let mut string_cases: Vec<(String, usize)> = Vec::new();
                for (cexpr, blk) in cases {
                    if let HirExpr::Str { value, .. } = cexpr {
                        let b = st.new_block("case", Some(blk.span.clone()));
                        st.set_cur(b);
                        lower_block(st, blk);
                        st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                        string_cases.push((value.clone(), b));
                    }
                }

                let def_b = if let Some(db) = default {
                    let b = st.new_block("default", Some(db.span.clone()));
                    st.set_cur(b);
                    lower_block(st, db);
                    st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                    b
                } else {
                    join
                };

                // Return to the head block to emit the scrutinee evaluation and if-else chain
                st.set_cur(head);

                // Evaluate the scrutinee once
                let scrut_v = lower_expr(st, expr);

                // For each string case, generate: if (scrut == case_str) goto case_block else continue
                let mut remaining_cases = string_cases.into_iter().peekable();
                while let Some((case_str, case_block)) = remaining_cases.next() {
                    let case_str_ix = intern_str(st, &case_str);
                    let case_str_v = st.emit(ir::InstKind::ConstStr(case_str_ix), None);
                    let cmp_v = st.emit(ir::InstKind::StrEq(scrut_v, case_str_v), None);

                    let next_block = if remaining_cases.peek().is_some() {
                        st.new_block("case_check", None)
                    } else {
                        def_b
                    };

                    st.set_term(ir::Terminator::CondBr {
                        cond: cmp_v,
                        then_bb: ir::Block(case_block as u32),
                        else_bb: ir::Block(next_block as u32),
                    });

                    if remaining_cases.peek().is_some() {
                        st.set_cur(next_block);
                    }
                }

                // If no cases, just branch to default
                if cases.is_empty() {
                    st.set_term(ir::Terminator::Br(ir::Block(def_b as u32)));
                }

                st.set_cur(join);
            } else {
                // Integer switch: use native Switch terminator
                let head = st.cur;
                let mut case_blocks: Vec<(i64, usize)> = Vec::new();
                for (cexpr, blk) in cases {
                    if let Some(cval) = eval_int_const(cexpr) {
                        let b = st.new_block("case", Some(blk.span.clone()));
                        st.set_cur(b);
                        lower_block(st, blk);
                        // Only add Br if block wasn't terminated by return/throw/etc
                        if !st.is_block_terminated() {
                            st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                        }
                        case_blocks.push((cval, b));
                    }
                }
                let def_b = if let Some(db) = default {
                    let b = st.new_block("default", Some(db.span.clone()));
                    st.set_cur(b);
                    lower_block(st, db);
                    // Only add Br if block wasn't terminated by return/throw/etc
                    if !st.is_block_terminated() {
                        st.set_term(ir::Terminator::Br(ir::Block(join as u32)));
                    }
                    b
                } else {
                    join
                };
                // Back to head to place switch terminator
                st.set_cur(head);
                let sv = lower_expr(st, expr);
                let term = ir::Terminator::Switch {
                    scrut: sv,
                    default: ir::Block(def_b as u32),
                    cases: case_blocks
                        .iter()
                        .map(|(v, b)| (*v, ir::Block(*b as u32)))
                        .collect(),
                };
                st.set_term(term);
                st.set_cur(join);
            }
        }
        HirStmt::Try {
            try_blk,
            catches,
            finally_blk,
            span,
            ..
        } => {
            // Create continuation block (where execution continues after try/catch/finally)
            let join_block = st.new_block("try_join", Some(span.clone()));

            // Create landing pad for exceptions
            let unwind = st.new_block("landing_pad", Some(span.clone()));

            // Register exception handler before try body
            st.emit(
                ir::InstKind::SetUnwindHandler(ir::Block(unwind as u32)),
                Some(span.clone()),
            );

            // Push finally block to scope stack so returns inside try/catch will inline it
            if let Some(fin) = finally_blk {
                st.finally_scopes.push(fin.clone());
            }

            // Try body runs with unwind target set
            let saved_unwind = st.unwind_target;
            st.unwind_target = Some(unwind);
            lower_block(st, try_blk);
            st.unwind_target = saved_unwind;

            // Only emit normal path code if the try block didn't terminate early (e.g., via throw)
            // If the block was terminated by throw/return/panic, we don't want to overwrite it
            if !st.is_block_terminated() {
                // Clear exception handler after try body completes normally
                st.emit(ir::InstKind::ClearUnwindHandler, Some(span.clone()));

                // After try body completes normally, execute finally (if present) then go to join
                if let Some(fin) = finally_blk {
                    // Normal path: try completed, run finally
                    let normal_finally = st.new_block("finally_normal", Some(fin.span.clone()));
                    st.set_term(ir::Terminator::Br(ir::Block(normal_finally as u32)));
                    st.set_cur(normal_finally);
                    lower_block(st, fin);
                    st.set_term(ir::Terminator::Br(ir::Block(join_block as u32)));
                } else {
                    // No finally, just branch to join
                    st.set_term(ir::Terminator::Br(ir::Block(join_block as u32)));
                }
            }

            // Landing pad block (exception path)
            st.set_cur(unwind);

            // Helper: extract type name string from HirType for comparison
            fn type_to_string(ty: &HirType) -> String {
                match ty {
                    HirType::Name { path } => path.join("."),
                    HirType::Generic { path, .. } => path.join("."),
                    HirType::TypeParam { name } => name.clone(),
                }
            }

            // Collect catch types for DWARF exception handling
            let catch_types: Vec<String> = catches
                .iter()
                .filter_map(|c| c.ty.as_ref().map(type_to_string))
                .collect();
            // Check if there's a catch-all (catch without type or catch(Exception))
            let is_catch_all = catches.iter().any(|c| {
                c.ty.is_none()
                    || c.ty
                        .as_ref()
                        .is_some_and(|t| type_to_string(t) == "Exception")
            });

            // LandingPad yields the exception value with catch type info for LLVM DWARF dispatch
            let exc_val = st.emit(
                ir::InstKind::LandingPad {
                    catch_types,
                    is_catch_all,
                },
                Some(span.clone()),
            );

            // Get the exception type name for type-based catch dispatch
            // (still used for VM backend runtime dispatch)
            let exc_type_name = st.emit(ir::InstKind::GetTypeName(exc_val), Some(span.clone()));

            // Store exception in a slot so catch blocks can access it
            let exc_slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));
            st.emit(ir::InstKind::Store(exc_slot, exc_val), Some(span.clone()));

            // Create blocks for each catch clause and a rethrow block
            let rethrow_block = st.new_block("rethrow", Some(span.clone()));

            // Build chain of type checks: check_0 -> catch_0/check_1 -> catch_1/check_2 -> ... -> rethrow
            let mut check_blocks: Vec<usize> = Vec::new();
            let mut catch_blocks: Vec<usize> = Vec::new();

            for (i, catch_clause) in catches.iter().enumerate() {
                check_blocks
                    .push(st.new_block(&format!("check_{i}"), Some(catch_clause.span.clone())));
                catch_blocks
                    .push(st.new_block(&format!("catch_{i}"), Some(catch_clause.span.clone())));
            }

            // Branch to first check (or rethrow if no catches)
            if !catches.is_empty() {
                st.set_term(ir::Terminator::Br(ir::Block(check_blocks[0] as u32)));
            } else {
                st.set_term(ir::Terminator::Br(ir::Block(rethrow_block as u32)));
            }

            // Generate type check and catch body for each clause
            for (i, catch_clause) in catches.iter().enumerate() {
                // Type check block
                st.set_cur(check_blocks[i]);

                let expected_type = catch_clause
                    .ty
                    .as_ref()
                    .map(type_to_string)
                    .unwrap_or_default();

                // Create string constant for expected type name
                let expected_idx = intern_str(st, &expected_type);
                let expected_val = st.emit(
                    ir::InstKind::ConstStr(expected_idx),
                    Some(catch_clause.span.clone()),
                );

                // Compare exception type with expected type
                let is_match = st.emit(
                    ir::InstKind::StrEq(exc_type_name, expected_val),
                    Some(catch_clause.span.clone()),
                );

                // Branch: if match, go to catch body; else go to next check or rethrow
                let next_target = if i + 1 < catches.len() {
                    check_blocks[i + 1]
                } else {
                    rethrow_block
                };
                st.set_term(ir::Terminator::CondBr {
                    cond: is_match,
                    then_bb: ir::Block(catch_blocks[i] as u32),
                    else_bb: ir::Block(next_target as u32),
                });

                // Catch body block
                st.set_cur(catch_blocks[i]);

                // If the catch clause has a variable binding, create a local for it
                if let Some(var_name) = &catch_clause.var {
                    // Load exception from slot and bind to variable
                    let loaded_exc = st.emit(
                        ir::InstKind::Load(exc_slot),
                        Some(catch_clause.span.clone()),
                    );
                    let var_slot = st.emit(ir::InstKind::Alloca, Some(catch_clause.span.clone()));
                    st.emit(
                        ir::InstKind::Store(var_slot, loaded_exc),
                        Some(catch_clause.span.clone()),
                    );
                    st.locals.insert(var_name.clone(), var_slot);
                    if let Some(var_ty) = &catch_clause.ty {
                        st.local_types.insert(var_name.clone(), var_ty.clone());
                    }
                }

                // If there's a finally block, any throw in catch body should go through rethrow
                // (which runs finally then re-throws). Register rethrow_block as handler.
                let catch_saved_unwind = st.unwind_target;
                if finally_blk.is_some() {
                    st.unwind_target = Some(rethrow_block);
                }

                // Lower the catch block body
                lower_block(st, &catch_clause.block);

                // Restore unwind target
                st.unwind_target = catch_saved_unwind;

                // Remove the exception variable from scope
                if let Some(var_name) = &catch_clause.var {
                    st.locals.remove(var_name);
                    st.local_types.remove(var_name);
                }

                // After catch body, call end_catch to clean up the exception before exiting
                // This is only called when exiting normally (not when rethrowing)
                if !st.is_block_terminated() {
                    st.emit(
                        ir::InstKind::Call {
                            name: "__arth_end_catch".to_string(),
                            args: vec![],
                            ret: ir::Ty::Void,
                        },
                        Some(catch_clause.span.clone()),
                    );
                }

                // After catch body, go to finally (if present) or join
                if !st.is_block_terminated() {
                    if let Some(fin) = finally_blk {
                        let catch_finally =
                            st.new_block(&format!("finally_catch_{i}"), Some(fin.span.clone()));
                        st.set_term(ir::Terminator::Br(ir::Block(catch_finally as u32)));
                        st.set_cur(catch_finally);
                        lower_block(st, fin);
                        if !st.is_block_terminated() {
                            st.set_term(ir::Terminator::Br(ir::Block(join_block as u32)));
                        }
                    } else {
                        st.set_term(ir::Terminator::Br(ir::Block(join_block as u32)));
                    }
                }
            }

            // Rethrow block: execute finally (if present) then propagate to outer handler
            // This block is reached when no catch matched the exception type.
            // The exception is already stored in exc_slot from the landing pad.
            st.set_cur(rethrow_block);

            if let Some(fin) = finally_blk {
                // Execute finally before rethrowing
                let rethrow_finally = st.new_block("finally_rethrow", Some(fin.span.clone()));
                st.set_term(ir::Terminator::Br(ir::Block(rethrow_finally as u32)));
                st.set_cur(rethrow_finally);
                lower_block(st, fin);
            }
            // Re-throw the exception to outer handler
            // Load exception from slot and throw
            if !st.is_block_terminated() {
                let rethrow_val = st.emit(ir::InstKind::Load(exc_slot), Some(span.clone()));
                st.set_term(ir::Terminator::Throw(Some(rethrow_val)));
            }

            // Pop the finally block from scope stack
            if finally_blk.is_some() {
                st.finally_scopes.pop();
            }

            // Continue after try/catch/finally
            st.set_cur(join_block);
        }
        HirStmt::Break { label, span, .. } => {
            // Choose target by label (if provided), else innermost
            let idx = if let Some(name) = label {
                st.loop_labels
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, l)| l.as_ref().map(|s| s == name).unwrap_or(false))
                    .map(|(i, _)| i)
            } else {
                st.loops.len().checked_sub(1)
            };
            if let Some(i) = idx {
                let (brk, _, drop_depth, region_id) = st.loops[i];
                // Emit drops for scopes being exited (from current to loop's entry scope)
                st.emit_drops_to_depth(drop_depth, Some(span.clone()));
                // Exit the loop's region before jumping out
                st.exit_loop_region(region_id);
                st.set_term(ir::Terminator::Br(ir::Block(brk as u32)));
            } else {
                // Label not found - typeck should have caught this, emit unreachable as fallback
                st.set_term(ir::Terminator::Unreachable);
            }
        }
        HirStmt::Continue { label, span, .. } => {
            let idx = if let Some(name) = label {
                st.loop_labels
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, l)| l.as_ref().map(|s| s == name).unwrap_or(false))
                    .map(|(i, _)| i)
            } else {
                st.loops.len().checked_sub(1)
            };
            if let Some(i) = idx {
                let (_, cont, drop_depth, region_id) = st.loops[i];
                // Emit drops for scopes being exited (from current to loop's entry scope)
                st.emit_drops_to_depth(drop_depth, Some(span.clone()));
                // Exit the loop's region before continuing (each iteration gets fresh region)
                st.exit_loop_region(region_id);
                st.set_term(ir::Terminator::Br(ir::Block(cont as u32)));
            } else {
                // Label not found - typeck should have caught this, emit unreachable as fallback
                st.set_term(ir::Terminator::Unreachable);
            }
        }
        HirStmt::Return { expr, span, .. } => {
            // Evaluate return value first (if any)
            let ret_val = expr.as_ref().map(|e| lower_expr(st, e));

            // If we're inside try-finally scopes, inline the finally blocks before returning
            // Clone the scopes to avoid borrow issues, and iterate in order (inner to outer)
            let pending_finally = st.finally_scopes.clone();
            for fin_blk in &pending_finally {
                lower_block(st, fin_blk);
            }

            // Returning from inside loop scopes must release active regions first.
            st.exit_all_loop_regions();
            // Emit drops for all scopes before returning
            st.emit_all_drops(Some(span.clone()));
            // Then emit return
            if let Some(v) = ret_val {
                st.set_term(ir::Terminator::Ret(Some(v)));
            } else {
                st.set_term(ir::Terminator::Ret(None));
            }
        }
        HirStmt::Block(b) => lower_block(st, b),
        HirStmt::Unsafe { block, .. } => {
            // Unsafe blocks execute their contents normally in the IR
            // The unsafe-ness is tracked during typeck to allow raw pointer ops
            lower_block(st, block);
        }
        HirStmt::Throw { expr, span, .. } => {
            // Evaluate the thrown expression
            let exc_val = lower_expr(st, expr);
            // Exceptional exit must release active loop regions.
            st.exit_all_loop_regions();
            // Emit drops before throwing
            st.emit_all_drops(Some(span.clone()));
            // For now, throw is lowered to a special terminator
            // In a full implementation, this would involve exception handling runtime
            st.set_term(ir::Terminator::Throw(Some(exc_val)));
        }
        HirStmt::Panic { msg, span, .. } => {
            // Evaluate the panic message expression
            let msg_val = lower_expr(st, msg);
            // Exceptional exit must release active loop regions.
            st.exit_all_loop_regions();
            // Emit drops before panicking (drop-on-unwind)
            // Panics run drops in reverse declaration order, same as normal cleanup
            st.emit_all_drops(Some(span.clone()));
            // Panic terminates the current block and unwinds
            // Unlike Throw, panics cannot be caught by try/catch
            st.set_term(ir::Terminator::Panic(Some(msg_val)));
        }
        HirStmt::Expr { expr, .. } => {
            // Evaluate for side effects and drop result
            let _ = lower_expr(st, expr);
        }
    }
}

#[allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::get_first,
    clippy::len_zero
)]
fn lower_expr(st: &mut LowerState, e: &HirExpr) -> ir::Value {
    match e {
        HirExpr::Int { value, span, .. } => {
            st.emit(ir::InstKind::ConstI64(*value), Some(span.clone()))
        }
        HirExpr::Float { value, span, .. } => {
            // Lower float literals directly to F64 constants so backends/VM can
            // preserve floating semantics (printing, math intrinsics, etc.).
            st.emit(ir::InstKind::ConstF64(*value), Some(span.clone()))
        }
        HirExpr::Str { value, span, .. } => {
            let idx = intern_str(st, value);
            st.emit(ir::InstKind::ConstStr(idx), Some(span.clone()))
        }
        HirExpr::Char { value, span, .. } => {
            // MVP: represent char literals as 1-length strings for the VM demo.
            // This allows println and string concatenation to behave intuitively
            // without introducing a dedicated Char runtime value.
            let s = value.to_string();
            let idx = intern_str(st, &s);
            st.emit(ir::InstKind::ConstStr(idx), Some(span.clone()))
        }
        HirExpr::Bool { value, span, .. } => st.emit(
            ir::InstKind::ConstI64(if *value { 1 } else { 0 }),
            Some(span.clone()),
        ),
        HirExpr::Conditional {
            cond,
            then_expr,
            else_expr,
            span,
            ..
        } => {
            // Lower ternary via an explicit stack slot (alloca) to avoid phi for the VM backend.
            // 1) Allocate a temp slot to hold the selected value.
            let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));

            // 2) Branch on condition to then/else blocks, both storing into the slot, then jump to join.
            let cval = lower_cond(st, cond);
            let then_b = st.new_block("cond_then", Some(span.clone()));
            let else_b = st.new_block("cond_else", Some(span.clone()));
            let join_b = st.new_block("cond_join", Some(span.clone()));
            let cur = st.cur;
            st.blocks[cur].term = ir::Terminator::CondBr {
                cond: cval,
                then_bb: ir::Block(then_b as u32),
                else_bb: ir::Block(else_b as u32),
            };

            // Then path: compute and store to slot
            st.set_cur(then_b);
            let tv = lower_expr(st, then_expr);
            let _ = st.emit(ir::InstKind::Store(slot, tv), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // Else path: compute and store to slot
            st.set_cur(else_b);
            let fv = lower_expr(st, else_expr);
            let _ = st.emit(ir::InstKind::Store(slot, fv), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // 3) Join: load the selected value and return it.
            st.set_cur(join_b);
            st.emit(ir::InstKind::Load(slot), Some(span.clone()))
        }
        HirExpr::Await { expr, span, .. } => {
            // Special-case HTTP fetch in the VM path: treat
            //   await Http.fetch(Request.get(url))
            // as a direct intrinsic call that returns the response handle.
            if let HirExpr::Call { callee, args, .. } = &**expr {
                // Recognize Http.fetch(...)
                let is_http_fetch = if let HirExpr::Member { object, member, .. } = &**callee {
                    if member == "fetch" {
                        if let HirExpr::Ident { name, .. } = &**object {
                            name == "Http"
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if is_http_fetch && args.len() == 1 {
                    // Expect argument of the form Request.get(url)
                    if let HirExpr::Call {
                        callee: req_callee,
                        args: req_args,
                        ..
                    } = &args[0]
                    {
                        let is_request_get =
                            if let HirExpr::Member { object, member, .. } = &**req_callee {
                                if member == "get" {
                                    if let HirExpr::Ident { name, .. } = &**object {
                                        name == "Request"
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                        if is_request_get && req_args.len() == 1 {
                            // Lower directly to an HTTP fetch intrinsic that takes the URL.
                            let url_val = lower_expr(st, &req_args[0]);
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_http_fetch".to_string(),
                                    args: vec![url_val],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
            }

            // Native backend currently lowers async in synchronous mode for phase-1:
            // await just evaluates the underlying expression value directly.
            if st.use_native_structs {
                return lower_expr(st, expr);
            }

            // General await lowering: map await to a runtime await call that may unwind;
            // use invoke when within try.
            let h = lower_expr(st, expr);

            // NOTE: For MVP synchronous execution, we skip the cancellation check.
            // In the MVP model, async bodies run synchronously and can't be cancelled,
            // so checking res == -1 is unnecessary. Additionally, for non-numeric return
            // types (like String), comparing against -1 would cause a type mismatch error.
            // A full async implementation would need to handle cancellation differently,
            // perhaps with a separate task status check before retrieving the result.

            if let Some(unwind) = st.unwind_target {
                let res = st.fresh_value();
                let normal = st.new_block("after_await", Some(span.clone()));
                st.set_term(ir::Terminator::Invoke {
                    callee: "__arth_await".to_string(),
                    args: vec![h],
                    ret: ir::Ty::I64,
                    result: Some(res),
                    normal: ir::Block(normal as u32),
                    unwind: ir::Block(unwind as u32),
                });
                st.set_cur(normal);
                res
            } else {
                st.emit(
                    ir::InstKind::Call {
                        name: "__arth_await".to_string(),
                        args: vec![h],
                        ret: ir::Ty::I64,
                    },
                    Some(span.clone()),
                )
            }
        }
        HirExpr::Cast { to, expr, span, .. } => {
            // Lower numeric casts via pseudo runtime helpers that the VM compiler recognizes
            let v = lower_expr(st, expr);
            let tgt = match to {
                crate::compiler::hir::HirType::Name { path }
                | crate::compiler::hir::HirType::Generic { path, .. } => {
                    path.last().map(|s| s.as_str()).unwrap_or("")
                }
                crate::compiler::hir::HirType::TypeParam { name } => name.as_str(),
            };
            let (fname, ret_ty) = match tgt.to_ascii_lowercase().as_str() {
                // Signed int widths
                "i8" | "int8" => ("__arth_cast_i8", ir::Ty::I64),
                "i16" | "int16" | "short" => ("__arth_cast_i16", ir::Ty::I64),
                "i32" | "int32" | "int" => ("__arth_cast_i32", ir::Ty::I64),
                "i64" | "int64" | "long" => ("__arth_cast_i64", ir::Ty::I64),
                // Unsigned int widths
                "u8" | "uint8" => ("__arth_cast_u8", ir::Ty::I64),
                "u16" | "uint16" => ("__arth_cast_u16", ir::Ty::I64),
                "u32" | "uint32" => ("__arth_cast_u32", ir::Ty::I64),
                "u64" | "uint64" => ("__arth_cast_u64", ir::Ty::I64),
                // Floats
                "f32" | "float32" => ("__arth_cast_f32", ir::Ty::F64),
                "f64" | "float64" | "double" | "float" => ("__arth_cast_f64", ir::Ty::F64),
                // Other primitives
                "bool" | "boolean" => ("__arth_cast_bool", ir::Ty::I1),
                "char" => ("__arth_cast_char", ir::Ty::I64),
                // default to generic i64 cast
                _ => ("__arth_cast_i64", ir::Ty::I64),
            };
            st.emit(
                ir::InstKind::Call {
                    name: fname.to_string(),
                    args: vec![v],
                    ret: ret_ty,
                },
                Some(span.clone()),
            )
        }
        HirExpr::Ident { name, span, .. } => {
            if let Some(slot) = st.locals.get(name) {
                if st.shared_locals.contains(name) {
                    let h = st.emit(ir::InstKind::Load(*slot), Some(span.clone()));
                    st.emit(
                        ir::InstKind::Call {
                            name: "__arth_shared_load".to_string(),
                            args: vec![h],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    )
                } else {
                    st.emit(ir::InstKind::Load(*slot), Some(span.clone()))
                }
            } else {
                st.emit(ir::InstKind::ConstI64(0), Some(span.clone()))
            }
        }
        HirExpr::Binary {
            left,
            op,
            right,
            span,
            ..
        } => {
            let l = lower_expr(st, left);
            let r = lower_expr(st, right);
            match op {
                HirBinOp::Add => {
                    // Check if this is string concatenation
                    // Helper to check if a HirType is String
                    fn is_string_type(ty: &HirType) -> bool {
                        matches!(ty, HirType::Name { path } if path.len() == 1 && path[0] == "String")
                    }
                    // Helper to detect if an expression is a string or string concatenation
                    fn is_string_expr(e: &HirExpr, local_types: &HashMap<String, HirType>) -> bool {
                        match e {
                            HirExpr::Str { .. } => true,
                            HirExpr::Ident { name, .. } => {
                                local_types.get(name).map(is_string_type).unwrap_or(false)
                            }
                            HirExpr::Binary {
                                left,
                                op: HirBinOp::Add,
                                ..
                            } => is_string_expr(left, local_types),
                            _ => false,
                        }
                    }
                    if is_string_expr(left, &st.local_types)
                        || is_string_expr(right, &st.local_types)
                    {
                        st.emit(ir::InstKind::StrConcat(l, r), Some(span.clone()))
                    } else {
                        st.emit(
                            ir::InstKind::Binary(ir::BinOp::Add, l, r),
                            Some(span.clone()),
                        )
                    }
                }
                HirBinOp::Sub => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Sub, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Mul => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Mul, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Div => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Div, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Mod => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Mod, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Shl => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Shl, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Shr => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Shr, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Lt => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Lt, l, r), Some(span.clone()))
                }
                HirBinOp::Le => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Le, l, r), Some(span.clone()))
                }
                HirBinOp::Gt => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Gt, l, r), Some(span.clone()))
                }
                HirBinOp::Ge => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Ge, l, r), Some(span.clone()))
                }
                HirBinOp::Eq => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Eq, l, r), Some(span.clone()))
                }
                HirBinOp::Ne => {
                    st.emit(ir::InstKind::Cmp(ir::CmpPred::Ne, l, r), Some(span.clone()))
                }
                HirBinOp::And => st.emit(
                    ir::InstKind::Binary(ir::BinOp::And, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Or => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Or, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::BitAnd => st.emit(
                    ir::InstKind::Binary(ir::BinOp::And, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::BitOr => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Or, l, r),
                    Some(span.clone()),
                ),
                HirBinOp::Xor => st.emit(
                    ir::InstKind::Binary(ir::BinOp::Xor, l, r),
                    Some(span.clone()),
                ),
            }
        }
        HirExpr::Unary { op, expr, span, .. } => {
            match (op, &**expr) {
                // Constant fold unary minus for float literals to avoid i64 subtraction on F64
                (HirUnOp::Neg, HirExpr::Float { value, .. }) => {
                    st.emit(ir::InstKind::ConstF64(-value), Some(span.clone()))
                }
                (HirUnOp::Neg, _) => {
                    let v = lower_expr(st, expr);
                    let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                    st.emit(
                        ir::InstKind::Binary(ir::BinOp::Sub, zero, v),
                        Some(span.clone()),
                    )
                }
                (HirUnOp::Not, _) => {
                    let v = lower_expr(st, expr);
                    // icmp eq v, 0
                    let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                    st.emit(
                        ir::InstKind::Cmp(ir::CmpPred::Eq, v, zero),
                        Some(span.clone()),
                    )
                }
            }
        }
        HirExpr::Call {
            callee, args, span, ..
        } => {
            // Recognize Http.serve(port) and lower to a simple VM intrinsic
            // __arth_http_serve that records the requested port and returns a
            // synthetic server handle. This mirrors the Http.fetch prototype
            // and keeps the VM path free of real network I/O for now.
            if let HirExpr::Member { object, member, .. } = &**callee {
                if member == "serve" && args.len() == 1 {
                    let is_http_mod = match &**object {
                        HirExpr::Ident { name, .. } => name == "Http",
                        HirExpr::Member {
                            object: pkg,
                            member: m,
                            ..
                        } => {
                            matches!(**pkg, HirExpr::Ident { ref name, .. } if name == "net")
                                && m == "http"
                        }
                        _ => false,
                    };
                    if is_http_mod {
                        let port_val = lower_expr(st, &args[0]);
                        return st.emit(
                            ir::InstKind::Call {
                                name: "__arth_http_serve".to_string(),
                                args: vec![port_val],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }
            // Optional<T> (MVP): represent Optional<T> as i64 where 0 = empty, non-zero = present
            // Static: Optional.of(x) -> x; Optional.empty() -> 0
            if let HirExpr::Member {
                object: opt_mod,
                member: fname,
                ..
            } = &**callee
            {
                let is_optional_mod = match &**opt_mod {
                    HirExpr::Ident { name, .. } => name == "Optional",
                    HirExpr::Member {
                        object: pkg,
                        member: m,
                        ..
                    } => {
                        matches!(**pkg, HirExpr::Ident { ref name, .. } if name == "optional")
                            && m == "Optional"
                    }
                    _ => false,
                };
                if is_optional_mod {
                    if fname == "of" && !args.is_empty() {
                        return lower_expr(st, &args[0]);
                    } else if fname == "empty" {
                        return st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                    }
                }
            }
            // Instance methods on Optional: isPresent, isEmpty, orElse, orElseThrow, ifPresent
            // Skip if receiver is "Optional" module identifier (handled as module intrinsic below)
            if let HirExpr::Member { object, member, .. } = &**callee {
                let is_optional_module_call =
                    matches!(&**object, HirExpr::Ident { name, .. } if name == "Optional");
                if !is_optional_module_call {
                    let recv = lower_expr(st, object);
                    if member == "isPresent" {
                        let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                        return st.emit(
                            ir::InstKind::Cmp(ir::CmpPred::Ne, recv, zero),
                            Some(span.clone()),
                        );
                    }
                    if member == "isEmpty" {
                        let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                        return st.emit(
                            ir::InstKind::Cmp(ir::CmpPred::Eq, recv, zero),
                            Some(span.clone()),
                        );
                    }
                    if member == "orElse" && !args.is_empty() {
                        let alt = lower_expr(st, &args[0]);
                        let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));
                        let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                        let is_empty = st.emit(
                            ir::InstKind::Cmp(ir::CmpPred::Eq, recv, zero),
                            Some(span.clone()),
                        );
                        let then_b = st.new_block("opt_else", Some(span.clone()));
                        let else_b = st.new_block("opt_then", Some(span.clone()));
                        let join_b = st.new_block("opt_join", Some(span.clone()));
                        let cur = st.cur;
                        st.blocks[cur].term = ir::Terminator::CondBr {
                            cond: is_empty,
                            then_bb: ir::Block(then_b as u32),
                            else_bb: ir::Block(else_b as u32),
                        };
                        st.set_cur(then_b);
                        let _ = st.emit(ir::InstKind::Store(slot, alt), Some(span.clone()));
                        st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));
                        st.set_cur(else_b);
                        let _ = st.emit(ir::InstKind::Store(slot, recv), Some(span.clone()));
                        st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));
                        st.set_cur(join_b);
                        return st.emit(ir::InstKind::Load(slot), Some(span.clone()));
                    }
                    if member == "orElseThrow" {
                        let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                        let is_empty = st.emit(
                            ir::InstKind::Cmp(ir::CmpPred::Eq, recv, zero),
                            Some(span.clone()),
                        );
                        let throw_b = st.new_block("opt_throw", Some(span.clone()));
                        let cont_b = st.new_block("opt_cont", Some(span.clone()));
                        st.set_term(ir::Terminator::CondBr {
                            cond: is_empty,
                            then_bb: ir::Block(throw_b as u32),
                            else_bb: ir::Block(cont_b as u32),
                        });
                        st.set_cur(throw_b);
                        let msg_ix = intern_str(st, "ERROR: Optional is empty");
                        let msg_v = st.emit(ir::InstKind::ConstStr(msg_ix), Some(span.clone()));
                        let _ = st.emit(
                            ir::InstKind::Call {
                                name: "__arth_vm_print_str".to_string(),
                                args: vec![msg_v],
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                        st.set_term(ir::Terminator::Ret(None));
                        st.set_cur(cont_b);
                        return recv;
                    }
                    if member == "ifPresent" && args.len() == 1 {
                        let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                        let present = st.emit(
                            ir::InstKind::Cmp(ir::CmpPred::Ne, recv, zero),
                            Some(span.clone()),
                        );
                        let then_b = st.new_block("opt_ifpresent", Some(span.clone()));
                        let join_b = st.new_block("opt_join", Some(span.clone()));
                        let cur = st.cur;
                        st.blocks[cur].term = ir::Terminator::CondBr {
                            cond: present,
                            then_bb: ir::Block(then_b as u32),
                            else_bb: ir::Block(join_b as u32),
                        };
                        st.set_cur(then_b);
                        // best-effort callee name (module-qualified ignored in VM demo)
                        let fname = match &args[0] {
                            HirExpr::Ident { name, .. } => Some(name.clone()),
                            HirExpr::Member { member, .. } => Some(member.clone()),
                            _ => None,
                        };
                        if let Some(f) = fname {
                            let _ = st.emit(
                                ir::InstKind::Call {
                                    name: f,
                                    args: vec![recv],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        } else {
                            let _ = lower_expr(st, &args[0]);
                        }
                        st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));
                        st.set_cur(join_b);
                        return st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                    }
                } // end if !is_optional_module_call
            }
            // Enum variant constructors: Enum.Variant(args...) -> native enum handle
            #[allow(clippy::collapsible_if)]
            if let HirExpr::Member { object, member, .. } = &**callee {
                if let HirExpr::Ident {
                    name: enum_name, ..
                } = &**object
                {
                    let looks_type = enum_name
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_uppercase())
                        .unwrap_or(false);
                    if looks_type {
                        let tag_opt: Option<i64> = st
                            .enum_tags
                            .as_ref()
                            .and_then(|tags| tags.get(enum_name))
                            .and_then(|vmap| vmap.get(member).copied());
                        if let Some(tag) = tag_opt {
                            // Create native enum instance with payload
                            let enum_name_ix = intern_str(st, enum_name);
                            let variant_name_ix = intern_str(st, member);
                            let enum_name_val =
                                st.emit(ir::InstKind::ConstStr(enum_name_ix), Some(span.clone()));
                            let variant_name_val = st
                                .emit(ir::InstKind::ConstStr(variant_name_ix), Some(span.clone()));
                            let tag_val = st.emit(ir::InstKind::ConstI64(tag), Some(span.clone()));
                            let payload_count = st.emit(
                                ir::InstKind::ConstI64(args.len() as i64),
                                Some(span.clone()),
                            );

                            let handle = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_enum_new".to_string(),
                                    args: vec![
                                        enum_name_val,
                                        variant_name_val,
                                        tag_val,
                                        payload_count,
                                    ],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );

                            // Set each payload value
                            for (idx, a) in args.iter().enumerate() {
                                let v = lower_expr(st, a);
                                let idx_val = st.emit(ir::InstKind::ConstI64(idx as i64), None);
                                let _ = st.emit(
                                    ir::InstKind::Call {
                                        name: "__arth_enum_set_payload".to_string(),
                                        args: vec![handle, idx_val, v],
                                        ret: ir::Ty::I64,
                                    },
                                    None,
                                );
                            }
                            return handle;
                        }
                    }
                }
            }
            // Method sugar for collections: xs.push(v) -> __arth_list_push(xs,v); xs.get(i) -> __arth_list_get(xs,i); tbl.put(k,v) -> __arth_map_put(tbl,k,v)
            if let HirExpr::Member { object, member, .. } = &**callee {
                let is_local_receiver = matches!(**object, HirExpr::Ident { ref name, .. } if name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false));
                if is_local_receiver && member == "push" && !args.is_empty() {
                    let lobj = lower_expr(st, object);
                    let aval = lower_expr(st, &args[0]);
                    return st.emit(
                        ir::InstKind::Call {
                            name: "__arth_list_push".to_string(),
                            args: vec![lobj, aval],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
                if is_local_receiver && member == "get" && !args.is_empty() {
                    let lobj = lower_expr(st, object);
                    let idx = lower_expr(st, &args[0]);
                    return st.emit(
                        ir::InstKind::Call {
                            name: "__arth_list_get".to_string(),
                            args: vec![lobj, idx],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
                if is_local_receiver && member == "put" && args.len() >= 2 {
                    let lobj = lower_expr(st, object);
                    let k = lower_expr(st, &args[0]);
                    let v = lower_expr(st, &args[1]);
                    return st.emit(
                        ir::InstKind::Call {
                            name: "__arth_map_put".to_string(),
                            args: vec![lobj, k, v],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
            }
            // Recognize Logger.<level>(..., ...) and lower to intrinsic __arth_log_emit_str(level,event,msg,fields)
            #[allow(clippy::collapsible_if)]
            if let HirExpr::Member {
                object: _obj,
                member: level_name,
                ..
            } = &**callee
            {
                if matches!(
                    level_name.as_str(),
                    "trace" | "debug" | "info" | "warn" | "error"
                ) {
                    let lvl_i = match level_name.as_str() {
                        "trace" => 0i64,
                        "debug" => 1i64,
                        "info" => 2i64,
                        "warn" => 3i64,
                        _ => 4i64,
                    };
                    // Evaluate arguments and use real string constants where present
                    let lvl_v = st.emit(ir::InstKind::ConstI64(lvl_i), Some(span.clone()));
                    let ev_v = if let Some(a0) = args.first() {
                        lower_expr(st, a0)
                    } else {
                        let ix = intern_str(st, "");
                        st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()))
                    };
                    let msg_v = if let Some(a1) = args.get(1) {
                        lower_expr(st, a1)
                    } else {
                        let ix = intern_str(st, "");
                        st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()))
                    };
                    // Try VM-friendly multi-field dynamic printing: Fields.of(k1, v1, k2, v2, ...)
                    #[allow(clippy::collapsible_if, clippy::collapsible_match)]
                    if let Some(fa) = args.get(2) {
                        if let HirExpr::Call {
                            callee: fcal,
                            args: fargs,
                            ..
                        } = fa
                        {
                            if let HirExpr::Member { object, member, .. } = &**fcal {
                                let is_fields = match &**object {
                                    HirExpr::Ident { name, .. } if name == "Fields" => true,
                                    HirExpr::Member {
                                        object: pkg,
                                        member: m,
                                        ..
                                    } => {
                                        matches!(**pkg, HirExpr::Ident { ref name, .. } if name=="log")
                                            && m == "Fields"
                                    }
                                    _ => false,
                                };
                                if is_fields && member == "of" {
                                    // Build prefix string
                                    let lvl_s = match lvl_i {
                                        0 => "TRACE",
                                        1 => "DEBUG",
                                        2 => "INFO",
                                        3 => "WARN",
                                        _ => "ERROR",
                                    };
                                    // Assume event/message are string literals for now
                                    let ev_s =
                                        if let Some(HirExpr::Str { value, .. }) = args.first() {
                                            value.clone()
                                        } else {
                                            String::new()
                                        };
                                    let msg_s =
                                        if let Some(HirExpr::Str { value, .. }) = args.get(1) {
                                            value.clone()
                                        } else {
                                            String::new()
                                        };
                                    let mut prefix = String::new();
                                    prefix.push_str(lvl_s);
                                    if !ev_s.is_empty() {
                                        prefix.push(' ');
                                        prefix.push_str(&ev_s);
                                    }
                                    if !msg_s.is_empty() {
                                        prefix.push_str(": ");
                                        prefix.push_str(&msg_s);
                                    }
                                    let pfx_ix = intern_str(st, &prefix);
                                    let pfx_v =
                                        st.emit(ir::InstKind::ConstStr(pfx_ix), Some(span.clone()));
                                    let _ = st.emit(
                                        ir::InstKind::Call {
                                            name: "__arth_vm_print_raw".to_string(),
                                            args: vec![pfx_v],
                                            ret: ir::Ty::I64,
                                        },
                                        Some(span.clone()),
                                    );

                                    // Helper: constant to string (subset)
                                    fn const_to_string(e: &HirExpr) -> Option<String> {
                                        use crate::compiler::hir::HirBinOp as HB;
                                        match e {
                                            HirExpr::Str { value, .. } => Some(value.clone()),
                                            HirExpr::Int { value, .. } => Some(value.to_string()),
                                            HirExpr::Float { value, .. } => {
                                                Some(format!("{}", value))
                                            }
                                            HirExpr::Char { value, .. } => Some(value.to_string()),
                                            HirExpr::Bool { value, .. } => Some(if *value {
                                                "true".into()
                                            } else {
                                                "false".into()
                                            }),
                                            HirExpr::Binary {
                                                left, op, right, ..
                                            } => {
                                                let ls = const_to_string(left)?;
                                                let rs = const_to_string(right)?;
                                                let (li, ri) = (
                                                    ls.parse::<i64>().ok(),
                                                    rs.parse::<i64>().ok(),
                                                );
                                                let (lf, rf) = (
                                                    ls.parse::<f64>().ok(),
                                                    rs.parse::<f64>().ok(),
                                                );
                                                if let (Some(a), Some(b)) = (li, ri) {
                                                    match op {
                                                        HB::Add => Some((a + b).to_string()),
                                                        HB::Sub => Some((a - b).to_string()),
                                                        HB::Mul => Some((a * b).to_string()),
                                                        HB::Div => Some(
                                                            (if b == 0 { a } else { a / b })
                                                                .to_string(),
                                                        ),
                                                        HB::Mod => Some(
                                                            (if b == 0 { 0 } else { a % b })
                                                                .to_string(),
                                                        ),
                                                        HB::Shl => Some((a << b).to_string()),
                                                        HB::Shr => Some((a >> b).to_string()),
                                                        HB::Lt => Some((a < b).to_string()),
                                                        HB::Le => Some((a <= b).to_string()),
                                                        HB::Gt => Some((a > b).to_string()),
                                                        HB::Ge => Some((a >= b).to_string()),
                                                        HB::Eq => Some((a == b).to_string()),
                                                        HB::Ne => Some((a != b).to_string()),
                                                        HB::And => {
                                                            Some(((a != 0) & (b != 0)).to_string())
                                                        }
                                                        HB::Or => {
                                                            Some(((a != 0) | (b != 0)).to_string())
                                                        }
                                                        HB::BitAnd => Some((a & b).to_string()),
                                                        HB::BitOr => Some((a | b).to_string()),
                                                        HB::Xor => Some((a ^ b).to_string()),
                                                    }
                                                } else if let (Some(a), Some(b)) = (lf, rf) {
                                                    match op {
                                                        HB::Add => Some((a + b).to_string()),
                                                        HB::Sub => Some((a - b).to_string()),
                                                        HB::Mul => Some((a * b).to_string()),
                                                        HB::Div => Some(
                                                            (if b == 0.0 { a } else { a / b })
                                                                .to_string(),
                                                        ),
                                                        HB::Mod => None,
                                                        HB::Shl | HB::Shr => None,
                                                        HB::Lt => Some((a < b).to_string()),
                                                        HB::Le => Some((a <= b).to_string()),
                                                        HB::Gt => Some((a > b).to_string()),
                                                        HB::Ge => Some((a >= b).to_string()),
                                                        HB::Eq => Some((a == b).to_string()),
                                                        HB::Ne => Some((a != b).to_string()),
                                                        HB::And
                                                        | HB::Or
                                                        | HB::BitAnd
                                                        | HB::BitOr
                                                        | HB::Xor => None,
                                                    }
                                                } else {
                                                    None
                                                }
                                            }
                                            _ => None,
                                        }
                                    }

                                    let mut it = fargs.iter();
                                    while let Some(k) = it.next() {
                                        let v = it.next();
                                        let key = const_to_string(k).unwrap_or_else(|| "?".into());
                                        let label = format!(" {}=", key);
                                        let lab_ix = intern_str(st, &label);
                                        let lab_v = st.emit(
                                            ir::InstKind::ConstStr(lab_ix),
                                            Some(span.clone()),
                                        );
                                        if let Some(val_e) = v {
                                            match val_e {
                                                HirExpr::Str { value, .. } => {
                                                    let vs_ix = intern_str(st, value);
                                                    let vs_v = st.emit(
                                                        ir::InstKind::ConstStr(vs_ix),
                                                        Some(span.clone()),
                                                    );
                                                    let _ = st.emit(
                                                        ir::InstKind::Call {
                                                            name: "__arth_vm_print_raw".to_string(),
                                                            args: vec![lab_v],
                                                            ret: ir::Ty::I64,
                                                        },
                                                        Some(span.clone()),
                                                    );
                                                    let _ = st.emit(
                                                        ir::InstKind::Call {
                                                            name: "__arth_vm_print_raw".to_string(),
                                                            args: vec![vs_v],
                                                            ret: ir::Ty::I64,
                                                        },
                                                        Some(span.clone()),
                                                    );
                                                }
                                                HirExpr::Float { value, .. } => {
                                                    let vs_ix =
                                                        intern_str(st, &format!("{}", value));
                                                    let vs_v = st.emit(
                                                        ir::InstKind::ConstStr(vs_ix),
                                                        Some(span.clone()),
                                                    );
                                                    let _ = st.emit(
                                                        ir::InstKind::Call {
                                                            name: "__arth_vm_print_raw".to_string(),
                                                            args: vec![lab_v],
                                                            ret: ir::Ty::I64,
                                                        },
                                                        Some(span.clone()),
                                                    );
                                                    let _ = st.emit(
                                                        ir::InstKind::Call {
                                                            name: "__arth_vm_print_raw".to_string(),
                                                            args: vec![vs_v],
                                                            ret: ir::Ty::I64,
                                                        },
                                                        Some(span.clone()),
                                                    );
                                                }
                                                _ => {
                                                    let vv = lower_expr(st, val_e);
                                                    let _ = st.emit(
                                                        ir::InstKind::Call {
                                                            name: "__arth_vm_print_raw_str_val"
                                                                .to_string(),
                                                            args: vec![lab_v, vv],
                                                            ret: ir::Ty::I64,
                                                        },
                                                        Some(span.clone()),
                                                    );
                                                }
                                            }
                                        } else {
                                            let vs_ix = intern_str(st, "?");
                                            let vs_v = st.emit(
                                                ir::InstKind::ConstStr(vs_ix),
                                                Some(span.clone()),
                                            );
                                            let _ = st.emit(
                                                ir::InstKind::Call {
                                                    name: "__arth_vm_print_raw".to_string(),
                                                    args: vec![lab_v],
                                                    ret: ir::Ty::I64,
                                                },
                                                Some(span.clone()),
                                            );
                                            let _ = st.emit(
                                                ir::InstKind::Call {
                                                    name: "__arth_vm_print_raw".to_string(),
                                                    args: vec![vs_v],
                                                    ret: ir::Ty::I64,
                                                },
                                                Some(span.clone()),
                                            );
                                        }
                                    }
                                    let _ = st.emit(
                                        ir::InstKind::Call {
                                            name: "__arth_vm_print_ln".to_string(),
                                            args: vec![],
                                            ret: ir::Ty::I64,
                                        },
                                        Some(span.clone()),
                                    );
                                    return st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                                }
                            }
                        }
                    }
                    let fields_v = if let Some(a2) = args.get(2) {
                        lower_fields_buffer(st, a2)
                    } else {
                        let ix = intern_str(st, "");
                        st.emit(ir::InstKind::ConstStr(ix), Some(span.clone()))
                    };
                    return st.emit(
                        ir::InstKind::Call {
                            name: "__arth_log_emit_str".to_string(),
                            args: vec![lvl_v, ev_v, msg_v, fields_v],
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
            }

            // Recognize math.Math.<fn>(...) and lower to math intrinsics for the VM
            // Patterns supported: Ident("math").Math.<name>
            if let HirExpr::Member {
                object: math_mod,
                member: fname,
                ..
            } = &**callee
            {
                if let HirExpr::Member {
                    object: pkg,
                    member: mname,
                    ..
                } = &**math_mod
                {
                    if matches!(**pkg, HirExpr::Ident { ref name, .. } if name == "math")
                        && mname == "Math"
                    {
                        // Special-case round with precision: round(x, n) should map to __arth_math_round_n
                        if fname == "round" && args.len() == 2 {
                            let arg_vals = vec![lower_expr(st, &args[0]), lower_expr(st, &args[1])];
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_math_round_n".to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        let name = match fname.as_str() {
                            "sqrt" => Some("__arth_math_sqrt"),
                            "pow" => Some("__arth_math_pow"),
                            "sin" => Some("__arth_math_sin"),
                            "cos" => Some("__arth_math_cos"),
                            "tan" => Some("__arth_math_tan"),
                            "floor" => Some("__arth_math_floor"),
                            "ceil" => Some("__arth_math_ceil"),
                            "round" => Some("__arth_math_round"),
                            "minF" => Some("__arth_math_min_f"),
                            "maxF" => Some("__arth_math_max_f"),
                            "clampF" => Some("__arth_math_clamp_f"),
                            "absF" => Some("__arth_math_abs_f"),
                            "minI" => Some("__arth_math_min_i"),
                            "maxI" => Some("__arth_math_max_i"),
                            "clampI" => Some("__arth_math_clamp_i"),
                            "absI" => Some("__arth_math_abs_i"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::with_capacity(args.len());
                            for a in args {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
                // Also support unqualified import usage: Math.<name>(...)
                if let HirExpr::Ident { name: m, .. } = &**math_mod {
                    if m == "Math" {
                        if fname == "round" && args.len() == 2 {
                            let arg_vals = vec![lower_expr(st, &args[0]), lower_expr(st, &args[1])];
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_math_round_n".to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        let name = match fname.as_str() {
                            "sqrt" => Some("__arth_math_sqrt"),
                            "pow" => Some("__arth_math_pow"),
                            "sin" => Some("__arth_math_sin"),
                            "cos" => Some("__arth_math_cos"),
                            "tan" => Some("__arth_math_tan"),
                            "floor" => Some("__arth_math_floor"),
                            "ceil" => Some("__arth_math_ceil"),
                            "round" => Some("__arth_math_round"),
                            "minF" => Some("__arth_math_min_f"),
                            "maxF" => Some("__arth_math_max_f"),
                            "clampF" => Some("__arth_math_clamp_f"),
                            "absF" => Some("__arth_math_abs_f"),
                            "minI" => Some("__arth_math_min_i"),
                            "maxI" => Some("__arth_math_max_i"),
                            "clampI" => Some("__arth_math_clamp_i"),
                            "absI" => Some("__arth_math_abs_i"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::with_capacity(args.len());
                            for a in args {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
            }

            // Recognize list.List.<fn>(...) and lower to list intrinsics for the VM (MVP int lists)
            if let HirExpr::Member {
                object: list_mod,
                member: fname,
                ..
            } = &**callee
            {
                if let HirExpr::Member {
                    object: pkg,
                    member: mname,
                    ..
                } = &**list_mod
                {
                    if matches!(**pkg, HirExpr::Ident { ref name, .. } if name == "list")
                        && mname == "List"
                    {
                        let name = match fname.as_str() {
                            "new" => Some("__arth_list_new"),
                            "push" => Some("__arth_list_push"),
                            "get" => Some("__arth_list_get"),
                            "set" => Some("__arth_list_set"),
                            "len" => Some("__arth_list_len"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::new();
                            // For mutators, accept an optional trailing capability arg and ignore it
                            let take = match fname.as_str() {
                                "push" => 2,
                                "get" => 2,
                                "set" => 3,
                                "len" => 1,
                                _ => args.len(),
                            };
                            for a in args.iter().take(take) {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
                // Unqualified import usage: List.<fn>(...)
                if let HirExpr::Ident { name: m, .. } = &**list_mod {
                    if m == "List" {
                        // Only map operations that have actual VM opcodes.
                        // Other operations (indexOf, contains, insert, clear, reverse, concat, slice, unique)
                        // are implemented as pure Arth code in stdlib/src/arth/array.arth
                        let name = match fname.as_str() {
                            "new" => Some("__arth_list_new"),
                            "push" => Some("__arth_list_push"),
                            "get" => Some("__arth_list_get"),
                            "set" => Some("__arth_list_set"),
                            "len" => Some("__arth_list_len"),
                            "remove" => Some("__arth_list_remove"),
                            "sort" => Some("__arth_list_sort"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::new();
                            let take = match fname.as_str() {
                                "push" => 2,
                                "get" => 2,
                                "set" => 3,
                                "len" => 1,
                                "remove" => 2,
                                "sort" => 1,
                                _ => args.len(),
                            };
                            for a in args.iter().take(take) {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
            }

            // Recognize Strings.<fn>(...) and lower to string intrinsics for the VM
            if let HirExpr::Member {
                object: str_mod,
                member: fname,
                ..
            } = &**callee
            {
                // Check if this is a Strings module call (qualified or unqualified)
                let is_strings_module = match &**str_mod {
                    HirExpr::Ident { name, .. } => name == "Strings",
                    HirExpr::Member {
                        object: _pkg,
                        member: m,
                        ..
                    } => m == "Strings",
                    _ => false,
                };
                if is_strings_module {
                    // Map Strings functions to their intrinsic names
                    let (intrinsic, arg_count, ret_ty) = match fname.as_str() {
                        "len" => (Some("__arth_str_len"), 1, ir::Ty::I64),
                        "substring" => (Some("__arth_str_substring"), 3, ir::Ty::I64), // returns str idx
                        "indexOf" => (Some("__arth_str_indexof"), 2, ir::Ty::I64),
                        "lastIndexOf" => (Some("__arth_str_lastindexof"), 2, ir::Ty::I64),
                        "startsWith" => (Some("__arth_str_startswith"), 2, ir::Ty::I64),
                        "endsWith" => (Some("__arth_str_endswith"), 2, ir::Ty::I64),
                        "split" => (Some("__arth_str_split"), 2, ir::Ty::I64), // returns list handle
                        "trim" => (Some("__arth_str_trim"), 1, ir::Ty::I64),   // returns str idx
                        "toLower" => (Some("__arth_str_tolower"), 1, ir::Ty::I64),
                        "toUpper" => (Some("__arth_str_toupper"), 1, ir::Ty::I64),
                        "replace" => (Some("__arth_str_replace"), 3, ir::Ty::I64),
                        "charAt" => (Some("__arth_str_charat"), 2, ir::Ty::I64),
                        "contains" => (Some("__arth_str_contains"), 2, ir::Ty::I64),
                        "repeat" => (Some("__arth_str_repeat"), 2, ir::Ty::I64),
                        "parseInt" => (Some("__arth_str_parseint"), 1, ir::Ty::I64),
                        "parseFloat" => (Some("__arth_str_parsefloat"), 1, ir::Ty::F64),
                        "fromInt" => (Some("__arth_str_fromint"), 1, ir::Ty::I64),
                        "fromFloat" => (Some("__arth_str_fromfloat"), 1, ir::Ty::I64),
                        _ => (None, 0, ir::Ty::I64),
                    };
                    if let Some(intrin) = intrinsic {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args.iter().take(arg_count) {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ret_ty,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }

            // Recognize Enum.<fn>(...) helpers for enum payload access (tag/get)
            if let HirExpr::Member {
                object: en_mod,
                member: fname,
                ..
            } = &**callee
            {
                // Qualified: pkg.Enum or unqualified: Enum
                let is_enum_module = match &**en_mod {
                    HirExpr::Ident { name, .. } => name == "Enum",
                    HirExpr::Member {
                        object: _pkg,
                        member: m,
                        ..
                    } => m == "Enum",
                    _ => false,
                };
                if is_enum_module {
                    match fname.as_str() {
                        "tag" if !args.is_empty() => {
                            // tag(e) -> __arth_list_get(e, 0)
                            let h = lower_expr(st, &args[0]);
                            let zero = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_list_get".to_string(),
                                    args: vec![h, zero],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        "get" if args.len() >= 2 => {
                            // get(e, i) -> __arth_list_get(e, i+1)
                            let h = lower_expr(st, &args[0]);
                            let idx = lower_expr(st, &args[1]);
                            let one = st.emit(ir::InstKind::ConstI64(1), Some(span.clone()));
                            let ip1 = st.emit(
                                ir::InstKind::Binary(ir::BinOp::Add, idx, one),
                                Some(span.clone()),
                            );
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_list_get".to_string(),
                                    args: vec![h, ip1],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        "payloadCount" if !args.is_empty() => {
                            // payloadCount(e) -> __arth_list_len(e) - 1
                            let h = lower_expr(st, &args[0]);
                            let len = st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_list_len".to_string(),
                                    args: vec![h],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                            let one = st.emit(ir::InstKind::ConstI64(1), Some(span.clone()));
                            return st.emit(
                                ir::InstKind::Binary(ir::BinOp::Sub, len, one),
                                Some(span.clone()),
                            );
                        }
                        _ => {}
                    }
                }
            }

            // Recognize map.Map.<fn>(...) and lower to map intrinsics for the VM (MVP int->int maps)
            if let HirExpr::Member {
                object: map_mod,
                member: fname,
                ..
            } = &**callee
            {
                if let HirExpr::Member {
                    object: pkg,
                    member: mname,
                    ..
                } = &**map_mod
                {
                    if matches!(**pkg, HirExpr::Ident { ref name, .. } if name == "map")
                        && mname == "Map"
                    {
                        let name = match fname.as_str() {
                            "new" => Some("__arth_map_new"),
                            "put" => Some("__arth_map_put"),
                            "get" => Some("__arth_map_get"),
                            "len" => Some("__arth_map_len"),
                            "containsKey" => Some("__arth_map_contains_key"),
                            "containsValue" => Some("__arth_map_contains_value"),
                            "remove" => Some("__arth_map_remove"),
                            "clear" => Some("__arth_map_clear"),
                            "isEmpty" => Some("__arth_map_is_empty"),
                            "getOrDefault" => Some("__arth_map_get_or_default"),
                            "keys" => Some("__arth_map_keys"),
                            "values" => Some("__arth_map_values"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::new();
                            let take = match fname.as_str() {
                                "put" => 3,
                                "get" => 2,
                                "len" => 1,
                                "containsKey" => 2,
                                "containsValue" => 2,
                                "remove" => 2,
                                "clear" => 1,
                                "isEmpty" => 1,
                                "getOrDefault" => 3,
                                "keys" => 1,
                                "values" => 1,
                                _ => args.len(),
                            };
                            for a in args.iter().take(take) {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
                if let HirExpr::Ident { name: m, .. } = &**map_mod {
                    if m == "Map" {
                        let name = match fname.as_str() {
                            "new" => Some("__arth_map_new"),
                            "put" => Some("__arth_map_put"),
                            "get" => Some("__arth_map_get"),
                            "len" => Some("__arth_map_len"),
                            "containsKey" => Some("__arth_map_contains_key"),
                            "containsValue" => Some("__arth_map_contains_value"),
                            "remove" => Some("__arth_map_remove"),
                            "clear" => Some("__arth_map_clear"),
                            "isEmpty" => Some("__arth_map_is_empty"),
                            "getOrDefault" => Some("__arth_map_get_or_default"),
                            "keys" => Some("__arth_map_keys"),
                            "values" => Some("__arth_map_values"),
                            _ => None,
                        };
                        if let Some(intrin) = name {
                            let mut arg_vals: Vec<ir::Value> = Vec::new();
                            let take = match fname.as_str() {
                                "put" => 3,
                                "get" => 2,
                                "len" => 1,
                                "containsKey" => 2,
                                "containsValue" => 2,
                                "remove" => 2,
                                "clear" => 1,
                                "isEmpty" => 1,
                                "getOrDefault" => 3,
                                "keys" => 1,
                                "values" => 1,
                                _ => args.len(),
                            };
                            for a in args.iter().take(take) {
                                arg_vals.push(lower_expr(st, a));
                            }
                            return st.emit(
                                ir::InstKind::Call {
                                    name: intrin.to_string(),
                                    args: arg_vals,
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
            }

            // Recognize Optional.<fn>(...) and lower to optional intrinsics
            if let HirExpr::Member {
                object: opt_mod,
                member: fname,
                ..
            } = &**callee
            {
                let is_optional_module = match &**opt_mod {
                    HirExpr::Ident { name, .. } => name == "Optional",
                    HirExpr::Member { member: m, .. } => m == "Optional",
                    _ => false,
                };
                if is_optional_module {
                    let intrin = match fname.as_str() {
                        "some" => Some("__arth_opt_some"),
                        "none" => Some("__arth_opt_none"),
                        "isSome" => Some("__arth_opt_is_some"),
                        "isNone" => Some("__arth_opt_is_none"),
                        "unwrap" => Some("__arth_opt_unwrap"),
                        "orElse" => Some("__arth_opt_or_else"),
                        "getOrElse" => Some("__arth_opt_or_else"),
                        _ => None,
                    };
                    if let Some(intrin_name) = intrin {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        let take = match fname.as_str() {
                            "some" => 1,
                            "none" => 0,
                            "isSome" | "isNone" | "unwrap" => 1,
                            "orElse" | "getOrElse" => 2,
                            _ => args.len(),
                        };
                        for a in args.iter().take(take) {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin_name.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }

            // Recognize Json.stringify/parse and lower to JSON intrinsics
            // Patterns: encoding.json.Json.<fn> OR json.Json.<fn> OR Json.<fn>
            if let HirExpr::Member {
                object: json_mod,
                member: fname,
                ..
            } = &**callee
            {
                // Check for encoding.json.Json, json.Json, Json, or JSON (JavaScript style)
                let is_json_module = match &**json_mod {
                    HirExpr::Ident { name, .. } => name == "Json" || name == "JSON",
                    HirExpr::Member { member: m, .. } => m == "Json" || m == "JSON",
                    _ => false,
                };
                if is_json_module {
                    // Map JSON method names to intrinsics and their return types
                    let intrin: Option<(&str, ir::Ty)> = match fname.as_str() {
                        // Core serialization
                        "stringify" => Some(("__arth_json_stringify", ir::Ty::I64)),
                        "parse" => Some(("__arth_json_parse", ir::Ty::I64)),
                        // JSON value accessors (for accessing parsed JSON)
                        "getField" => Some(("__arth_json_get_field", ir::Ty::I64)),
                        "getIndex" => Some(("__arth_json_get_index", ir::Ty::I64)),
                        "getString" => Some(("__arth_json_get_string", ir::Ty::Ptr)), // returns string
                        "getNumber" => Some(("__arth_json_get_number", ir::Ty::F64)),
                        "getBool" => Some(("__arth_json_get_bool", ir::Ty::I64)),
                        "isNull" => Some(("__arth_json_is_null", ir::Ty::I64)),
                        "isObject" => Some(("__arth_json_is_object", ir::Ty::I64)),
                        "isArray" => Some(("__arth_json_is_array", ir::Ty::I64)),
                        "arrayLen" => Some(("__arth_json_array_len", ir::Ty::I64)),
                        "keys" => Some(("__arth_json_keys", ir::Ty::I64)),
                        _ => None,
                    };
                    if let Some((name, ret_ty)) = intrin {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: name.to_string(),
                                args: arg_vals,
                                ret: ret_ty,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }

            // Recognize <Type>JsonCodec.toJson/fromJson and lower to struct JSON intrinsics
            // Patterns: PointJsonCodec.toJson(obj), PointJsonCodec.fromJson(json)
            if let HirExpr::Member {
                object: codec_mod,
                member: fname,
                ..
            } = &**callee
            {
                if let HirExpr::Ident { name: mod_name, .. } = &**codec_mod {
                    if mod_name.ends_with("JsonCodec") {
                        let struct_name = mod_name.strip_suffix("JsonCodec").unwrap_or(mod_name);
                        // Look up field metadata from json_codec_structs index
                        // If found, use the enhanced metadata format; otherwise fall back to struct name
                        let field_meta = st
                            .json_codec_structs
                            .get(struct_name)
                            .map(|m| m.field_meta.clone())
                            .unwrap_or_else(|| struct_name.to_string());

                        if fname == "toJson" && args.len() == 1 {
                            // StructJsonCodec.toJson(obj) -> __arth_struct_to_json(obj, "name:idx,...")
                            let obj_val = lower_expr(st, &args[0]);
                            let meta_idx = intern_str(st, &field_meta);
                            let meta_val = st.emit(ir::InstKind::ConstStr(meta_idx), None);
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_struct_to_json".to_string(),
                                    args: vec![obj_val, meta_val],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                        if fname == "fromJson" && args.len() == 1 {
                            // StructJsonCodec.fromJson(json) -> __arth_json_to_struct(json, "name:idx,...;flags")
                            let json_val = lower_expr(st, &args[0]);
                            let meta_idx = intern_str(st, &field_meta);
                            let meta_val = st.emit(ir::InstKind::ConstStr(meta_idx), None);
                            return st.emit(
                                ir::InstKind::Call {
                                    name: "__arth_json_to_struct".to_string(),
                                    args: vec![json_val, meta_val],
                                    ret: ir::Ty::I64,
                                },
                                Some(span.clone()),
                            );
                        }
                    }
                }
            }

            // Recognize io.Files.<fn>(...) or Files.<fn>(...) and lower to I/O intrinsics
            if let HirExpr::Member {
                object: io_mod,
                member: fname,
                ..
            } = &**callee
            {
                // Check for io.Files or just Files
                let is_files_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Files",
                    HirExpr::Member { member: m, .. } => m == "Files",
                    _ => false,
                };
                if is_files_module {
                    let name = match fname.as_str() {
                        "open" => Some("__arth_file_open"),
                        "close" => Some("__arth_file_close"),
                        "read" => Some("__arth_file_read"),
                        "readAll" => Some("__arth_file_read_all"),
                        "readText" => Some("__arth_file_read_all"),
                        "write" => Some("__arth_file_write"),
                        "flush" => Some("__arth_file_flush"),
                        "seek" => Some("__arth_file_seek"),
                        "size" => Some("__arth_file_size"),
                        "exists" => Some("__arth_file_exists"),
                        "delete" => Some("__arth_file_delete"),
                        "copy" => Some("__arth_file_copy"),
                        "move" => Some("__arth_file_move"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for io.fs.Fs, io.fs.Paths, or just Fs, Paths
                let is_fs_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Fs",
                    HirExpr::Member { member: m, .. } => m == "Fs",
                    _ => false,
                };
                if is_fs_module {
                    let name = match fname.as_str() {
                        "createDir" => Some("__arth_dir_create"),
                        "createDirAll" => Some("__arth_dir_create_all"),
                        "deleteDir" => Some("__arth_dir_delete"),
                        "list" => Some("__arth_dir_list"),
                        "exists" => Some("__arth_dir_exists"),
                        "isDir" => Some("__arth_is_dir"),
                        "isFile" => Some("__arth_is_file"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                let is_paths_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Paths",
                    HirExpr::Member { member: m, .. } => m == "Paths",
                    _ => false,
                };
                if is_paths_module {
                    let name = match fname.as_str() {
                        "join" => Some("__arth_path_join"),
                        "parent" => Some("__arth_path_parent"),
                        "filename" => Some("__arth_path_filename"),
                        "extension" => Some("__arth_path_extension"),
                        "absolute" => Some("__arth_path_absolute"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64, // Strings are returned as handles/values
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for io.Console or Console
                let is_console_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Console",
                    HirExpr::Member { member: m, .. } => m == "Console",
                    _ => false,
                };
                if is_console_module {
                    let name = match fname.as_str() {
                        "readLine" => Some("__arth_console_read_line"),
                        "write" => Some("__arth_console_write"),
                        "writeln" => Some("__arth_console_writeln"),
                        "writeErr" => Some("__arth_console_write_err"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64, // All returns are I64 (handles or status codes)
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for time.DateTimes or DateTimes
                let is_datetimes_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "DateTimes",
                    HirExpr::Member { member: m, .. } => m == "DateTimes",
                    _ => false,
                };
                if is_datetimes_module {
                    let name = match fname.as_str() {
                        "now" => Some("__arth_datetime_now"),
                        "fromMillis" => Some("__arth_datetime_from_millis"),
                        "toMillis" => Some("__arth_datetime_to_millis"),
                        "year" => Some("__arth_datetime_year"),
                        "month" => Some("__arth_datetime_month"),
                        "day" => Some("__arth_datetime_day"),
                        "hour" => Some("__arth_datetime_hour"),
                        "minute" => Some("__arth_datetime_minute"),
                        "second" => Some("__arth_datetime_second"),
                        "dayOfWeek" => Some("__arth_datetime_day_of_week"),
                        "dayOfYear" => Some("__arth_datetime_day_of_year"),
                        "add" => Some("__arth_datetime_add"),
                        "sub" | "subtract" => Some("__arth_datetime_sub"),
                        "compare" => Some("__arth_datetime_compare"),
                        "parse" => Some("__arth_datetime_parse"),
                        "format" => Some("__arth_datetime_format"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for time.Durations or Durations
                let is_durations_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Durations",
                    HirExpr::Member { member: m, .. } => m == "Durations",
                    _ => false,
                };
                if is_durations_module {
                    let name = match fname.as_str() {
                        "fromMillis" => Some("__arth_duration_from_millis"),
                        "fromSeconds" | "fromSecs" => Some("__arth_duration_from_secs"),
                        "fromMinutes" | "fromMins" => Some("__arth_duration_from_mins"),
                        "fromHours" => Some("__arth_duration_from_hours"),
                        "fromDays" => Some("__arth_duration_from_days"),
                        "toMillis" => Some("__arth_duration_to_millis"),
                        "toSeconds" | "toSecs" => Some("__arth_duration_to_secs"),
                        "add" => Some("__arth_duration_add"),
                        "sub" | "subtract" => Some("__arth_duration_sub"),
                        "mul" | "multiply" => Some("__arth_duration_mul"),
                        "div" | "divide" => Some("__arth_duration_div"),
                        "compare" => Some("__arth_duration_compare"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for time.Instants or Instants
                let is_instants_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Instants",
                    HirExpr::Member { member: m, .. } => m == "Instants",
                    _ => false,
                };
                if is_instants_module {
                    let name = match fname.as_str() {
                        "now" => Some("__arth_instant_now"),
                        "elapsed" => Some("__arth_instant_elapsed"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for numeric.BigDecimals or BigDecimals
                let is_bigdecimals_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "BigDecimals",
                    HirExpr::Member { member: m, .. } => m == "BigDecimals",
                    _ => false,
                };
                if is_bigdecimals_module {
                    let name = match fname.as_str() {
                        "new" | "fromString" => Some("__arth_bigdecimal_new"),
                        "fromInt" => Some("__arth_bigdecimal_from_int"),
                        "fromFloat" => Some("__arth_bigdecimal_from_float"),
                        "add" => Some("__arth_bigdecimal_add"),
                        "sub" | "subtract" => Some("__arth_bigdecimal_sub"),
                        "mul" | "multiply" => Some("__arth_bigdecimal_mul"),
                        "div" | "divide" => Some("__arth_bigdecimal_div"),
                        "rem" | "remainder" => Some("__arth_bigdecimal_rem"),
                        "pow" | "power" => Some("__arth_bigdecimal_pow"),
                        "abs" => Some("__arth_bigdecimal_abs"),
                        "negate" => Some("__arth_bigdecimal_negate"),
                        "compare" => Some("__arth_bigdecimal_compare"),
                        "toString" => Some("__arth_bigdecimal_to_string"),
                        "toInt" => Some("__arth_bigdecimal_to_int"),
                        "toFloat" => Some("__arth_bigdecimal_to_float"),
                        "scale" => Some("__arth_bigdecimal_scale"),
                        "setScale" => Some("__arth_bigdecimal_set_scale"),
                        "round" => Some("__arth_bigdecimal_round"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for numeric.BigInts or BigInts
                let is_bigints_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "BigInts",
                    HirExpr::Member { member: m, .. } => m == "BigInts",
                    _ => false,
                };
                if is_bigints_module {
                    let name = match fname.as_str() {
                        "new" | "fromString" => Some("__arth_bigint_new"),
                        "fromInt" => Some("__arth_bigint_from_int"),
                        "add" => Some("__arth_bigint_add"),
                        "sub" | "subtract" => Some("__arth_bigint_sub"),
                        "mul" | "multiply" => Some("__arth_bigint_mul"),
                        "div" | "divide" => Some("__arth_bigint_div"),
                        "rem" | "remainder" => Some("__arth_bigint_rem"),
                        "pow" | "power" => Some("__arth_bigint_pow"),
                        "abs" => Some("__arth_bigint_abs"),
                        "negate" => Some("__arth_bigint_negate"),
                        "compare" => Some("__arth_bigint_compare"),
                        "toString" => Some("__arth_bigint_to_string"),
                        "toInt" => Some("__arth_bigint_to_int"),
                        "gcd" => Some("__arth_bigint_gcd"),
                        "modPow" => Some("__arth_bigint_mod_pow"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for concurrent.Executor or Executor
                let is_executor_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Executor",
                    HirExpr::Member { member: m, .. } => m == "Executor",
                    _ => false,
                };
                if is_executor_module {
                    let name = match fname.as_str() {
                        "init" => Some("__arth_executor_init"),
                        "threadCount" => Some("__arth_executor_thread_count"),
                        "activeWorkers" => Some("__arth_executor_active_workers"),
                        "spawn" => Some("__arth_executor_spawn"),
                        "cancel" if st.use_native_structs => Some("__arth_executor_cancel"),
                        "join" => Some("__arth_executor_join"),
                        // C02 work-stealing stats intrinsics
                        "spawnWithArg" => Some("__arth_executor_spawn_with_arg"),
                        "activeExecutorCount" => Some("__arth_executor_active_executor_count"),
                        "workerTaskCount" => Some("__arth_executor_worker_task_count"),
                        "resetStats" => Some("__arth_executor_reset_stats"),
                        // C04 task suspension intrinsic
                        "spawnAwait" => Some("__arth_executor_spawn_await"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for MpmcChan (C06 - MPMC channels)
                let is_mpmc_chan_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "MpmcChan",
                    HirExpr::Member { member: m, .. } => m == "MpmcChan",
                    _ => false,
                };
                if is_mpmc_chan_module {
                    let name = match fname.as_str() {
                        // C06: Basic MPMC channel operations
                        "create" => Some("__arth_mpmc_chan_create"),
                        "send" => Some("__arth_mpmc_chan_send"),
                        "sendBlocking" => Some("__arth_mpmc_chan_send_blocking"),
                        "recv" => Some("__arth_mpmc_chan_recv"),
                        "recvBlocking" => Some("__arth_mpmc_chan_recv_blocking"),
                        "close" => Some("__arth_mpmc_chan_close"),
                        "len" => Some("__arth_mpmc_chan_len"),
                        "empty" => Some("__arth_mpmc_chan_is_empty"),
                        "isFull" => Some("__arth_mpmc_chan_is_full"),
                        "isClosed" => Some("__arth_mpmc_chan_is_closed"),
                        "capacity" => Some("__arth_mpmc_chan_capacity"),
                        // C07: Executor-integrated operations
                        "sendWithTask" => Some("__arth_mpmc_chan_send_with_task"),
                        "recvWithTask" => Some("__arth_mpmc_chan_recv_with_task"),
                        "recvAndWake" => Some("__arth_mpmc_chan_recv_and_wake"),
                        "popWaitingSender" => Some("__arth_mpmc_chan_pop_waiting_sender"),
                        "getWaitingSenderValue" => {
                            Some("__arth_mpmc_chan_get_waiting_sender_value")
                        }
                        "popWaitingReceiver" => Some("__arth_mpmc_chan_pop_waiting_receiver"),
                        "waitingSenderCount" => Some("__arth_mpmc_chan_waiting_sender_count"),
                        "waitingReceiverCount" => Some("__arth_mpmc_chan_waiting_receiver_count"),
                        "getWokenSender" => Some("__arth_mpmc_chan_get_woken_sender"),
                        // C08: Blocking receive operations
                        "sendAndWake" => Some("__arth_mpmc_chan_send_and_wake"),
                        "getWokenReceiver" => Some("__arth_mpmc_chan_get_woken_receiver"),
                        // C09: Channel select operations
                        "selectClear" => Some("__arth_mpmc_chan_select_clear"),
                        "selectAdd" => Some("__arth_mpmc_chan_select_add"),
                        "selectCount" => Some("__arth_mpmc_chan_select_count"),
                        "trySelectRecv" => Some("__arth_mpmc_chan_try_select_recv"),
                        "selectRecvBlocking" => Some("__arth_mpmc_chan_select_recv_blocking"),
                        "selectRecvWithTask" => Some("__arth_mpmc_chan_select_recv_with_task"),
                        "selectGetReadyIndex" => Some("__arth_mpmc_chan_select_get_ready_index"),
                        "selectGetValue" => Some("__arth_mpmc_chan_select_get_value"),
                        "selectDeregister" => Some("__arth_mpmc_chan_select_deregister"),
                        "selectGetHandle" => Some("__arth_mpmc_chan_select_get_handle"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for Actor module (C11 - Actor = Task + Channel)
                let is_actor_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Actor",
                    HirExpr::Member { member: m, .. } => m == "Actor",
                    _ => false,
                };
                if is_actor_module {
                    let name = match fname.as_str() {
                        // C11: Actor operations
                        "create" => Some("__arth_actor_create"),
                        "spawn" => Some("__arth_actor_spawn"),
                        "send" => Some("__arth_actor_send"),
                        "sendBlocking" => Some("__arth_actor_send_blocking"),
                        "recv" => Some("__arth_actor_recv"),
                        "recvBlocking" => Some("__arth_actor_recv_blocking"),
                        "close" => Some("__arth_actor_close"),
                        "stop" => Some("__arth_actor_stop"),
                        "getTask" => Some("__arth_actor_get_task"),
                        "getMailbox" => Some("__arth_actor_get_mailbox"),
                        "isRunning" => Some("__arth_actor_is_running"),
                        "getState" => Some("__arth_actor_get_state"),
                        "messageCount" => Some("__arth_actor_message_count"),
                        "mailboxEmpty" => Some("__arth_actor_mailbox_empty"),
                        "mailboxLen" => Some("__arth_actor_mailbox_len"),
                        "setTask" => Some("__arth_actor_set_task"),
                        "markStopped" => Some("__arth_actor_mark_stopped"),
                        "markFailed" => Some("__arth_actor_mark_failed"),
                        "isFailed" => Some("__arth_actor_is_failed"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for Atomic module (C19 - Atomic<T> operations)
                let is_atomic_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Atomic",
                    HirExpr::Member { member: m, .. } => m == "Atomic",
                    _ => false,
                };
                if is_atomic_module {
                    let name = match fname.as_str() {
                        // C19: Atomic<T> operations
                        "create" => Some("__arth_atomic_create"),
                        "new" => Some("__arth_atomic_create"),
                        "load" => Some("__arth_atomic_load"),
                        "store" => Some("__arth_atomic_store"),
                        "cas" => Some("__arth_atomic_cas"),
                        "compareAndSwap" => Some("__arth_atomic_cas"),
                        "fetchAdd" => Some("__arth_atomic_fetch_add"),
                        "fetchSub" => Some("__arth_atomic_fetch_sub"),
                        "swap" => Some("__arth_atomic_swap"),
                        "get" => Some("__arth_atomic_get"),
                        "set" => Some("__arth_atomic_set"),
                        "inc" => Some("__arth_atomic_inc"),
                        "increment" => Some("__arth_atomic_inc"),
                        "dec" => Some("__arth_atomic_dec"),
                        "decrement" => Some("__arth_atomic_dec"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for Task module primitives (sleep/yield/cancel-check)
                let is_task_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Task",
                    HirExpr::Member { member: m, .. } => m == "Task",
                    _ => false,
                };
                if is_task_module {
                    let name = match fname.as_str() {
                        "sleep" => Some("__arth_timer_sleep"),
                        "yield" => Some("__arth_task_yield"),
                        "checkCancel" => Some("__arth_task_check_cancelled"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for EventLoop module (C21 - Event Loop operations)
                let is_event_loop_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "EventLoop",
                    HirExpr::Member { member: m, .. } => m == "EventLoop",
                    _ => false,
                };
                if is_event_loop_module {
                    let name = match fname.as_str() {
                        // C21: Event Loop operations
                        "create" => Some("__arth_event_loop_create"),
                        "new" => Some("__arth_event_loop_create"),
                        "registerTimer" => Some("__arth_event_loop_register_timer"),
                        "registerFd" => Some("__arth_event_loop_register_fd"),
                        "deregister" => Some("__arth_event_loop_deregister"),
                        "poll" => Some("__arth_event_loop_poll"),
                        "getEvent" => Some("__arth_event_loop_get_event"),
                        "getEventType" => Some("__arth_event_loop_get_event_type"),
                        "close" => Some("__arth_event_loop_close"),
                        "pipeCreate" => Some("__arth_event_loop_pipe_create"),
                        "pipeGetWriteFd" => Some("__arth_event_loop_pipe_get_write_fd"),
                        "pipeWrite" => Some("__arth_event_loop_pipe_write"),
                        "pipeRead" => Some("__arth_event_loop_pipe_read"),
                        "pipeClose" => Some("__arth_event_loop_pipe_close"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for Timer module (C22 - Async Timer operations)
                let is_timer_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "Timer",
                    HirExpr::Member { member: m, .. } => m == "Timer",
                    _ => false,
                };
                if is_timer_module {
                    let name = match fname.as_str() {
                        // C22: Timer operations
                        "sleep" => Some("__arth_timer_sleep"),
                        "sleepBlocking" => Some("__arth_timer_sleep"),
                        "sleepAsync" => Some("__arth_timer_sleep_async"),
                        "checkExpired" => Some("__arth_timer_check_expired"),
                        "getWaitingTask" => Some("__arth_timer_get_waiting_task"),
                        "cancel" => Some("__arth_timer_cancel"),
                        "pollExpired" => Some("__arth_timer_poll_expired"),
                        "now" => Some("__arth_timer_now"),
                        "elapsed" => Some("__arth_timer_elapsed"),
                        "remove" => Some("__arth_timer_remove"),
                        "remaining" => Some("__arth_timer_remaining"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for TcpListener module (C23 - Async TCP operations)
                let is_tcp_listener_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "TcpListener",
                    HirExpr::Member { member: m, .. } => m == "TcpListener",
                    _ => false,
                };
                if is_tcp_listener_module {
                    let name = match fname.as_str() {
                        // C23: TcpListener operations
                        "bind" => Some("__arth_tcp_listener_bind"),
                        "accept" => Some("__arth_tcp_listener_accept"),
                        "acceptBlocking" => Some("__arth_tcp_listener_accept"),
                        "acceptAsync" => Some("__arth_tcp_listener_accept_async"),
                        "close" => Some("__arth_tcp_listener_close"),
                        "localPort" => Some("__arth_tcp_listener_local_port"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for TcpStream module (C23 - Async TCP operations)
                let is_tcp_stream_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "TcpStream",
                    HirExpr::Member { member: m, .. } => m == "TcpStream",
                    _ => false,
                };
                if is_tcp_stream_module {
                    let name = match fname.as_str() {
                        // C23: TcpStream operations
                        "connect" => Some("__arth_tcp_stream_connect"),
                        "connectBlocking" => Some("__arth_tcp_stream_connect"),
                        "connectAsync" => Some("__arth_tcp_stream_connect_async"),
                        "read" => Some("__arth_tcp_stream_read"),
                        "readBlocking" => Some("__arth_tcp_stream_read"),
                        "readAsync" => Some("__arth_tcp_stream_read_async"),
                        "write" => Some("__arth_tcp_stream_write"),
                        "writeBlocking" => Some("__arth_tcp_stream_write"),
                        "writeAsync" => Some("__arth_tcp_stream_write_async"),
                        "close" => Some("__arth_tcp_stream_close"),
                        "getLastRead" => Some("__arth_tcp_stream_get_last_read_string"),
                        "setTimeout" => Some("__arth_tcp_stream_set_timeout"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for TcpAsync module (C23 - Async request management)
                let is_tcp_async_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "TcpAsync",
                    HirExpr::Member { member: m, .. } => m == "TcpAsync",
                    _ => false,
                };
                if is_tcp_async_module {
                    let name = match fname.as_str() {
                        // C23: Async request management
                        "checkReady" => Some("__arth_tcp_check_ready"),
                        "getResult" => Some("__arth_tcp_get_result"),
                        "pollReady" => Some("__arth_tcp_poll_ready"),
                        "removeRequest" => Some("__arth_tcp_remove_request"),
                        "getWaitingTask" => Some("__arth_tcp_get_waiting_task"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpClient module (C24 - HTTP Client operations)
                let is_http_client_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpClient",
                    HirExpr::Member { member: m, .. } => m == "HttpClient",
                    _ => false,
                };
                if is_http_client_module {
                    let name = match fname.as_str() {
                        // C24: HTTP Client operations
                        "get" => Some("__arth_http_get"),
                        "post" => Some("__arth_http_post"),
                        "getAsync" => Some("__arth_http_get_async"),
                        "postAsync" => Some("__arth_http_post_async"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpResponse module (C24 - HTTP Response operations)
                let is_http_response_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpResponse",
                    HirExpr::Member { member: m, .. } => m == "HttpResponse",
                    _ => false,
                };
                if is_http_response_module {
                    let name = match fname.as_str() {
                        // C24: HTTP Response operations
                        "status" => Some("__arth_http_response_status"),
                        "header" => Some("__arth_http_response_header"),
                        "body" => Some("__arth_http_response_body"),
                        "close" => Some("__arth_http_response_close"),
                        "bodyLength" => Some("__arth_http_get_body_length"),
                        "headerCount" => Some("__arth_http_get_header_count"),
                        "getLastHeader" => Some("__arth_http_get_last_header_string"),
                        "getLastBody" => Some("__arth_http_get_last_body_string"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpAsync module (C24 - HTTP Async request management)
                let is_http_async_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpAsync",
                    HirExpr::Member { member: m, .. } => m == "HttpAsync",
                    _ => false,
                };
                if is_http_async_module {
                    let name = match fname.as_str() {
                        // C24: Async HTTP request management
                        "checkReady" => Some("__arth_http_check_ready"),
                        "getResult" => Some("__arth_http_get_result"),
                        "pollReady" => Some("__arth_http_poll_ready"),
                        "removeRequest" => Some("__arth_http_remove_request"),
                        "getWaitingTask" => Some("__arth_http_get_waiting_task"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpServer module (C25 - HTTP Server operations)
                let is_http_server_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpServer",
                    HirExpr::Member { member: m, .. } => m == "HttpServer",
                    _ => false,
                };
                if is_http_server_module {
                    let name = match fname.as_str() {
                        // C25: HTTP Server operations
                        "create" => Some("__arth_http_server_create"),
                        "close" => Some("__arth_http_server_close"),
                        "getPort" => Some("__arth_http_server_get_port"),
                        "accept" => Some("__arth_http_server_accept"),
                        "acceptAsync" => Some("__arth_http_server_accept_async"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpRequest module (C25 - HTTP Request reading)
                let is_http_request_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpRequest",
                    HirExpr::Member { member: m, .. } => m == "HttpRequest",
                    _ => false,
                };
                if is_http_request_module {
                    let name = match fname.as_str() {
                        // C25: HTTP Request operations
                        "method" => Some("__arth_http_request_method"),
                        "path" => Some("__arth_http_request_path"),
                        "header" => Some("__arth_http_request_header"),
                        "body" => Some("__arth_http_request_body"),
                        "headerCount" => Some("__arth_http_request_header_count"),
                        "bodyLength" => Some("__arth_http_request_body_length"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpWriter module (C25 - HTTP Response writing)
                let is_http_writer_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpWriter",
                    HirExpr::Member { member: m, .. } => m == "HttpWriter",
                    _ => false,
                };
                if is_http_writer_module {
                    let name = match fname.as_str() {
                        // C25: HTTP Writer operations
                        "status" => Some("__arth_http_writer_status"),
                        "header" => Some("__arth_http_writer_header"),
                        "body" => Some("__arth_http_writer_body"),
                        "send" => Some("__arth_http_writer_send"),
                        "sendAsync" => Some("__arth_http_writer_send_async"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }

                // Check for HttpServerAsync module (C25 - HTTP Server async management)
                let is_http_server_async_module = match &**io_mod {
                    HirExpr::Ident { name, .. } => name == "HttpServerAsync",
                    HirExpr::Member { member: m, .. } => m == "HttpServerAsync",
                    _ => false,
                };
                if is_http_server_async_module {
                    let name = match fname.as_str() {
                        // C25: HTTP Server async operations
                        "checkReady" => Some("__arth_http_server_check_ready"),
                        "getResult" => Some("__arth_http_server_get_result"),
                        "pollReady" => Some("__arth_http_server_poll_ready"),
                        "removeRequest" => Some("__arth_http_server_remove_request"),
                        "getWaitingTask" => Some("__arth_http_server_get_waiting_task"),
                        _ => None,
                    };
                    if let Some(intrin) = name {
                        let mut arg_vals: Vec<ir::Value> = Vec::new();
                        for a in args {
                            arg_vals.push(lower_expr(st, a));
                        }
                        return st.emit(
                            ir::InstKind::Call {
                                name: intrin.to_string(),
                                args: arg_vals,
                                ret: ir::Ty::I64,
                            },
                            Some(span.clone()),
                        );
                    }
                }
            }

            // Fallback: value-style and direct calls.
            //
            // If the callee is a *value expression* (e.g., a local variable of function type,
            // a lambda literal, or any computed expression), lower to an indirect
            // `ClosureCall`. This matches typechecker semantics where calls to
            // function-typed values are allowed.
            //
            // Otherwise, for simple identifiers or module-style members (Module.func or
            // pkg.Module.func), lower to a direct `Call`/`Invoke` using the function name.
            let mut arg_vals: Vec<ir::Value> = Vec::with_capacity(args.len());
            for a in args {
                arg_vals.push(lower_expr(st, a));
            }

            // Value-style call: non-name callee, or a local identifier.
            match &**callee {
                // Local function-typed value: call via closure.
                HirExpr::Ident { name, .. } if st.locals.contains_key(name) => {
                    let closure_val = lower_expr(st, callee);
                    return st.emit(
                        ir::InstKind::ClosureCall {
                            closure: closure_val,
                            args: arg_vals,
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
                // Any non-name callee (lambda literal, conditional, call result, indexing, etc.)
                // is treated as a computed closure handle.
                HirExpr::Ident { .. } | HirExpr::Member { .. } => {
                    // Fall through to direct-call lowering below.
                }
                _ => {
                    let closure_val = lower_expr(st, callee);
                    return st.emit(
                        ir::InstKind::ClosureCall {
                            closure: closure_val,
                            args: arg_vals,
                            ret: ir::Ty::I64,
                        },
                        Some(span.clone()),
                    );
                }
            }

            // Direct call: simple identifier or module-style member.
            let name = match &**callee {
                HirExpr::Ident { name, .. } => name.clone(),
                HirExpr::Member { object, member, .. } => {
                    // Reconstruct fully-qualified name: Module.method
                    // This is needed for module function calls like StringFns.getName()
                    if let HirExpr::Ident { name: obj_name, .. } = &**object {
                        format!("{}.{}", obj_name, member)
                    } else {
                        // Nested member access or complex expression - just use member name
                        member.clone()
                    }
                }
                _ => "func".to_string(),
            };

            // Check if this is a call to an extern function (FFI)
            if let Some(sig) = st.extern_funcs.get(&name).cloned() {
                // Extern calls use C calling convention and don't support Arth exceptions.
                // They bypass the try/invoke mechanism entirely.
                let result = st.emit(
                    ir::InstKind::ExternCall {
                        name: name.clone(),
                        args: arg_vals,
                        params: sig.params.clone(),
                        ret: sig.ret.clone(),
                    },
                    Some(span.clone()),
                );

                // If @ffi_owned, register the returned value for cleanup
                if sig.return_ownership == FfiOwnership::Owned && sig.ret != ir::Ty::Void {
                    // Get type name for drop resolution
                    let ty_name = sig
                        .ret_type_name
                        .clone()
                        .unwrap_or_else(|| "FFI".to_string());

                    // Allocate a slot, store the value, and register for drop
                    let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));
                    st.emit(ir::InstKind::Store(slot, result), Some(span.clone()));

                    // Generate a unique name for this FFI value and register for cleanup
                    let ffi_var_name = format!("__ffi_owned_{}_{}", name, result.0);
                    st.register_ffi_owned_drop(&ffi_var_name, slot, &ty_name);

                    // Return a load from the slot (the value is now owned by the slot)
                    st.emit(ir::InstKind::Load(slot), Some(span.clone()))
                } else {
                    result
                }
            } else if let Some(unwind) = st.unwind_target {
                // Use invoke: end current block and continue at a fresh normal block
                let res = st.fresh_value();
                let normal = st.new_block("after_invoke", Some(span.clone()));
                st.set_term(ir::Terminator::Invoke {
                    callee: name,
                    args: arg_vals,
                    ret: ir::Ty::I64,
                    result: Some(res),
                    normal: ir::Block(normal as u32),
                    unwind: ir::Block(unwind as u32),
                });
                st.set_cur(normal);
                res
            } else {
                st.emit(
                    ir::InstKind::Call {
                        name,
                        args: arg_vals,
                        ret: ir::Ty::I64,
                    },
                    Some(span.clone()),
                )
            }
        }
        HirExpr::Member { object, member, .. } => {
            // Support enum variant value as heap handle when used without call: Enum.Variant
            if let HirExpr::Ident {
                name: enum_name, ..
            } = &**object
            {
                let looks_type = enum_name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false);
                if looks_type {
                    let tag_opt: Option<i64> = st
                        .enum_tags
                        .as_ref()
                        .and_then(|tags| tags.get(enum_name))
                        .and_then(|vmap| vmap.get(member).copied());
                    if let Some(tag) = tag_opt {
                        // Create native enum instance for unit variant
                        let enum_name_ix = intern_str(st, enum_name);
                        let variant_name_ix = intern_str(st, member);
                        let enum_name_val = st.emit(ir::InstKind::ConstStr(enum_name_ix), None);
                        let variant_name_val =
                            st.emit(ir::InstKind::ConstStr(variant_name_ix), None);
                        let tag_val = st.emit(ir::InstKind::ConstI64(tag), None);
                        let payload_count = st.emit(ir::InstKind::ConstI64(0), None);

                        return st.emit(
                            ir::InstKind::Call {
                                name: "__arth_enum_new".to_string(),
                                args: vec![enum_name_val, variant_name_val, tag_val, payload_count],
                                ret: ir::Ty::I64,
                            },
                            None,
                        );
                    }
                }
            }
            // Provider singleton field access: ProviderName.field
            // Only applies when object is an identifier that is a provider type name
            if let HirExpr::Ident {
                name: obj_ident, ..
            } = &**object
            {
                if st.provider_names.contains(obj_ident) {
                    return st.emit(
                        ir::InstKind::ProviderFieldGet {
                            obj: ir::Value(0), // Singleton/global access
                            provider: obj_ident.clone(),
                            field: member.clone(),
                        },
                        None,
                    );
                }
            }

            // Check if the object expression evaluates to a provider type.
            // This handles both simple identifiers (c.count) and nested access (bundle.counter.value).
            if let Some(pname) = st.get_expr_provider_type(object) {
                let obj_val = lower_expr(st, object);
                return st.emit(
                    ir::InstKind::ProviderFieldGet {
                        obj: obj_val,
                        provider: pname,
                        field: member.clone(),
                    },
                    None,
                );
            }

            // Special case: `.length` on string types should use StrLen
            if member == "length" && st.is_expr_string_type(object) {
                let obj_val = lower_expr(st, object);
                return st.emit(
                    ir::InstKind::Call {
                        name: "__arth_str_len".to_string(),
                        args: vec![obj_val],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
            }

            // Non-provider instance field access.
            // In native mode, use direct typed field access for known structs.
            if st.use_native_structs
                && let Some(struct_name) = st.get_expr_type_name(object)
                && st.struct_defs.contains_key(&struct_name)
                && let Some(field_index) = st.get_field_index(&struct_name, member)
            {
                let obj_val = lower_expr(st, object);
                return st.emit(
                    ir::InstKind::StructFieldGet {
                        ptr: obj_val,
                        type_name: struct_name,
                        field_name: member.clone(),
                        field_index,
                    },
                    None,
                );
            }

            // Fallback: dynamic runtime field get.
            let obj_val = lower_expr(st, object);
            let field_ix = intern_str(st, member);
            let field_key = st.emit(ir::InstKind::ConstStr(field_ix), None);
            st.emit(
                ir::InstKind::Call {
                    name: "__arth_struct_get_named".to_string(),
                    args: vec![obj_val, field_key],
                    ret: ir::Ty::I64,
                },
                None,
            )
        }
        HirExpr::OptionalMember {
            object,
            member,
            span,
            ..
        } => {
            // Optional chaining: obj?.field
            // If obj is Some(x), returns Some(x.field); if obj is None, returns None

            // 1) Allocate a temp slot for the result (Optional<FieldType>)
            let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));

            // 2) Evaluate the object (should be Optional<T>)
            let obj_val = lower_expr(st, object);

            // 3) Check if the optional is Some using __arth_opt_is_some
            let is_some_i64 = st.emit(
                ir::InstKind::Call {
                    name: "__arth_opt_is_some".to_string(),
                    args: vec![obj_val],
                    ret: ir::Ty::I64,
                },
                None,
            );
            // Convert I64 result (0 or 1) to Bool for CondBr
            let zero = st.emit(ir::InstKind::ConstI64(0), None);
            let is_some = st.emit(ir::InstKind::Cmp(ir::CmpPred::Ne, is_some_i64, zero), None);

            // 4) Branch: if Some, access field and wrap; if None, return None
            let then_b = st.new_block("optchain_some", Some(span.clone()));
            let else_b = st.new_block("optchain_none", Some(span.clone()));
            let join_b = st.new_block("optchain_join", Some(span.clone()));

            let cur = st.cur;
            st.blocks[cur].term = ir::Terminator::CondBr {
                cond: is_some,
                then_bb: ir::Block(then_b as u32),
                else_bb: ir::Block(else_b as u32),
            };

            // Then path (Some): unwrap, access field, wrap result in Some
            st.set_cur(then_b);
            // Unwrap the optional to get the inner value
            let unwrapped = st.emit(
                ir::InstKind::Call {
                    name: "__arth_opt_unwrap".to_string(),
                    args: vec![obj_val],
                    ret: ir::Ty::I64,
                },
                None,
            );
            // Access the field
            let field_ix = intern_str(st, member);
            let field_key = st.emit(ir::InstKind::ConstStr(field_ix), None);
            let field_val = st.emit(
                ir::InstKind::Call {
                    name: "__arth_struct_get_named".to_string(),
                    args: vec![unwrapped, field_key],
                    ret: ir::Ty::I64,
                },
                None,
            );
            // Wrap field value in Some
            let result_some = st.emit(
                ir::InstKind::Call {
                    name: "__arth_opt_some".to_string(),
                    args: vec![field_val],
                    ret: ir::Ty::I64,
                },
                None,
            );
            let _ = st.emit(ir::InstKind::Store(slot, result_some), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // Else path (None): return None
            st.set_cur(else_b);
            let result_none = st.emit(
                ir::InstKind::Call {
                    name: "__arth_opt_none".to_string(),
                    args: vec![],
                    ret: ir::Ty::I64,
                },
                None,
            );
            let _ = st.emit(ir::InstKind::Store(slot, result_none), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // Join block: load the result
            st.set_cur(join_b);
            st.emit(ir::InstKind::Load(slot), Some(span.clone()))
        }
        HirExpr::Index { object, index, .. } => {
            // Implement proper indexing
            let obj_val = lower_expr(st, object);
            let idx_val = lower_expr(st, index);
            // Call runtime intrinsic for indexing
            st.emit(
                ir::InstKind::Call {
                    name: "__arth_list_get".to_string(),
                    args: vec![obj_val, idx_val],
                    ret: ir::Ty::I64,
                },
                None,
            )
        }
        HirExpr::ListLit { elements, span, .. } => {
            // Create a new list
            let list_handle = st.emit(
                ir::InstKind::Call {
                    name: "__arth_list_new".to_string(),
                    args: vec![],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );

            // Push each element
            for elem in elements {
                let elem_val = lower_expr(st, elem);
                let _ = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_list_push".to_string(),
                        args: vec![list_handle, elem_val],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
            }

            list_handle
        }
        HirExpr::MapLit {
            pairs,
            spread,
            span,
            ..
        } => {
            // Create a new map
            let map_handle = st.emit(
                ir::InstKind::Call {
                    name: "__arth_map_new".to_string(),
                    args: vec![],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );

            // If spread is present, merge fields from spread source into new map
            if let Some(spread_expr) = spread {
                let spread_handle = lower_expr(st, spread_expr);
                let _ = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_map_merge".to_string(),
                        args: vec![map_handle, spread_handle],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
            }

            // Put each explicit key-value pair (these override spread fields)
            for (key, value) in pairs {
                // For struct literals, keys are field name identifiers - convert to string keys
                let key_val = if let HirExpr::Ident { name, .. } = key {
                    // Intern the field name as a string key
                    let key_ix = intern_str(st, name);
                    st.emit(ir::InstKind::ConstStr(key_ix), None)
                } else {
                    lower_expr(st, key)
                };
                let val_val = lower_expr(st, value);
                let _ = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_map_put".to_string(),
                        args: vec![map_handle, key_val, val_val],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
            }

            map_handle
        }

        HirExpr::Lambda {
            params,
            body,
            captures,
            span,
            ret,
            ..
        } => {
            // Generate a unique function name for this lambda
            let lambda_id = st.next_val; // Use next value as unique ID
            let func_name = format!("__lambda_{}", lambda_id);

            // Generate a separate IR function for the lambda body
            // Lambda function parameters: first come captures, then the lambda's own params
            let mut lambda_st = LowerState::default();
            lambda_st.enum_tags = st.enum_tags.clone();
            lambda_st.shared_field_names = st.shared_field_names.clone();
            lambda_st.type_aliases = st.type_aliases.clone();
            lambda_st.json_codec_structs = st.json_codec_structs.clone();
            // Inherit native struct settings from parent
            lambda_st.use_native_structs = st.use_native_structs;
            lambda_st.struct_defs = st.struct_defs.clone();

            let entry = lambda_st.new_block("entry", None);
            lambda_st.cur = entry;

            // Map HIR types to IR types
            fn map_hir_ty_to_ir_local(
                t: &HirType,
                aliases: &BTreeMap<String, Vec<String>>,
            ) -> ir::Ty {
                let (base, last) = match t {
                    HirType::Name { path } | HirType::Generic { path, .. } => (
                        path.last()
                            .map(|s| s.to_ascii_lowercase())
                            .unwrap_or_default(),
                        path.last().cloned(),
                    ),
                    HirType::TypeParam { name } => (name.to_ascii_lowercase(), Some(name.clone())),
                };
                let mut b = base;
                if let Some(name) = last
                    && let Some(target) = aliases.get(&name)
                    && let Some(tl) = target.last()
                {
                    b = tl.to_ascii_lowercase();
                }
                match b.as_str() {
                    "float" | "f64" | "double" => ir::Ty::F64,
                    "bool" => ir::Ty::I1,
                    _ => ir::Ty::I64,
                }
            }

            // Reserve parameter value IDs and materialize them into local slots
            let mut param_tys: Vec<ir::Ty> = Vec::new();

            // First, add captured variables as parameters
            for (_cap_name, cap_ty) in captures {
                param_tys.push(map_hir_ty_to_ir_local(cap_ty, &lambda_st.type_aliases));
            }

            // Then add lambda's own parameters
            for p in params {
                param_tys.push(map_hir_ty_to_ir_local(&p.ty, &lambda_st.type_aliases));
            }

            let argc = param_tys.len() as u32;
            lambda_st.next_val = argc; // reserve Value(0..argc-1) as incoming params

            // Create local slots for captures
            for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                let slot = lambda_st.emit(ir::InstKind::Alloca, None);
                lambda_st.locals.insert(cap_name.clone(), slot);
                // Track captured variable type for string concat detection
                lambda_st
                    .local_types
                    .insert(cap_name.clone(), cap_ty.clone());
                let _ = lambda_st.emit(ir::InstKind::Store(slot, ir::Value(i as u32)), None);
            }

            // Create local slots for lambda parameters
            let capture_count = captures.len();
            for (i, p) in params.iter().enumerate() {
                let slot = lambda_st.emit(ir::InstKind::Alloca, None);
                lambda_st.locals.insert(p.name.clone(), slot);
                // Track parameter type for string concat detection
                lambda_st.local_types.insert(p.name.clone(), p.ty.clone());
                let param_idx = (capture_count + i) as u32;
                let _ = lambda_st.emit(ir::InstKind::Store(slot, ir::Value(param_idx)), None);
            }

            // Lower the lambda body
            lower_block(&mut lambda_st, body);

            // Determine return type - use explicit type if provided, otherwise infer from body
            let ret_ty = if let Some(t) = ret.as_ref() {
                map_hir_ty_to_ir_local(t, &lambda_st.type_aliases)
            } else {
                // Infer return type from the lowered lambda blocks
                // Look for Ret terminators and infer type from the returned value
                infer_return_type_from_blocks(&lambda_st.blocks)
            };

            // Merge lambda's string pool into parent's string pool
            // Adjust string constant indices in lambda blocks
            let string_base = st.strings.len() as u32;
            let mut lambda_blocks = lambda_st.blocks;
            if string_base > 0 {
                for b in &mut lambda_blocks {
                    for inst in &mut b.insts {
                        if let ir::InstKind::ConstStr(ref mut ix) = inst.kind {
                            *ix += string_base;
                        }
                    }
                }
            }
            st.strings.extend(lambda_st.strings);

            // Create the lambda function
            let lambda_func = ir::Func {
                name: func_name.clone(),
                params: param_tys,
                ret: ret_ty,
                blocks: lambda_blocks,
                linkage: ir::Linkage::Private, // Lambdas are private
                span: None,                    // Lambda expressions don't have function-level spans
            };

            // Add the lambda function to the accumulated lambda functions
            st.lambda_funcs.push(lambda_func);

            // Collect captured variable values to pass to MakeClosure
            let mut capture_vals = Vec::new();
            for (cap_name, _cap_ty) in captures {
                // Look up the captured variable in locals
                if let Some(&local_ptr) = st.locals.get(cap_name) {
                    // Load the captured variable value
                    let val = st.emit(ir::InstKind::Load(local_ptr), Some(span.clone()));
                    capture_vals.push(val);
                } else {
                    // Variable not found - emit placeholder 0
                    // This can happen if the variable hasn't been declared yet in IR
                    let val = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
                    capture_vals.push(val);
                }
            }

            // Emit MakeClosure instruction
            st.emit(
                ir::InstKind::MakeClosure {
                    func: func_name,
                    captures: capture_vals,
                },
                Some(span.clone()),
            )
        }
        HirExpr::StructLit {
            type_name,
            fields,
            spread,
            span,
            ..
        } => {
            // Extract struct type name for registration
            let struct_name = match type_name {
                HirType::Name { path } => path.last().cloned().unwrap_or_default(),
                HirType::Generic { path, .. } => path.last().cloned().unwrap_or_default(),
                _ => String::new(),
            };

            if st.provider_names.contains(&struct_name) {
                // Provider instantiation: (provider_name, field_values) -> ProviderNew
                let mut lower_fields = Vec::new();
                for (field_name, value) in fields {
                    let field_val = lower_expr(st, value);
                    lower_fields.push((field_name.clone(), field_val));
                }
                return st.emit(
                    ir::InstKind::ProviderNew {
                        name: struct_name,
                        values: lower_fields,
                    },
                    Some(span.clone()),
                );
            }

            // Use native struct instructions when available and no spread operator
            // (spread requires runtime support for field enumeration)
            if st.use_native_structs
                && spread.is_none()
                && st.struct_defs.contains_key(&struct_name)
            {
                // Allocate struct on stack using native instruction
                let struct_ptr = st.emit(
                    ir::InstKind::StructAlloc {
                        type_name: struct_name.clone(),
                    },
                    Some(span.clone()),
                );

                // Set each field using GEP-based native instruction
                for (idx, (field_name, value)) in fields.iter().enumerate() {
                    let field_val = lower_expr(st, value);
                    let _ = st.emit(
                        ir::InstKind::StructFieldSet {
                            ptr: struct_ptr,
                            type_name: struct_name.clone(),
                            field_name: field_name.clone(),
                            field_index: idx as u32,
                            value: field_val,
                        },
                        None,
                    );
                }

                struct_ptr
            } else {
                // Fall back to runtime calls (VM backend or spread operator)
                let field_count = fields.len() as i64;
                let name_ix = intern_str(st, &struct_name);
                let name_val = st.emit(ir::InstKind::ConstStr(name_ix), Some(span.clone()));
                let count_val = st.emit(ir::InstKind::ConstI64(field_count), Some(span.clone()));

                let struct_handle = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_struct_new".to_string(),
                        args: vec![name_val, count_val],
                        ret: ir::Ty::I64,
                    },
                    Some(span.clone()),
                );

                // If spread is present, copy fields from spread source
                if let Some(spread_expr) = spread {
                    let spread_handle = lower_expr(st, spread_expr);
                    let _ = st.emit(
                        ir::InstKind::Call {
                            name: "__arth_struct_copy".to_string(),
                            args: vec![struct_handle, spread_handle],
                            ret: ir::Ty::I64,
                        },
                        None,
                    );
                }

                // Set each field value by index
                for (idx, (field_name, value)) in fields.iter().enumerate() {
                    let field_val = lower_expr(st, value);
                    let field_name_ix = intern_str(st, field_name);
                    let field_name_val = st.emit(ir::InstKind::ConstStr(field_name_ix), None);
                    let idx_val = st.emit(ir::InstKind::ConstI64(idx as i64), None);
                    let _ = st.emit(
                        ir::InstKind::Call {
                            name: "__arth_struct_set".to_string(),
                            args: vec![struct_handle, idx_val, field_val, field_name_val],
                            ret: ir::Ty::I64,
                        },
                        None,
                    );
                }

                struct_handle
            }
        }
        HirExpr::EnumVariant {
            enum_name,
            variant_name,
            args,
            span,
            ..
        } => {
            // Get the tag for this variant
            let tag: i64 = st
                .enum_tags
                .as_ref()
                .and_then(|tags| tags.get(enum_name))
                .and_then(|vmap| vmap.get(variant_name).copied())
                .unwrap_or(0);

            // Create enum instance with tag and payload
            let enum_name_ix = intern_str(st, enum_name);
            let variant_name_ix = intern_str(st, variant_name);
            let enum_name_val = st.emit(ir::InstKind::ConstStr(enum_name_ix), Some(span.clone()));
            let variant_name_val =
                st.emit(ir::InstKind::ConstStr(variant_name_ix), Some(span.clone()));
            let tag_val = st.emit(ir::InstKind::ConstI64(tag), Some(span.clone()));
            let payload_count = st.emit(
                ir::InstKind::ConstI64(args.len() as i64),
                Some(span.clone()),
            );

            let enum_handle = st.emit(
                ir::InstKind::Call {
                    name: "__arth_enum_new".to_string(),
                    args: vec![enum_name_val, variant_name_val, tag_val, payload_count],
                    ret: ir::Ty::I64,
                },
                Some(span.clone()),
            );

            // Set payload values
            for (idx, arg) in args.iter().enumerate() {
                let arg_val = lower_expr(st, arg);
                let idx_val = st.emit(ir::InstKind::ConstI64(idx as i64), None);
                let _ = st.emit(
                    ir::InstKind::Call {
                        name: "__arth_enum_set_payload".to_string(),
                        args: vec![enum_handle, idx_val, arg_val],
                        ret: ir::Ty::I64,
                    },
                    None,
                );
            }

            enum_handle
        }
    }
}

fn intern_str(st: &mut LowerState, s: &str) -> u32 {
    if let Some((i, _)) = st.strings.iter().enumerate().find(|(_, x)| x == &s) {
        i as u32
    } else {
        st.strings.push(s.to_string());
        (st.strings.len() - 1) as u32
    }
}

/// Compute a simple hash of a string for function identification.
/// Uses FNV-1a hash algorithm for simplicity and speed.
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

fn lower_fields_buffer(st: &mut LowerState, e: &HirExpr) -> ir::Value {
    // Recognize Fields.of(...) or log.Fields.of(...). Build "k=v" pairs separated by space.
    let mut out = String::new();
    let mut appended = false;
    if let HirExpr::Call { callee, args, .. } = e {
        if let HirExpr::Member { object, member, .. } = &**callee {
            if member == "of" {
                let is_fields = match &**object {
                    HirExpr::Ident { name, .. } if name == "Fields" => true,
                    HirExpr::Member {
                        object: pkg,
                        member: m,
                        ..
                    } => {
                        matches!(**pkg, HirExpr::Ident { ref name, .. } if name=="log")
                            && m == "Fields"
                    }
                    _ => false,
                };
                if is_fields {
                    fn const_to_string(e: &HirExpr) -> Option<String> {
                        match e {
                            HirExpr::Str { value, .. } => Some(value.clone()),
                            HirExpr::Int { value, .. } => Some(value.to_string()),
                            HirExpr::Float { value, .. } => Some(format!("{}", value)),
                            HirExpr::Char { value, .. } => Some(value.to_string()),
                            HirExpr::Bool { value, .. } => Some(if *value {
                                "true".into()
                            } else {
                                "false".into()
                            }),
                            HirExpr::Unary { op, expr, .. } => {
                                if let Some(s) = const_to_string(expr) {
                                    match op {
                                        HirUnOp::Neg => {
                                            s.parse::<i64>().ok().map(|n| (-n).to_string()).or_else(
                                                || s.parse::<f64>().ok().map(|x| (-x).to_string()),
                                            )
                                        }
                                        HirUnOp::Not => s
                                            .parse::<i64>()
                                            .ok()
                                            .map(|n| (n == 0).to_string())
                                            .or_else(|| {
                                                s.parse::<bool>().ok().map(|b| (!b).to_string())
                                            }),
                                    }
                                } else {
                                    None
                                }
                            }
                            HirExpr::Binary {
                                left, op, right, ..
                            } => {
                                let ls = const_to_string(left)?;
                                let rs = const_to_string(right)?;
                                // Try integer then float ops
                                let (li, ri) = (ls.parse::<i64>().ok(), rs.parse::<i64>().ok());
                                let (lf, rf) = (ls.parse::<f64>().ok(), rs.parse::<f64>().ok());
                                use HirBinOp as HB;
                                if let (Some(a), Some(b)) = (li, ri) {
                                    match op {
                                        HB::Add => Some((a + b).to_string()),
                                        HB::Sub => Some((a - b).to_string()),
                                        HB::Mul => Some((a * b).to_string()),
                                        HB::Div => {
                                            Some((if b == 0 { a } else { a / b }).to_string())
                                        }
                                        HB::Mod => {
                                            Some((if b == 0 { 0 } else { a % b }).to_string())
                                        }
                                        HB::Shl => Some((a << b).to_string()),
                                        HB::Shr => Some((a >> b).to_string()),
                                        HB::Lt => Some((a < b).to_string()),
                                        HB::Le => Some((a <= b).to_string()),
                                        HB::Gt => Some((a > b).to_string()),
                                        HB::Ge => Some((a >= b).to_string()),
                                        HB::Eq => Some((a == b).to_string()),
                                        HB::Ne => Some((a != b).to_string()),
                                        HB::And => Some(((a != 0) & (b != 0)).to_string()),
                                        HB::Or => Some(((a != 0) | (b != 0)).to_string()),
                                        HB::BitAnd => Some((a & b).to_string()),
                                        HB::BitOr => Some((a | b).to_string()),
                                        HB::Xor => Some((a ^ b).to_string()),
                                    }
                                } else if let (Some(a), Some(b)) = (lf, rf) {
                                    match op {
                                        HB::Add => Some((a + b).to_string()),
                                        HB::Sub => Some((a - b).to_string()),
                                        HB::Mul => Some((a * b).to_string()),
                                        HB::Div => {
                                            Some((if b == 0.0 { a } else { a / b }).to_string())
                                        }
                                        HB::Mod => None,
                                        HB::Shl | HB::Shr => None,
                                        HB::Lt => Some((a < b).to_string()),
                                        HB::Le => Some((a <= b).to_string()),
                                        HB::Gt => Some((a > b).to_string()),
                                        HB::Ge => Some((a >= b).to_string()),
                                        HB::Eq => Some((a == b).to_string()),
                                        HB::Ne => Some((a != b).to_string()),
                                        HB::And | HB::Or | HB::BitAnd | HB::BitOr | HB::Xor => None,
                                    }
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    let mut it = args.iter();
                    while let Some(k) = it.next() {
                        let v = it.next();
                        let key = const_to_string(k).unwrap_or_else(|| {
                            let _ = lower_expr(st, k);
                            String::from("?")
                        });
                        let val = match v {
                            Some(expr) => const_to_string(expr).unwrap_or_else(|| {
                                let _ = lower_expr(st, expr);
                                String::from("?")
                            }),
                            None => String::new(),
                        };
                        if !key.is_empty() {
                            if appended {
                                out.push(' ');
                            }
                            appended = true;
                            out.push_str(&key);
                            if !val.is_empty() {
                                out.push('=');
                                out.push_str(&val);
                            }
                        }
                    }
                }
            }
        }
    }
    let ix = intern_str(st, &out);
    st.emit(ir::InstKind::ConstStr(ix), None)
}

fn lower_cond(st: &mut LowerState, e: &HirExpr) -> ir::Value {
    // Ensure a boolean-ish value for branch conditions, with short-circuit semantics
    match e {
        // Short-circuit AND: if lhs is false, result is false; otherwise evaluate rhs
        HirExpr::Binary {
            left,
            op: HirBinOp::And,
            right,
            span,
            ..
        } => {
            // Allocate result slot for the boolean value (0 or 1)
            let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));
            // Evaluate left side condition
            let lval = lower_cond(st, left);

            // Create blocks for RHS path, false path, and join
            let rhs_b = st.new_block("and_rhs", Some(span.clone()));
            let false_b = st.new_block("and_false", Some(span.clone()));
            let join_b = st.new_block("and_join", Some(span.clone()));

            // Branch on left value
            let cur = st.cur;
            st.blocks[cur].term = ir::Terminator::CondBr {
                cond: lval,
                then_bb: ir::Block(rhs_b as u32),
                else_bb: ir::Block(false_b as u32),
            };

            // RHS path: evaluate right condition, store it, then jump to join
            st.set_cur(rhs_b);
            let rval = lower_cond(st, right);
            let _ = st.emit(ir::InstKind::Store(slot, rval), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // False path: store boolean false, then jump to join
            st.set_cur(false_b);
            let z0 = st.emit(ir::InstKind::ConstI64(0), Some(span.clone()));
            let o1 = st.emit(ir::InstKind::ConstI64(1), Some(span.clone()));
            let fval = st.emit(
                ir::InstKind::Cmp(ir::CmpPred::Eq, z0, o1),
                Some(span.clone()),
            );
            let _ = st.emit(ir::InstKind::Store(slot, fval), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // Join: load result
            st.set_cur(join_b);
            st.emit(ir::InstKind::Load(slot), Some(span.clone()))
        }
        // Short-circuit OR: if lhs is true, result is true; otherwise evaluate rhs
        HirExpr::Binary {
            left,
            op: HirBinOp::Or,
            right,
            span,
            ..
        } => {
            // Allocate result slot for the boolean value (0 or 1)
            let slot = st.emit(ir::InstKind::Alloca, Some(span.clone()));
            // Evaluate left side condition
            let lval = lower_cond(st, left);

            // Create blocks for true path, RHS path, and join
            let true_b = st.new_block("or_true", Some(span.clone()));
            let rhs_b = st.new_block("or_rhs", Some(span.clone()));
            let join_b = st.new_block("or_join", Some(span.clone()));

            // Branch on left value
            let cur = st.cur;
            st.blocks[cur].term = ir::Terminator::CondBr {
                cond: lval,
                then_bb: ir::Block(true_b as u32),
                else_bb: ir::Block(rhs_b as u32),
            };

            // True path: store boolean true, then jump to join
            st.set_cur(true_b);
            let o1a = st.emit(ir::InstKind::ConstI64(1), Some(span.clone()));
            let o1b = st.emit(ir::InstKind::ConstI64(1), Some(span.clone()));
            let tval = st.emit(
                ir::InstKind::Cmp(ir::CmpPred::Eq, o1a, o1b),
                Some(span.clone()),
            );
            let _ = st.emit(ir::InstKind::Store(slot, tval), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // RHS path: evaluate right condition, store it, then jump to join
            st.set_cur(rhs_b);
            let rval = lower_cond(st, right);
            let _ = st.emit(ir::InstKind::Store(slot, rval), Some(span.clone()));
            st.set_term(ir::Terminator::Br(ir::Block(join_b as u32)));

            // Join: load result
            st.set_cur(join_b);
            st.emit(ir::InstKind::Load(slot), Some(span.clone()))
        }
        // Simple cases: already boolean-like expressions
        HirExpr::Bool { .. } | HirExpr::Binary { .. } | HirExpr::Unary { .. } => lower_expr(st, e),
        HirExpr::Await { .. } => lower_expr(st, e),
        // Fallback: compare against zero to form a condition
        _ => {
            let v = lower_expr(st, e);
            let zero = st.emit(ir::InstKind::ConstI64(0), None);
            st.emit(ir::InstKind::Cmp(ir::CmpPred::Ne, v, zero), None)
        }
    }
}

fn eval_int_const(e: &HirExpr) -> Option<i64> {
    match e {
        HirExpr::Int { value, .. } => Some(*value),
        HirExpr::Bool { value, .. } => Some(if *value { 1 } else { 0 }),
        HirExpr::Await { .. } => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::compiler::hir::{HirDecl, make_hir_file};
    use crate::compiler::parser::parse_file;
    use crate::compiler::source::SourceFile;

    use super::{
        ExternSig, LoweringOptions, lower_hir_extern_func_to_ir, lower_hir_func_to_ir_demo,
        lower_hir_func_to_ir_native, make_extern_sig,
    };

    #[test]
    fn lambda_value_call_lowers_to_closurecall() {
        let code = r#"
package ft;
module M {
  public void main() {
    Fn<Int>(Int, Int) add = fn (Int a, Int b) { return a + b; };
    Int z = add(2, 3);
    println(z);
  }
}
"#;
        let sf = SourceFile {
            path: PathBuf::from("/mem/ft/fn_ir.arth"),
            text: code.to_string(),
        };
        let mut rep = crate::compiler::diagnostics::Reporter::new();
        let ast = parse_file(&sf, &mut rep);
        assert!(!rep.has_errors(), "parse errors detected");
        let pkg = ast.package.as_ref().map(|p| p.to_string());
        let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

        // Find main() in module M
        let mut main_func = None;
        for d in &hir.decls {
            if let HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.sig.name == "main" {
                        main_func = Some(f.clone());
                        break;
                    }
                }
            }
        }
        let main_func = main_func.expect("main function not found");

        let (funcs, _strings) = lower_hir_func_to_ir_demo(&main_func, None);

        // Locate IR for main and check it uses MakeClosure + ClosureCall,
        // not a direct Call to 'add'.
        let mut make_closure_count = 0usize;
        let mut closure_call_count = 0usize;
        let mut direct_add_calls = 0usize;
        for f in &funcs {
            if f.name != "main" {
                continue;
            }
            for b in &f.blocks {
                for inst in &b.insts {
                    match &inst.kind {
                        crate::compiler::ir::InstKind::MakeClosure { .. } => {
                            make_closure_count += 1;
                        }
                        crate::compiler::ir::InstKind::ClosureCall { .. } => {
                            closure_call_count += 1;
                        }
                        crate::compiler::ir::InstKind::Call { name, .. } if name == "add" => {
                            direct_add_calls += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        assert!(
            make_closure_count >= 1,
            "expected at least one MakeClosure in main, found {}",
            make_closure_count
        );
        assert!(
            closure_call_count >= 1,
            "expected at least one ClosureCall in main, found {}",
            closure_call_count
        );
        assert_eq!(
            direct_add_calls, 0,
            "expected no direct Call to 'add' in main"
        );
    }

    #[test]
    fn extern_call_lowers_to_extern_call_ir() {
        use super::EnumLowerContext;
        use super::ExternSig;
        use super::lower_hir_extern_func_to_ir;
        use std::collections::{BTreeMap, HashMap, HashSet};

        let code = r#"
package demo.ffi;

extern "C" fn c_abs(int x) -> int;

module M {
  public unsafe int call_abs(int n) {
    return c_abs(n);
  }
}
"#;
        let sf = SourceFile {
            path: PathBuf::from("/mem/demo/ffi_ir.arth"),
            text: code.to_string(),
        };
        let mut rep = crate::compiler::diagnostics::Reporter::new();
        let ast = parse_file(&sf, &mut rep);
        assert!(!rep.has_errors(), "parse errors detected");
        let pkg = ast.package.as_ref().map(|p| p.to_string());
        let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

        // Collect extern function signatures and lower extern declarations
        let mut extern_funcs: HashMap<String, ExternSig> = HashMap::new();
        let mut extern_ir_decls = Vec::new();
        for d in &hir.decls {
            if let HirDecl::ExternFunc(ef) = d {
                extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
                extern_ir_decls.push(lower_hir_extern_func_to_ir(ef));
            }
        }

        // Verify extern function is lowered to IR correctly
        assert_eq!(extern_ir_decls.len(), 1, "expected one extern declaration");
        let ir_extern = &extern_ir_decls[0];
        assert_eq!(ir_extern.name, "c_abs");
        assert_eq!(ir_extern.abi, "C");
        assert_eq!(ir_extern.params.len(), 1);
        assert_eq!(ir_extern.ret, crate::compiler::ir::Ty::I64);

        // Create enum context with extern functions
        let enum_ctx = EnumLowerContext {
            tags: BTreeMap::new(),
            shared_field_names: HashSet::new(),
            type_aliases: BTreeMap::new(),
            types_needing_drop: BTreeMap::new(),
            json_codec_structs: BTreeMap::new(),
            extern_funcs,
            provider_names: HashSet::new(),
            struct_field_types: HashMap::new(),
        };

        // Find call_abs() function and lower it
        let mut call_abs_func = None;
        for d in &hir.decls {
            if let HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.sig.name == "call_abs" {
                        call_abs_func = Some(f.clone());
                        break;
                    }
                }
            }
        }
        let call_abs_func = call_abs_func.expect("call_abs function not found");

        let (funcs, _strings) = lower_hir_func_to_ir_demo(&call_abs_func, Some(&enum_ctx));

        // Check that call_abs lowered to ExternCall, not regular Call
        let mut extern_call_count = 0usize;
        let mut regular_call_to_c_abs = 0usize;
        for ir_func in &funcs {
            for block in &ir_func.blocks {
                for inst in &block.insts {
                    match &inst.kind {
                        crate::compiler::ir::InstKind::ExternCall { name, .. }
                            if name == "c_abs" =>
                        {
                            extern_call_count += 1;
                        }
                        crate::compiler::ir::InstKind::Call { name, .. } if name == "c_abs" => {
                            regular_call_to_c_abs += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        assert!(
            extern_call_count >= 1,
            "expected at least one ExternCall to c_abs, found {}",
            extern_call_count
        );
        assert_eq!(
            regular_call_to_c_abs, 0,
            "expected no regular Call to c_abs (should be ExternCall)"
        );
    }

    #[test]
    fn test_ffi_ownership_extraction() {
        use super::{FfiOwnership, extract_ffi_ownership};
        use crate::compiler::hir::HirAttr;

        // Test @ffi_owned extraction
        let attrs = vec![HirAttr {
            name: "ffi_owned".to_string(),
            args: None,
        }];
        assert_eq!(extract_ffi_ownership(&attrs), FfiOwnership::Owned);

        // Test @ffi_borrowed extraction
        let attrs = vec![HirAttr {
            name: "ffi_borrowed".to_string(),
            args: None,
        }];
        assert_eq!(extract_ffi_ownership(&attrs), FfiOwnership::Borrowed);

        // Test @ffi_transfers extraction
        let attrs = vec![HirAttr {
            name: "ffi_transfers".to_string(),
            args: None,
        }];
        assert_eq!(extract_ffi_ownership(&attrs), FfiOwnership::Transfers);

        // Test no FFI attribute
        let attrs = vec![HirAttr {
            name: "some_other_attr".to_string(),
            args: None,
        }];
        assert_eq!(extract_ffi_ownership(&attrs), FfiOwnership::None);

        // Test empty attrs
        let attrs: Vec<HirAttr> = vec![];
        assert_eq!(extract_ffi_ownership(&attrs), FfiOwnership::None);
    }

    #[test]
    fn test_ffi_owned_generates_cleanup() {
        use crate::compiler::diagnostics::Reporter;
        use std::collections::{BTreeMap, HashMap, HashSet};

        use super::EnumLowerContext;

        // Test that @ffi_owned extern function calls generate cleanup instructions
        let code = r#"
package ffi_test;

@ffi_owned
extern "C" fn allocate_buffer() -> ptr;

module Main {
    public void main() {
        unsafe {
            ptr buf = allocate_buffer();
            // buf should be registered for cleanup at scope exit
        }
    }
}
"#;
        let sf = SourceFile {
            path: PathBuf::from("test.arth"),
            text: code.to_string(),
        };
        let mut rep = Reporter::new();
        let ast = parse_file(&sf, &mut rep);
        assert!(!rep.has_errors(), "parse errors detected");
        let pkg = ast.package.as_ref().map(|p| p.to_string());
        let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

        // Collect extern function signatures
        let mut extern_funcs: HashMap<String, ExternSig> = HashMap::new();
        for d in &hir.decls {
            if let HirDecl::ExternFunc(ef) = d {
                let sig = make_extern_sig(ef);
                // Verify @ffi_owned is captured
                assert_eq!(
                    sig.return_ownership,
                    super::FfiOwnership::Owned,
                    "Expected @ffi_owned to be captured"
                );
                extern_funcs.insert(ef.name.clone(), sig);
            }
        }

        // Verify we found the allocate_buffer extern
        assert!(
            extern_funcs.contains_key("allocate_buffer"),
            "Expected allocate_buffer extern function"
        );

        // Create enum context with extern functions
        let enum_ctx = EnumLowerContext {
            tags: BTreeMap::new(),
            shared_field_names: HashSet::new(),
            type_aliases: BTreeMap::new(),
            types_needing_drop: BTreeMap::new(),
            json_codec_structs: BTreeMap::new(),
            extern_funcs,
            provider_names: HashSet::new(),
            struct_field_types: HashMap::new(),
        };

        // Find main() function and lower it
        let mut main_func = None;
        for d in &hir.decls {
            if let HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.sig.name == "main" {
                        main_func = Some(f.clone());
                        break;
                    }
                }
            }
        }
        let main_func = main_func.expect("main function not found");

        let (funcs, _strings) = lower_hir_func_to_ir_demo(&main_func, Some(&enum_ctx));

        // Check that main() has an ExternCall to allocate_buffer
        let mut has_extern_call = false;
        let mut has_drop = false;
        for ir_func in &funcs {
            for block in &ir_func.blocks {
                for inst in &block.insts {
                    match &inst.kind {
                        crate::compiler::ir::InstKind::ExternCall { name, .. }
                            if name == "allocate_buffer" =>
                        {
                            has_extern_call = true;
                        }
                        crate::compiler::ir::InstKind::Drop { .. } => {
                            has_drop = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        assert!(
            has_extern_call,
            "Expected ExternCall to allocate_buffer in lowered IR"
        );
        assert!(
            has_drop,
            "Expected Drop instruction for @ffi_owned return value"
        );
    }

    #[test]
    fn native_lowering_maps_executor_cancel_to_runtime_intrinsic() {
        let code = r#"
package test.native;

module Main {
    public void main() {
        Int task = Executor.spawn(1);
        Int status = Executor.cancel(task);
        println(status);
    }
}
"#;
        let sf = SourceFile {
            path: PathBuf::from("/mem/native/executor_cancel.arth"),
            text: code.to_string(),
        };
        let mut rep = crate::compiler::diagnostics::Reporter::new();
        let ast = parse_file(&sf, &mut rep);
        assert!(!rep.has_errors(), "parse errors detected");
        let pkg = ast.package.as_ref().map(|p| p.to_string());
        let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

        let mut main_func = None;
        for d in &hir.decls {
            if let HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.sig.name == "main" {
                        main_func = Some(f.clone());
                        break;
                    }
                }
            }
        }
        let main_func = main_func.expect("main function not found");

        let opts = LoweringOptions {
            use_native_structs: true,
            ..Default::default()
        };
        let (funcs, _strings) = lower_hir_func_to_ir_native(&main_func, None, None, &opts);

        let mut saw_cancel_call = false;
        for f in &funcs {
            if f.name != "main" {
                continue;
            }
            for b in &f.blocks {
                for inst in &b.insts {
                    if let crate::compiler::ir::InstKind::Call { name, .. } = &inst.kind
                        && name == "__arth_executor_cancel"
                    {
                        saw_cancel_call = true;
                    }
                }
            }
        }
        assert!(
            saw_cancel_call,
            "expected native lowering to emit __arth_executor_cancel call"
        );
    }

    #[test]
    fn native_region_alloc_strategy_emits_region_alloc_call() {
        use crate::compiler::typeck::escape_results::{
            AllocStrategy as EscapeAllocStrategy, FunctionEscapeInfo, LocalEscapeInfo,
        };

        let code = r#"
package test.native;

module Main {
    public void main() {
        Int total = 0;
        for (Int i = 0; i < 2; i = i + 1) {
            Int loopLocal = i;
            total = total + loopLocal;
        }
        println(total);
    }
}
"#;
        let sf = SourceFile {
            path: PathBuf::from("/mem/native/region_alloc.arth"),
            text: code.to_string(),
        };
        let mut rep = crate::compiler::diagnostics::Reporter::new();
        let ast = parse_file(&sf, &mut rep);
        assert!(!rep.has_errors(), "parse errors detected");
        let pkg = ast.package.as_ref().map(|p| p.to_string());
        let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

        let mut main_func = None;
        for d in &hir.decls {
            if let HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.sig.name == "main" {
                        main_func = Some(f.clone());
                        break;
                    }
                }
            }
        }
        let main_func = main_func.expect("main function not found");

        let mut escape_info = FunctionEscapeInfo::new();
        escape_info.add_local(
            "loopLocal",
            LocalEscapeInfo {
                alloc_strategy: EscapeAllocStrategy::Region(0),
                ..Default::default()
            },
        );

        let opts = LoweringOptions {
            use_native_structs: true,
            ..Default::default()
        };
        let (funcs, _strings) =
            lower_hir_func_to_ir_native(&main_func, None, Some(&escape_info), &opts);

        let mut saw_region_alloc = false;
        let mut saw_region_enter = false;
        let mut saw_region_exit = false;
        for f in &funcs {
            if f.name != "main" {
                continue;
            }
            for b in &f.blocks {
                for inst in &b.insts {
                    match &inst.kind {
                        crate::compiler::ir::InstKind::Call { name, .. }
                            if name == "__arth_region_alloc" =>
                        {
                            saw_region_alloc = true;
                        }
                        crate::compiler::ir::InstKind::RegionEnter { .. } => {
                            saw_region_enter = true;
                        }
                        crate::compiler::ir::InstKind::RegionExit { .. } => {
                            saw_region_exit = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        assert!(saw_region_enter, "expected region enter for loop body");
        assert!(
            saw_region_alloc,
            "expected region-local slot allocation call"
        );
        assert!(saw_region_exit, "expected region exit for loop body");
    }
}
