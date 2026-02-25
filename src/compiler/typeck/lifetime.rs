// Lifetime Inference for Arth
//
// This module implements automatic, function-scoped lifetime inference for borrows.
// Arth uses automatic borrowing - no explicit `&` syntax in source code. The compiler
// infers when values are borrowed vs moved, and tracks lifetimes to ensure borrows
// don't outlive their source values.
//
// Key design principles (from docs/scope.md):
// - No explicit lifetime annotations in source code
// - Lifetimes are inferred function-locally
// - Borrow checker validates safety without user-visible lifetime parameters
//
// Algorithm overview:
// 1. Assign unique region IDs to each borrow point (call site where borrow occurs)
// 2. Track the "origin" of each borrow (which local variable it borrows from)
// 3. Track liveness of source variables and borrowed references
// 4. At each program point, verify no borrow outlives its source
// 5. At function exit, verify no borrows escape

use std::collections::{HashMap, HashSet};

use super::nll::{NllContext, NllDiagnostic, NllError, ProgramPoint};

/// Unique identifier for a lifetime region.
/// Each borrow creates a new region that tracks when the borrow is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionId(pub u32);

impl RegionId {
    pub fn new(id: u32) -> Self {
        RegionId(id)
    }
}

/// Represents the origin of a borrow - what value it borrows from
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BorrowOrigin {
    /// Borrowed from a local variable
    Local(String),
    /// Borrowed from a field of a local: (base_local, field_path)
    Field(String, Vec<String>),
    /// Borrowed from a provider (longer-lived than function scope)
    Provider(String),
    /// Borrowed from a function parameter
    Param(String),
    /// Unknown origin (for error recovery)
    Unknown,
}

/// Kind of borrow - immutable (shared) or mutable (exclusive)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BorrowMode {
    /// Shared/immutable borrow - multiple allowed simultaneously
    Shared,
    /// Exclusive/mutable borrow - only one allowed, blocks all other access
    Exclusive,
}

/// Information about an active borrow
#[derive(Clone, Debug)]
pub struct BorrowInfo {
    /// Unique region ID for this borrow
    pub region: RegionId,
    /// What this borrows from
    pub origin: BorrowOrigin,
    /// Whether this is shared or exclusive
    pub mode: BorrowMode,
    /// Name of the local that holds the borrow (if any)
    pub holder: Option<String>,
    /// Source span where borrow was created
    pub span: Option<crate::compiler::hir::core::Span>,
    /// Depth of scope where borrow was created (for drop tracking)
    pub scope_depth: u32,
    /// HIR ID of the expression that created this borrow (for diagnostics and constraint generation)
    /// This allows us to point to the exact expression that caused the borrow
    pub creation_site: Option<crate::compiler::hir::core::HirId>,
}

/// Information about a borrow that is live at an await point.
/// Used for full borrow analysis at await boundaries.
#[derive(Clone, Debug)]
pub struct AwaitBorrowInfo {
    /// Region ID of the borrow
    pub region: RegionId,
    /// Name of the local that holds the borrow (if any)
    pub holder: Option<String>,
    /// Whether this is shared or exclusive
    pub mode: BorrowMode,
    /// What this borrows from
    pub origin: BorrowOrigin,
    /// Source span where borrow was created
    pub span: Option<crate::compiler::hir::core::Span>,
}

impl AwaitBorrowInfo {
    /// Create a description for diagnostics
    pub fn describe(&self) -> String {
        let mode_str = match self.mode {
            BorrowMode::Shared => "shared",
            BorrowMode::Exclusive => "exclusive",
        };
        let origin_str = match &self.origin {
            BorrowOrigin::Local(name) => format!("'{}'", name),
            BorrowOrigin::Param(name) => format!("parameter '{}'", name),
            BorrowOrigin::Field(obj, path) => format!("'{}.{}'", obj, path.join(".")),
            BorrowOrigin::Provider(name) => format!("provider '{}'", name),
            BorrowOrigin::Unknown => "unknown".to_string(),
        };
        let holder_str = match &self.holder {
            Some(name) => format!(" (held by '{}')", name),
            None => String::new(),
        };
        format!("{} borrow of {}{}", mode_str, origin_str, holder_str)
    }
}

/// Tracks lifetime state for a single local variable
#[derive(Clone, Debug)]
pub struct LocalLifetime {
    /// Region ID for the variable's own storage lifetime
    pub storage_region: RegionId,
    /// Active borrows of this variable (region IDs that borrow from this local)
    pub borrowed_by: HashSet<RegionId>,
    /// Whether this local holds a borrow (and from what region)
    pub holds_borrow: Option<RegionId>,
    /// Scope depth where this local was declared
    pub declared_at_depth: u32,
    /// Whether this binding is a function parameter
    pub is_param: bool,
    /// Whether this binding is still live (in-scope)
    pub is_live: bool,
    /// Escape analysis: whether this value may escape its defining scope
    pub escapes: EscapeState,
    /// Allocation strategy determined by escape analysis
    pub alloc_strategy: AllocStrategy,
    /// Loop region this local was declared in (if any) - for region-based allocation
    pub declared_in_loop_region: Option<RegionId>,
}

/// Escape analysis state for a local variable
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscapeState {
    /// Value does not escape - can be stack allocated
    NoEscape,
    /// Value is returned from function
    EscapesViaReturn,
    /// Value is stored in a field of an escaping object
    EscapesViaField,
    /// Value is captured by a closure
    EscapesViaClosure,
    /// Value is passed to a function that may store it
    EscapesViaCall,
    /// Value is stored in a provider (long-lived)
    EscapesViaProvider,
    /// Conservative: assume escapes (for complex cases)
    MayEscape,
}

impl Default for EscapeState {
    fn default() -> Self {
        EscapeState::NoEscape
    }
}

/// Memory allocation strategy determined by escape analysis
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AllocStrategy {
    /// Stack allocation - value doesn't escape, deterministic lifetime
    Stack,
    /// Region allocation - value lives for a known region (e.g., loop body)
    Region(RegionId),
    /// Reference counted - value escapes, needs RC for cleanup
    RefCounted,
    /// Unique ownership - single owner, move semantics, deterministic drop
    UniqueOwned,
}

/// Kind of region for allocation purposes
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// Function-scoped region (default for function body)
    Function,
    /// Loop iteration region - allocations are bulk-deallocated on each iteration exit
    Loop(u32),
}

/// Information about an active region (for loop-based allocation)
#[derive(Clone, Debug)]
pub struct RegionInfo {
    /// Unique region ID
    pub id: RegionId,
    /// Kind of region (function, loop)
    pub kind: RegionKind,
    /// Scope depth where this region was created
    pub depth: u32,
}

impl Default for AllocStrategy {
    fn default() -> Self {
        AllocStrategy::UniqueOwned
    }
}

/// Summary of allocation strategies for a function's locals
#[derive(Clone, Debug, Default)]
pub struct AllocationSummary {
    /// Number of locals using stack allocation
    pub stack_allocated: usize,
    /// Number of locals using reference counting
    pub ref_counted: usize,
    /// Number of locals with unique ownership
    pub unique_owned: usize,
    /// Number of locals using region-based allocation
    pub region_allocated: usize,
}

/// Counter for generating unique region IDs
#[derive(Clone, Debug)]
pub struct RegionGenerator {
    next_id: u32,
}

impl RegionGenerator {
    pub fn new() -> Self {
        RegionGenerator { next_id: 0 }
    }

    pub fn fresh(&mut self) -> RegionId {
        let id = RegionId(self.next_id);
        self.next_id += 1;
        id
    }
}

impl Default for RegionGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Lifetime environment - tracks all active borrows and their lifetimes
#[derive(Clone, Debug)]
pub struct LifetimeEnv {
    /// Region ID generator
    region_gen: RegionGenerator,
    /// All active borrows: region_id -> borrow info
    pub(crate) active_borrows: HashMap<RegionId, BorrowInfo>,
    /// Per-local lifetime tracking: local_name -> lifetime info
    pub(crate) local_lifetimes: HashMap<String, LocalLifetime>,
    /// Current scope depth (incremented on push, decremented on pop)
    scope_depth: u32,
    /// Stack of scope boundaries (regions created at each scope level)
    scope_regions: Vec<HashSet<RegionId>>,
    /// Stack of active loop regions (for region-based allocation)
    loop_regions: Vec<RegionInfo>,
    /// Counter for generating unique loop IDs
    next_loop_id: u32,
    /// NLL context for non-lexical lifetime analysis (Phase 3)
    /// When enabled, tracks program points and computes precise live ranges
    pub(crate) nll: Option<NllContext>,
    /// Providers that have been mutated - borrows from these are invalidated
    pub(crate) invalidated_providers: HashSet<String>,
}

impl LifetimeEnv {
    pub fn new() -> Self {
        LifetimeEnv {
            region_gen: RegionGenerator::new(),
            active_borrows: HashMap::new(),
            local_lifetimes: HashMap::new(),
            scope_depth: 0,
            scope_regions: vec![HashSet::new()],
            loop_regions: Vec::new(),
            next_loop_id: 0,
            nll: None,
            invalidated_providers: HashSet::new(),
        }
    }

    /// Create a LifetimeEnv with NLL analysis enabled.
    /// This enables precise, non-lexical lifetime tracking.
    pub fn with_nll() -> Self {
        LifetimeEnv {
            region_gen: RegionGenerator::new(),
            active_borrows: HashMap::new(),
            local_lifetimes: HashMap::new(),
            scope_depth: 0,
            scope_regions: vec![HashSet::new()],
            loop_regions: Vec::new(),
            next_loop_id: 0,
            nll: Some(NllContext::new()),
            invalidated_providers: HashSet::new(),
        }
    }

    /// Enable NLL analysis on an existing LifetimeEnv.
    pub fn enable_nll(&mut self) {
        if self.nll.is_none() {
            self.nll = Some(NllContext::new());
        }
    }

    /// Check if NLL analysis is enabled.
    pub fn is_nll_enabled(&self) -> bool {
        self.nll.is_some()
    }

    // --- NLL Integration Methods ---

    /// Advance to the next program point (call before each statement).
    /// Only does anything if NLL is enabled.
    pub fn nll_advance(&mut self) -> Option<ProgramPoint> {
        self.nll.as_mut().map(|ctx| ctx.advance())
    }

    /// Get the current program point (for diagnostics).
    pub fn nll_current_point(&self) -> Option<ProgramPoint> {
        self.nll.as_ref().map(|ctx| ctx.current_point)
    }

    /// Record a use of a local variable at the current program point.
    /// This updates liveness information for NLL analysis.
    pub fn nll_record_local_use(&mut self, name: &str) {
        if let Some(nll) = &mut self.nll {
            // Get the region for this local if it exists
            if let Some(region) = nll.local_regions.get(name).copied() {
                nll.regions
                    .entry(region)
                    .or_default()
                    .add_point(nll.current_point);
            }
        }
    }

    /// Record a use of a borrow at the current program point.
    /// This extends the borrow's live range.
    pub fn nll_record_borrow_use(&mut self, region: RegionId) {
        if let Some(nll) = &mut self.nll {
            nll.record_borrow_use(region);
        }
    }

    /// Enter a new control flow block (if/else branch, loop body).
    /// Returns the new block ID for later joining.
    pub fn nll_enter_block(&mut self) -> Option<u32> {
        self.nll.as_mut().map(|ctx| ctx.enter_block())
    }

    /// Create a join point from multiple control flow paths.
    pub fn nll_create_join(&mut self, predecessors: &[u32]) -> Option<u32> {
        self.nll
            .as_mut()
            .map(|ctx| ctx.create_join_block(predecessors))
    }

    /// Mark the current block as a function exit (return/throw).
    pub fn nll_mark_exit(&mut self) {
        if let Some(nll) = &mut self.nll {
            nll.mark_exit();
        }
    }

    /// Add an outlives constraint: source must outlive borrow.
    pub fn nll_add_constraint(&mut self, source: RegionId, borrow: RegionId, reason: &str) {
        if let Some(nll) = &mut self.nll {
            nll.add_constraint(source, borrow, reason);
        }
    }

    /// Solve NLL constraints and return any errors.
    pub fn nll_solve(&self) -> Vec<NllError> {
        self.nll
            .as_ref()
            .map(|ctx| ctx.solve_constraints())
            .unwrap_or_default()
    }

    /// Check if a borrow is still live at the current point (NLL-aware).
    /// Falls back to lexical checking if NLL is disabled.
    pub fn nll_is_borrow_live(&self, region: RegionId) -> bool {
        if let Some(nll) = &self.nll {
            nll.is_borrow_live(region)
        } else {
            // Fallback: check if borrow is in active_borrows
            self.active_borrows.contains_key(&region)
        }
    }

    /// Enter a new scope (block, if-branch, loop body, etc.)
    pub fn push_scope(&mut self) {
        self.scope_depth += 1;
        self.scope_regions.push(HashSet::new());
    }

    /// Exit a scope - invalidates borrows created in this scope
    pub fn pop_scope(&mut self) -> Vec<LifetimeError> {
        let mut errors = Vec::new();

        if let Some(scope_regions) = self.scope_regions.pop() {
            // End all borrows created in this scope
            for region in scope_regions {
                if let Some(borrow) = self.active_borrows.remove(&region) {
                    // Remove from the source's borrowed_by set
                    if let BorrowOrigin::Local(ref name) | BorrowOrigin::Param(ref name) =
                        borrow.origin
                    {
                        if let Some(local) = self.local_lifetimes.get_mut(name) {
                            local.borrowed_by.remove(&region);
                        }
                    }
                }
            }

            // Mark locals declared at this scope depth as no longer live, but keep them
            // around so escape analysis can still export allocation strategies.
            let to_end: Vec<String> = self
                .local_lifetimes
                .iter()
                .filter(|(_, lt)| lt.is_live && lt.declared_at_depth == self.scope_depth)
                .map(|(name, _)| name.clone())
                .collect();

            for name in to_end {
                if let Some(local) = self.local_lifetimes.get_mut(&name) {
                    // Check if any borrows of this local are still active
                    for region in &local.borrowed_by {
                        if let Some(borrow) = self.active_borrows.get(region) {
                            errors.push(LifetimeError::BorrowOutlivesSource {
                                borrow_region: *region,
                                source_name: name.clone(),
                                borrow_span: borrow.span.clone(),
                            });
                        }
                    }
                    local.is_live = false;
                }
            }
        }

        self.scope_depth = self.scope_depth.saturating_sub(1);
        errors
    }

    /// Declare a new local variable
    pub fn declare_local(&mut self, name: &str) {
        let region = self.region_gen.fresh();
        // Track which loop region this local was declared in (if any)
        let loop_region = self.current_loop_region_id();
        self.local_lifetimes.insert(
            name.to_string(),
            LocalLifetime {
                storage_region: region,
                borrowed_by: HashSet::new(),
                holds_borrow: None,
                declared_at_depth: self.scope_depth,
                is_param: false,
                is_live: true,
                escapes: EscapeState::NoEscape,
                alloc_strategy: AllocStrategy::UniqueOwned,
                declared_in_loop_region: loop_region,
            },
        );
        // Track this region at current scope
        if let Some(scope) = self.scope_regions.last_mut() {
            scope.insert(region);
        }
        // Register in NLL context if enabled
        if let Some(nll) = &mut self.nll {
            nll.create_local_region(name);
        }
    }

    /// Declare a function parameter (same as local but marked differently for errors)
    pub fn declare_param(&mut self, name: &str) {
        let region = self.region_gen.fresh();
        self.local_lifetimes.insert(
            name.to_string(),
            LocalLifetime {
                storage_region: region,
                borrowed_by: HashSet::new(),
                holds_borrow: None,
                declared_at_depth: 0, // Parameters live for entire function
                is_param: true,
                is_live: true,
                escapes: EscapeState::NoEscape,
                alloc_strategy: AllocStrategy::UniqueOwned,
                declared_in_loop_region: None, // Parameters are never inside loops
            },
        );
        // Register in NLL context if enabled
        if let Some(nll) = &mut self.nll {
            nll.create_local_region(name);
        }
    }

    // --- Escape Analysis Methods ---

    /// Mark a local as escaping via return
    pub fn mark_escape_return(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            local.escapes = EscapeState::EscapesViaReturn;
            local.alloc_strategy = AllocStrategy::UniqueOwned;
        }
    }

    /// Mark a local as escaping via closure capture
    pub fn mark_escape_closure(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            if local.escapes == EscapeState::NoEscape {
                local.escapes = EscapeState::EscapesViaClosure;
                local.alloc_strategy = AllocStrategy::RefCounted;
            }
        }
    }

    /// Mark a local as escaping via field assignment to an escaping object
    pub fn mark_escape_field(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            if local.escapes == EscapeState::NoEscape {
                local.escapes = EscapeState::EscapesViaField;
                local.alloc_strategy = AllocStrategy::RefCounted;
            }
        }
    }

    /// Mark a local as escaping via provider storage
    pub fn mark_escape_provider(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            local.escapes = EscapeState::EscapesViaProvider;
            local.alloc_strategy = AllocStrategy::RefCounted;
        }
    }

    /// Mark a local as potentially escaping via function call
    pub fn mark_escape_call(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            if local.escapes == EscapeState::NoEscape {
                local.escapes = EscapeState::EscapesViaCall;
                // Conservative: assume RC needed for calls that may store the value
                local.alloc_strategy = AllocStrategy::RefCounted;
            }
        }
    }

    /// Check if a local escapes its scope
    pub fn does_escape(&self, name: &str) -> bool {
        self.local_lifetimes
            .get(name)
            .map(|l| l.escapes != EscapeState::NoEscape)
            .unwrap_or(false)
    }

    /// Check if a local holds a borrow (is itself a borrowed reference)
    /// Returns the borrow origin if it does, None otherwise
    pub fn local_holds_borrow(&self, name: &str) -> Option<&BorrowOrigin> {
        self.local_lifetimes.get(name).and_then(|local| {
            local
                .holds_borrow
                .and_then(|region_id| self.active_borrows.get(&region_id))
                .map(|borrow| &borrow.origin)
        })
    }

    /// Check if a local holds a borrow and return full borrow info
    pub fn get_local_borrow_info(&self, name: &str) -> Option<&BorrowInfo> {
        self.local_lifetimes.get(name).and_then(|local| {
            local
                .holds_borrow
                .and_then(|region_id| self.active_borrows.get(&region_id))
        })
    }

    /// Check if a local is a function parameter (declared at depth 0)
    pub fn is_parameter(&self, name: &str) -> bool {
        self.local_lifetimes
            .get(name)
            .map(|l| l.is_param)
            .unwrap_or(false)
    }

    /// Get the escape state of a local
    pub fn get_escape_state(&self, name: &str) -> Option<&EscapeState> {
        self.local_lifetimes.get(name).map(|l| &l.escapes)
    }

    /// Get the allocation strategy for a local
    pub fn get_alloc_strategy(&self, name: &str) -> Option<&AllocStrategy> {
        self.local_lifetimes.get(name).map(|l| &l.alloc_strategy)
    }

    /// Set stack allocation for a non-escaping local
    pub fn set_stack_alloc(&mut self, name: &str) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            if local.escapes == EscapeState::NoEscape {
                local.alloc_strategy = AllocStrategy::Stack;
            }
        }
    }

    /// Set region allocation for a local with known lifetime
    pub fn set_region_alloc(&mut self, name: &str, region: RegionId) {
        if let Some(local) = self.local_lifetimes.get_mut(name) {
            local.alloc_strategy = AllocStrategy::Region(region);
        }
    }

    // --- Loop Region Methods (for region-based allocation) ---

    /// Enter a loop region - creates a new region for loop-local allocations.
    /// Returns the region ID that should be used for RegionEnter/RegionExit instructions.
    pub fn enter_loop_region(&mut self) -> RegionId {
        let region_id = self.region_gen.fresh();
        let loop_id = self.next_loop_id;
        self.next_loop_id += 1;

        self.loop_regions.push(RegionInfo {
            id: region_id,
            kind: RegionKind::Loop(loop_id),
            depth: self.scope_depth,
        });

        region_id
    }

    /// Exit a loop region - removes the region from the stack.
    /// Values allocated in this region will be bulk-deallocated.
    pub fn exit_loop_region(&mut self, expected_id: RegionId) {
        if let Some(region) = self.loop_regions.last() {
            if region.id == expected_id {
                self.loop_regions.pop();
            }
        }
    }

    /// Get the current loop region (innermost active loop), if any.
    pub fn current_loop_region(&self) -> Option<&RegionInfo> {
        self.loop_regions.last()
    }

    /// Get the current loop region ID, if inside a loop.
    pub fn current_loop_region_id(&self) -> Option<RegionId> {
        self.loop_regions.last().map(|r| r.id)
    }

    /// Check if we're currently inside a loop region.
    pub fn is_in_loop_region(&self) -> bool {
        !self.loop_regions.is_empty()
    }

    /// Get the depth of the current loop region (number of nested loops).
    pub fn loop_region_depth(&self) -> usize {
        self.loop_regions.len()
    }

    /// Get all locals allocated in a specific region.
    pub fn get_locals_in_region(&self, region_id: RegionId) -> Vec<String> {
        self.local_lifetimes
            .iter()
            .filter(|(_, lt)| lt.alloc_strategy == AllocStrategy::Region(region_id))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get all locals that need reference counting
    pub fn get_rc_locals(&self) -> Vec<String> {
        self.local_lifetimes
            .iter()
            .filter(|(_, lt)| lt.alloc_strategy == AllocStrategy::RefCounted)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get all locals that can be stack allocated
    pub fn get_stack_locals(&self) -> Vec<String> {
        self.local_lifetimes
            .iter()
            .filter(|(_, lt)| {
                lt.escapes == EscapeState::NoEscape || lt.alloc_strategy == AllocStrategy::Stack
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Finalize allocation strategies based on escape analysis results.
    /// This should be called after the function body has been fully analyzed.
    /// Non-escaping locals are set to Stack allocation for optimal performance.
    pub fn finalize_allocation_strategies(&mut self) {
        for local in self.local_lifetimes.values_mut() {
            match local.escapes {
                EscapeState::NoEscape => {
                    // Non-escaping values: use region allocation if inside a loop,
                    // otherwise use stack allocation
                    if let Some(loop_region) = local.declared_in_loop_region {
                        // Loop-local value: allocate in region for bulk deallocation
                        local.alloc_strategy = AllocStrategy::Region(loop_region);
                    } else {
                        // Function-scoped non-escaping: stack allocation
                        local.alloc_strategy = AllocStrategy::Stack;
                    }
                }
                EscapeState::EscapesViaReturn => {
                    // Returned values use unique ownership (caller takes over)
                    local.alloc_strategy = AllocStrategy::UniqueOwned;
                }
                EscapeState::EscapesViaClosure
                | EscapeState::EscapesViaField
                | EscapeState::EscapesViaProvider
                | EscapeState::EscapesViaCall
                | EscapeState::MayEscape => {
                    // Values that may be shared need reference counting
                    local.alloc_strategy = AllocStrategy::RefCounted;
                }
            }
        }
    }

    /// Get all locals with their lifetime information (for escape analysis export)
    pub fn get_all_locals(&self) -> impl Iterator<Item = (&String, &LocalLifetime)> {
        self.local_lifetimes.iter()
    }

    /// Get allocation summary for debugging/diagnostics
    pub fn get_allocation_summary(&self) -> AllocationSummary {
        let mut stack_count = 0;
        let mut rc_count = 0;
        let mut unique_count = 0;
        let mut region_count = 0;

        for local in self.local_lifetimes.values() {
            match local.alloc_strategy {
                AllocStrategy::Stack => stack_count += 1,
                AllocStrategy::RefCounted => rc_count += 1,
                AllocStrategy::UniqueOwned => unique_count += 1,
                AllocStrategy::Region(_) => region_count += 1,
            }
        }

        AllocationSummary {
            stack_allocated: stack_count,
            ref_counted: rc_count,
            unique_owned: unique_count,
            region_allocated: region_count,
        }
    }

    /// Create a shared borrow of a local variable
    pub fn borrow_shared(
        &mut self,
        source: &str,
        holder: Option<&str>,
        span: Option<crate::compiler::hir::core::Span>,
    ) -> Result<RegionId, LifetimeError> {
        // Check if source exists
        if !self.local_lifetimes.get(source).is_some_and(|l| l.is_live) {
            return Err(LifetimeError::UnknownSource {
                name: source.to_string(),
            });
        }

        // Check for conflicting exclusive borrow
        if let Some(local) = self.local_lifetimes.get(source) {
            for region in &local.borrowed_by {
                if let Some(borrow) = self.active_borrows.get(region) {
                    if borrow.mode == BorrowMode::Exclusive {
                        return Err(LifetimeError::ConflictingBorrow {
                            source_name: source.to_string(),
                            existing_mode: BorrowMode::Exclusive,
                            existing_span: borrow.span.clone(),
                            new_mode: BorrowMode::Shared,
                            new_span: span,
                        });
                    }
                }
            }
        }

        // Create the borrow
        let region = self.region_gen.fresh();
        let borrow = BorrowInfo {
            region,
            origin: BorrowOrigin::Local(source.to_string()),
            mode: BorrowMode::Shared,
            holder: holder.map(|s| s.to_string()),
            span,
            scope_depth: self.scope_depth,
            creation_site: None, // Populated when call-site HirId plumbed through borrow inference.
        };

        self.active_borrows.insert(region, borrow);

        // Track borrow in source
        if let Some(local) = self.local_lifetimes.get_mut(source) {
            local.borrowed_by.insert(region);
        }

        // Track borrow in holder if specified
        if let Some(holder_name) = holder {
            if let Some(holder_local) = self.local_lifetimes.get_mut(holder_name) {
                holder_local.holds_borrow = Some(region);
            }
        }

        // Track region at current scope
        if let Some(scope) = self.scope_regions.last_mut() {
            scope.insert(region);
        }

        // NLL: Create borrow region and add outlives constraint
        if let Some(nll) = &mut self.nll {
            let nll_borrow_region = nll.create_borrow_region();
            // Get the source's NLL region and add constraint: source outlives borrow
            if let Some(&source_region) = nll.local_regions.get(source) {
                if let Some(source_points) = nll.regions.get_mut(&source_region) {
                    source_points.add_point(nll.current_point);
                }
                nll.add_constraint(
                    source_region,
                    nll_borrow_region,
                    &format!("shared borrow of '{}'", source),
                );
            }
        }

        Ok(region)
    }

    /// Create an exclusive (mutable) borrow of a local variable
    pub fn borrow_exclusive(
        &mut self,
        source: &str,
        holder: Option<&str>,
        span: Option<crate::compiler::hir::core::Span>,
    ) -> Result<RegionId, LifetimeError> {
        // Check if source exists
        if !self.local_lifetimes.get(source).is_some_and(|l| l.is_live) {
            return Err(LifetimeError::UnknownSource {
                name: source.to_string(),
            });
        }

        // Check for any conflicting borrows (exclusive blocks all)
        if let Some(local) = self.local_lifetimes.get(source) {
            for region in &local.borrowed_by {
                if let Some(borrow) = self.active_borrows.get(region) {
                    return Err(LifetimeError::ConflictingBorrow {
                        source_name: source.to_string(),
                        existing_mode: borrow.mode,
                        existing_span: borrow.span.clone(),
                        new_mode: BorrowMode::Exclusive,
                        new_span: span,
                    });
                }
            }
        }

        // Create the borrow
        let region = self.region_gen.fresh();
        let borrow = BorrowInfo {
            region,
            origin: BorrowOrigin::Local(source.to_string()),
            mode: BorrowMode::Exclusive,
            holder: holder.map(|s| s.to_string()),
            span,
            scope_depth: self.scope_depth,
            creation_site: None, // Populated when call-site HirId plumbed through borrow inference.
        };

        self.active_borrows.insert(region, borrow);

        // Track borrow in source
        if let Some(local) = self.local_lifetimes.get_mut(source) {
            local.borrowed_by.insert(region);
        }

        // Track borrow in holder if specified
        if let Some(holder_name) = holder {
            if let Some(holder_local) = self.local_lifetimes.get_mut(holder_name) {
                holder_local.holds_borrow = Some(region);
            }
        }

        // Track region at current scope
        if let Some(scope) = self.scope_regions.last_mut() {
            scope.insert(region);
        }

        // NLL: Create borrow region and add outlives constraint
        if let Some(nll) = &mut self.nll {
            let nll_borrow_region = nll.create_borrow_region();
            // Get the source's NLL region and add constraint: source outlives borrow
            if let Some(&source_region) = nll.local_regions.get(source) {
                if let Some(source_points) = nll.regions.get_mut(&source_region) {
                    source_points.add_point(nll.current_point);
                }
                nll.add_constraint(
                    source_region,
                    nll_borrow_region,
                    &format!("exclusive borrow of '{}'", source),
                );
            }
        }

        Ok(region)
    }

    /// Create a borrow from a provider.
    /// Provider borrows have longer lifetimes than function-scoped borrows.
    /// They are allowed to "escape" the function (stored in locals that may be returned),
    /// as the provider's lifetime extends beyond the function.
    ///
    /// Unlike local borrows, provider borrows don't require the source to be a declared local.
    /// The provider is looked up by name from the global provider registry.
    pub fn borrow_from_provider(
        &mut self,
        provider_name: &str,
        holder: Option<&str>,
        mode: BorrowMode,
        span: Option<crate::compiler::hir::core::Span>,
    ) -> RegionId {
        // A fresh borrow taken after mutation should be considered valid again.
        self.invalidated_providers.remove(provider_name);

        // Create the borrow with provider origin
        let region = self.region_gen.fresh();
        let borrow = BorrowInfo {
            region,
            origin: BorrowOrigin::Provider(provider_name.to_string()),
            mode,
            holder: holder.map(|s| s.to_string()),
            span,
            scope_depth: self.scope_depth,
            creation_site: None,
        };

        self.active_borrows.insert(region, borrow);

        // Track borrow in holder if specified
        if let Some(holder_name) = holder {
            if let Some(holder_local) = self.local_lifetimes.get_mut(holder_name) {
                holder_local.holds_borrow = Some(region);
            }
        }

        // Track region at current scope
        if let Some(scope) = self.scope_regions.last_mut() {
            scope.insert(region);
        }

        region
    }

    /// Invalidate all borrows from a specific provider.
    /// Called when a provider field is mutated, as this may affect the borrowed value.
    pub fn invalidate_provider_borrows(&mut self, provider_name: &str) {
        self.invalidated_providers.insert(provider_name.to_string());
    }

    /// Check if a local variable holds a borrow from an invalidated provider.
    /// Returns Some(provider_name) if the borrow is invalidated, None otherwise.
    pub fn check_invalidated_provider_borrow(&self, local_name: &str) -> Option<String> {
        if let Some(local) = self.local_lifetimes.get(local_name) {
            if let Some(region) = local.holds_borrow {
                if let Some(borrow) = self.active_borrows.get(&region) {
                    if let BorrowOrigin::Provider(ref prov_name) = borrow.origin {
                        if self.invalidated_providers.contains(prov_name) {
                            return Some(prov_name.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// Release a borrow (explicit release() call or scope end)
    pub fn release_borrow(&mut self, source: &str) {
        // First, collect the regions to remove and any holder names
        let mut regions_to_remove: Vec<RegionId> = Vec::new();
        let mut holders_to_clear: Vec<String> = Vec::new();

        if let Some(local) = self.local_lifetimes.get(source) {
            regions_to_remove = local.borrowed_by.iter().copied().collect();
        }

        // Gather holder names from the borrows we're about to remove
        for region in &regions_to_remove {
            if let Some(borrow) = self.active_borrows.get(region) {
                if let Some(ref holder_name) = borrow.holder {
                    holders_to_clear.push(holder_name.clone());
                }
            }
        }

        // Now perform the mutations
        if let Some(local) = self.local_lifetimes.get_mut(source) {
            for region in &regions_to_remove {
                local.borrowed_by.remove(region);
            }
        }

        // Remove from active_borrows
        for region in &regions_to_remove {
            self.active_borrows.remove(region);
        }

        // Clear holder references
        for holder_name in holders_to_clear {
            if let Some(holder_local) = self.local_lifetimes.get_mut(&holder_name) {
                holder_local.holds_borrow = None;
            }
        }
    }

    /// Check if a variable has any active exclusive borrows
    pub fn has_exclusive_borrow(&self, name: &str) -> bool {
        if let Some(local) = self.local_lifetimes.get(name) {
            for region in &local.borrowed_by {
                if let Some(borrow) = self.active_borrows.get(region) {
                    if borrow.mode == BorrowMode::Exclusive {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if a variable has any active borrows (shared or exclusive)
    pub fn has_any_borrow(&self, name: &str) -> bool {
        if let Some(local) = self.local_lifetimes.get(name) {
            !local.borrowed_by.is_empty()
        } else {
            false
        }
    }

    /// Get all active borrows of a variable
    pub fn get_borrows(&self, name: &str) -> Vec<&BorrowInfo> {
        let mut result = Vec::new();
        if let Some(local) = self.local_lifetimes.get(name) {
            for region in &local.borrowed_by {
                if let Some(borrow) = self.active_borrows.get(region) {
                    result.push(borrow);
                }
            }
        }
        result
    }

    /// Check if moving a value would invalidate any active borrows
    pub fn check_move(&self, name: &str) -> Option<LifetimeError> {
        if let Some(local) = self.local_lifetimes.get(name) {
            if !local.borrowed_by.is_empty() {
                // Find the first active borrow for the error message
                for region in &local.borrowed_by {
                    if let Some(borrow) = self.active_borrows.get(region) {
                        return Some(LifetimeError::MoveWhileBorrowed {
                            name: name.to_string(),
                            borrow_mode: borrow.mode,
                            borrow_span: borrow.span.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    /// Check if assigning to a value would invalidate any active borrows
    pub fn check_assign(&self, name: &str) -> Option<LifetimeError> {
        // Assignment is like a move followed by initialization - invalidates borrows
        self.check_move(name)
    }

    /// Validate at function exit - no borrows should escape
    pub fn check_function_exit(&self) -> Vec<LifetimeError> {
        let mut errors = Vec::new();

        // Check for any borrows still active at function exit
        for (_, borrow) in &self.active_borrows {
            // Borrows from providers are allowed to "escape" (they have longer lifetime)
            if matches!(borrow.origin, BorrowOrigin::Provider(_)) {
                continue;
            }

            errors.push(LifetimeError::BorrowEscapesFunction {
                origin: borrow.origin.clone(),
                mode: borrow.mode,
                span: borrow.span.clone(),
            });
        }

        // Additionally, ensure that closure-captured locals do not hold borrows
        // that would outlive their source values. If a local both escapes via
        // closure capture and still holds an active borrow region, report this
        // as a dedicated error so diagnostics can point at the capture site.
        for (name, local) in &self.local_lifetimes {
            if local.escapes == EscapeState::EscapesViaClosure {
                if let Some(region) = local.holds_borrow {
                    if let Some(borrow) = self.active_borrows.get(&region) {
                        errors.push(LifetimeError::BorrowCapturedByClosure {
                            holder: name.clone(),
                            origin: borrow.origin.clone(),
                            mode: borrow.mode,
                            span: borrow.span.clone(),
                        });
                    }
                }
            }
        }

        errors
    }

    /// Check if a borrow would cross an await boundary
    /// Returns errors for exclusive borrows (which are forbidden) and
    /// a full analysis of all live borrows at the await point.
    pub fn check_await_boundary(&self) -> Vec<LifetimeError> {
        let mut errors = Vec::new();

        for (_, borrow) in &self.active_borrows {
            if borrow.mode == BorrowMode::Exclusive {
                errors.push(LifetimeError::ExclusiveBorrowAcrossAwait {
                    origin: borrow.origin.clone(),
                    span: borrow.span.clone(),
                });
            }
        }

        errors
    }

    /// Get detailed analysis of all borrows live at current point.
    /// This is useful for understanding what state is held across await points.
    /// Returns a list of (holder_name, borrow_mode, origin) tuples.
    pub fn get_live_borrows(&self) -> Vec<(Option<String>, BorrowMode, BorrowOrigin)> {
        self.active_borrows
            .values()
            .map(|b| (b.holder.clone(), b.mode, b.origin.clone()))
            .collect()
    }

    /// Check await boundary with full analysis - returns both errors and all live borrows
    /// for comprehensive diagnostics.
    pub fn check_await_boundary_full(&self) -> (Vec<LifetimeError>, Vec<AwaitBorrowInfo>) {
        let mut errors = Vec::new();
        let mut live_borrows = Vec::new();

        for (region, borrow) in &self.active_borrows {
            let info = AwaitBorrowInfo {
                region: *region,
                holder: borrow.holder.clone(),
                mode: borrow.mode,
                origin: borrow.origin.clone(),
                span: borrow.span.clone(),
            };
            live_borrows.push(info);

            // Exclusive borrows are errors
            if borrow.mode == BorrowMode::Exclusive {
                errors.push(LifetimeError::ExclusiveBorrowAcrossAwait {
                    origin: borrow.origin.clone(),
                    span: borrow.span.clone(),
                });
            }
        }

        (errors, live_borrows)
    }

    /// Join two lifetime environments from different control flow paths
    /// Returns the joined environment and any errors from conflicting states
    pub fn join(&self, other: &Self) -> (Self, Vec<LifetimeError>) {
        let errors = Vec::new();

        // For lifetime tracking, we take the conservative union of borrows
        // A borrow is active after join if it's active in either branch
        let mut joined = self.clone();

        // Merge active borrows from other
        for (region, borrow) in &other.active_borrows {
            if !joined.active_borrows.contains_key(region) {
                joined.active_borrows.insert(*region, borrow.clone());
            }
        }

        // Merge local lifetimes
        for (name, local) in &other.local_lifetimes {
            if let Some(existing) = joined.local_lifetimes.get_mut(name) {
                // Union the borrowed_by sets
                for region in &local.borrowed_by {
                    existing.borrowed_by.insert(*region);
                }
            } else {
                joined.local_lifetimes.insert(name.clone(), local.clone());
            }
        }

        (joined, errors)
    }

    /// Get all locals with active borrows (for diagnostics)
    pub fn locals_with_borrows(&self) -> Vec<String> {
        self.local_lifetimes
            .iter()
            .filter(|(_, lt)| !lt.borrowed_by.is_empty())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Perform full function exit check, including NLL analysis if enabled.
    /// This is the main entry point for checking borrows at function exit.
    /// Returns all lifetime errors found, from both lexical and NLL analysis.
    pub fn check_function_exit_full(&self) -> Vec<LifetimeError> {
        // Start with lexical lifetime errors
        let mut errors = self.check_function_exit();

        // If NLL is enabled, also solve constraints and convert errors
        if let Some(nll) = &self.nll {
            for nll_error in nll.solve_constraints() {
                // Convert NLL errors to LifetimeErrors
                let lifetime_error = match nll_error {
                    NllError::BorrowOutlivesSource {
                        borrow_region,
                        reason,
                        ..
                    } => LifetimeError::BorrowOutlivesSource {
                        borrow_region,
                        source_name: reason.clone(),
                        borrow_span: None,
                    },
                    NllError::ConflictingBorrows {
                        first_reason,
                        second_reason: _,
                        ..
                    } => LifetimeError::ConflictingBorrow {
                        source_name: first_reason.clone(),
                        existing_mode: BorrowMode::Shared,
                        existing_span: None,
                        new_mode: BorrowMode::Shared,
                        new_span: None,
                    },
                    NllError::UseAfterMove { variable, .. } => LifetimeError::MoveWhileBorrowed {
                        name: variable.clone(),
                        borrow_mode: BorrowMode::Shared,
                        borrow_span: None,
                    },
                };
                errors.push(lifetime_error);
            }
        }

        errors
    }

    /// Get NLL diagnostics with full error context.
    /// Returns detailed diagnostics including notes and suggestions.
    pub fn get_nll_diagnostics(&self) -> Vec<NllDiagnostic> {
        if let Some(nll) = &self.nll {
            nll.solve_constraints()
                .into_iter()
                .map(|e| e.to_diagnostic())
                .collect()
        } else {
            Vec::new()
        }
    }
}

impl Default for LifetimeEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur during lifetime checking
#[derive(Clone, Debug)]
pub enum LifetimeError {
    /// Borrow conflicts with an existing borrow
    ConflictingBorrow {
        source_name: String,
        existing_mode: BorrowMode,
        existing_span: Option<crate::compiler::hir::core::Span>,
        new_mode: BorrowMode,
        new_span: Option<crate::compiler::hir::core::Span>,
    },

    /// Attempted to borrow from unknown source
    UnknownSource { name: String },

    /// Borrow outlives its source (source dropped while borrow still active)
    BorrowOutlivesSource {
        borrow_region: RegionId,
        source_name: String,
        borrow_span: Option<crate::compiler::hir::core::Span>,
    },

    /// Tried to move a value while it's borrowed
    MoveWhileBorrowed {
        name: String,
        borrow_mode: BorrowMode,
        borrow_span: Option<crate::compiler::hir::core::Span>,
    },

    /// Borrow escapes function scope
    BorrowEscapesFunction {
        origin: BorrowOrigin,
        mode: BorrowMode,
        span: Option<crate::compiler::hir::core::Span>,
    },

    /// Closure captures a value that holds a borrow which may outlive its source
    BorrowCapturedByClosure {
        /// Name of the local captured by the closure (the borrow holder)
        holder: String,
        /// Where the underlying borrow comes from (local/param/field/provider)
        origin: BorrowOrigin,
        /// Kind of borrow captured (shared/exclusive)
        mode: BorrowMode,
        /// Span where the borrow was originally created (if known)
        span: Option<crate::compiler::hir::core::Span>,
    },

    /// Exclusive borrow cannot cross await boundary
    ExclusiveBorrowAcrossAwait {
        origin: BorrowOrigin,
        span: Option<crate::compiler::hir::core::Span>,
    },

    /// Provider field cannot hold borrow of function-scoped value
    ProviderHoldsBorrow {
        /// Name of the provider being assigned to
        provider_name: String,
        /// Field being assigned
        field_name: String,
        /// The variable that holds a borrow being assigned
        borrow_holder: String,
        /// Origin of the borrow being stored
        origin: BorrowOrigin,
        /// Kind of borrow being stored
        mode: BorrowMode,
    },

    /// Lifetime parameter conflict in function signature
    SignatureLifetimeConflict {
        param_name: String,
        return_type: String,
        message: String,
    },
}

impl LifetimeError {
    /// Convert to a diagnostic message
    pub fn to_message(&self) -> String {
        match self {
            LifetimeError::ConflictingBorrow {
                source_name,
                existing_mode,
                new_mode,
                ..
            } => {
                let existing_str = match existing_mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                let new_str = match new_mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                format!(
                    "cannot take {} borrow of '{}' while {} borrow is active",
                    new_str, source_name, existing_str
                )
            }

            LifetimeError::UnknownSource { name } => {
                format!("cannot borrow '{}': not found in scope", name)
            }

            LifetimeError::BorrowOutlivesSource {
                source_name,
                borrow_span,
                ..
            } => {
                let span_info = if borrow_span.is_some() {
                    " (borrow created here)"
                } else {
                    ""
                };
                format!(
                    "borrow of '{}' outlives the value{}",
                    source_name, span_info
                )
            }

            LifetimeError::MoveWhileBorrowed {
                name, borrow_mode, ..
            } => {
                let mode_str = match borrow_mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                format!(
                    "cannot move '{}' while it has an active {} borrow",
                    name, mode_str
                )
            }

            LifetimeError::BorrowEscapesFunction { origin, mode, .. } => {
                let mode_str = match mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                let origin_str = match origin {
                    BorrowOrigin::Local(name) => format!("local '{}'", name),
                    BorrowOrigin::Param(name) => format!("parameter '{}'", name),
                    BorrowOrigin::Field(base, path) => {
                        format!("field '{}.{}'", base, path.join("."))
                    }
                    BorrowOrigin::Provider(name) => format!("provider '{}'", name),
                    BorrowOrigin::Unknown => "unknown".to_string(),
                };
                format!(
                    "{} borrow of {} escapes function scope",
                    mode_str, origin_str
                )
            }

            LifetimeError::BorrowCapturedByClosure {
                holder,
                origin,
                mode,
                ..
            } => {
                let mode_str = match mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                let origin_str = match origin {
                    BorrowOrigin::Local(name) => format!("local '{}'", name),
                    BorrowOrigin::Param(name) => format!("parameter '{}'", name),
                    BorrowOrigin::Field(base, path) => {
                        format!("field '{}.{}'", base, path.join("."))
                    }
                    BorrowOrigin::Provider(name) => format!("provider '{}'", name),
                    BorrowOrigin::Unknown => "unknown".to_string(),
                };
                format!(
                    "closure captures {} borrow held in '{}' from {}; borrow cannot outlive its source",
                    mode_str, holder, origin_str
                )
            }

            LifetimeError::ExclusiveBorrowAcrossAwait { origin, .. } => {
                let origin_str = match origin {
                    BorrowOrigin::Local(name) => format!("'{}'", name),
                    BorrowOrigin::Param(name) => format!("'{}'", name),
                    BorrowOrigin::Field(base, path) => format!("'{}.{}'", base, path.join(".")),
                    BorrowOrigin::Provider(name) => format!("provider '{}'", name),
                    BorrowOrigin::Unknown => "unknown".to_string(),
                };
                format!(
                    "exclusive borrow of {} cannot cross await boundary; \
                     release the borrow before await",
                    origin_str
                )
            }

            LifetimeError::ProviderHoldsBorrow {
                provider_name,
                field_name,
                borrow_holder,
                origin,
                mode,
            } => {
                let mode_str = match mode {
                    BorrowMode::Shared => "shared",
                    BorrowMode::Exclusive => "exclusive",
                };
                let origin_str = match origin {
                    BorrowOrigin::Local(name) => format!("'{}'", name),
                    BorrowOrigin::Param(name) => format!("parameter '{}'", name),
                    BorrowOrigin::Field(base, path) => format!("'{}.{}'", base, path.join(".")),
                    BorrowOrigin::Provider(name) => format!("provider '{}'", name),
                    BorrowOrigin::Unknown => "unknown source".to_string(),
                };
                format!(
                    "cannot store '{}' in provider field '{}.{}': it holds a {} borrow of {}; \
                     provider fields cannot hold borrows to function-scoped values",
                    borrow_holder, provider_name, field_name, mode_str, origin_str
                )
            }

            LifetimeError::SignatureLifetimeConflict { message, .. } => message.clone(),
        }
    }

    /// Get the span associated with this error (if any)
    pub fn span(&self) -> Option<&crate::compiler::hir::core::Span> {
        match self {
            LifetimeError::ConflictingBorrow { new_span, .. } => new_span.as_ref(),
            LifetimeError::BorrowOutlivesSource { borrow_span, .. } => borrow_span.as_ref(),
            LifetimeError::MoveWhileBorrowed { borrow_span, .. } => borrow_span.as_ref(),
            LifetimeError::BorrowEscapesFunction { span, .. } => span.as_ref(),
            LifetimeError::BorrowCapturedByClosure { span, .. } => span.as_ref(),
            LifetimeError::ExclusiveBorrowAcrossAwait { span, .. } => span.as_ref(),
            LifetimeError::ProviderHoldsBorrow { .. } => None, // No span stored for this error
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_borrow() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        // Should succeed
        let result = env.borrow_shared("x", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_shared_borrows() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        // Multiple shared borrows should succeed
        assert!(env.borrow_shared("x", None, None).is_ok());
        assert!(env.borrow_shared("x", None, None).is_ok());
    }

    #[test]
    fn test_exclusive_blocks_shared() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        // First exclusive borrow succeeds
        assert!(env.borrow_exclusive("x", None, None).is_ok());

        // Second borrow (shared) should fail
        let result = env.borrow_shared("x", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_shared_blocks_exclusive() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        // First shared borrow succeeds
        assert!(env.borrow_shared("x", None, None).is_ok());

        // Exclusive borrow should fail
        let result = env.borrow_exclusive("x", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_release_allows_reborrow() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        assert!(env.borrow_exclusive("x", None, None).is_ok());
        env.release_borrow("x");

        // After release, can borrow again
        assert!(env.borrow_exclusive("x", None, None).is_ok());
    }

    #[test]
    fn test_move_while_borrowed_error() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");
        env.borrow_shared("x", None, None).unwrap();

        // Moving should fail
        assert!(env.check_move("x").is_some());
    }

    #[test]
    fn test_scope_ends_borrows() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");

        env.push_scope();
        assert!(env.borrow_exclusive("x", None, None).is_ok());

        // After scope exit, borrow should be released
        let errors = env.pop_scope();
        assert!(errors.is_empty());

        // Can borrow again
        assert!(env.borrow_exclusive("x", None, None).is_ok());
    }

    #[test]
    fn test_borrow_outlives_source() {
        let mut env = LifetimeEnv::new();

        // Declare x in inner scope
        env.push_scope();
        env.declare_local("x");

        // Borrow x in outer scope (simulate storing in outer variable)
        // This is a simplified test - real borrowing to outer scope would be different
        assert!(env.borrow_shared("x", None, None).is_ok());

        // When inner scope ends, x goes out of scope but borrow might still be active
        // This test validates the scope_regions tracking
        let errors = env.pop_scope();
        // The borrow was created in the popped scope, so it's cleaned up
        assert!(errors.is_empty());
    }

    #[test]
    fn test_function_exit_check() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");
        env.borrow_exclusive("x", None, None).unwrap();

        // Function exit with active borrow should error
        let errors = env.check_function_exit();
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_closure_capture_borrow_reports_error() {
        let mut env = LifetimeEnv::new();

        // Local value that will be the source of a borrow
        env.declare_local("x");
        // Local that conceptually represents a closure value
        env.declare_local("closure");

        // Create a shared borrow of `x` that is held by `closure`
        env.borrow_shared("x", Some("closure"), None).unwrap();

        // Mark the closure local as escaping via closure capture
        env.mark_escape_closure("closure");

        // At function exit, this should be reported as a closure capturing
        // a borrowed value that would outlive its source.
        let errors = env.check_function_exit();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                LifetimeError::BorrowCapturedByClosure { holder, .. } if holder == "closure"
            )),
            "expected BorrowCapturedByClosure error for 'closure'"
        );
    }

    #[test]
    fn test_await_boundary_check() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");
        env.borrow_exclusive("x", None, None).unwrap();

        // Exclusive borrow across await should error
        let errors = env.check_await_boundary();
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_shared_borrow_across_await_ok() {
        let mut env = LifetimeEnv::new();
        env.declare_local("x");
        env.borrow_shared("x", None, None).unwrap();

        // Shared borrow across await is allowed
        let errors = env.check_await_boundary();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_join_environments() {
        let mut env1 = LifetimeEnv::new();
        env1.declare_local("x");
        env1.borrow_shared("x", None, None).unwrap();

        let mut env2 = LifetimeEnv::new();
        env2.declare_local("x");
        // No borrow in env2

        let (joined, errors) = env1.join(&env2);
        assert!(errors.is_empty());
        // After join, borrow should still be active (conservative)
        assert!(joined.has_any_borrow("x"));
    }

    // --- Loop Region Tests ---

    #[test]
    fn test_enter_loop_region() {
        let mut env = LifetimeEnv::new();

        // Initially not in a loop region
        assert!(!env.is_in_loop_region());
        assert!(env.current_loop_region_id().is_none());

        // Enter a loop region
        let region_id = env.enter_loop_region();
        assert!(env.is_in_loop_region());
        assert_eq!(env.current_loop_region_id(), Some(region_id));
        assert_eq!(env.loop_region_depth(), 1);
    }

    #[test]
    fn test_exit_loop_region() {
        let mut env = LifetimeEnv::new();

        let region_id = env.enter_loop_region();
        assert!(env.is_in_loop_region());

        // Exit the loop region
        env.exit_loop_region(region_id);
        assert!(!env.is_in_loop_region());
        assert!(env.current_loop_region_id().is_none());
    }

    #[test]
    fn test_nested_loop_regions() {
        let mut env = LifetimeEnv::new();

        // Enter outer loop
        let outer_region = env.enter_loop_region();
        assert_eq!(env.loop_region_depth(), 1);

        // Enter inner loop
        let inner_region = env.enter_loop_region();
        assert_eq!(env.loop_region_depth(), 2);
        assert_eq!(env.current_loop_region_id(), Some(inner_region));

        // Exit inner loop
        env.exit_loop_region(inner_region);
        assert_eq!(env.loop_region_depth(), 1);
        assert_eq!(env.current_loop_region_id(), Some(outer_region));

        // Exit outer loop
        env.exit_loop_region(outer_region);
        assert_eq!(env.loop_region_depth(), 0);
        assert!(!env.is_in_loop_region());
    }

    #[test]
    fn test_local_declared_in_loop_region() {
        let mut env = LifetimeEnv::new();

        // Declare x outside loop
        env.declare_local("x");

        // Enter loop and declare y
        let region_id = env.enter_loop_region();
        env.declare_local("y");

        // Check that y is tracked in the loop region
        let y_lifetime = env.local_lifetimes.get("y").unwrap();
        assert_eq!(y_lifetime.declared_in_loop_region, Some(region_id));

        // x should not be in a loop region
        let x_lifetime = env.local_lifetimes.get("x").unwrap();
        assert_eq!(x_lifetime.declared_in_loop_region, None);
    }

    #[test]
    fn test_finalize_allocates_loop_locals_to_region() {
        let mut env = LifetimeEnv::new();

        // Declare x outside loop (should get Stack allocation)
        env.declare_local("x");

        // Enter loop and declare y (should get Region allocation)
        let region_id = env.enter_loop_region();
        env.declare_local("y");

        // Finalize allocation strategies
        env.finalize_allocation_strategies();

        // x should be Stack allocated (non-escaping, not in loop)
        let x_strategy = env.get_alloc_strategy("x").unwrap();
        assert_eq!(*x_strategy, AllocStrategy::Stack);

        // y should be Region allocated (non-escaping, in loop)
        let y_strategy = env.get_alloc_strategy("y").unwrap();
        assert_eq!(*y_strategy, AllocStrategy::Region(region_id));
    }

    #[test]
    fn test_escaping_loop_local_gets_rc() {
        let mut env = LifetimeEnv::new();

        // Enter loop and declare y
        let _region_id = env.enter_loop_region();
        env.declare_local("y");

        // Mark y as escaping via closure
        env.mark_escape_closure("y");

        // Finalize allocation strategies
        env.finalize_allocation_strategies();

        // y should be RefCounted (escaping overrides region allocation)
        let y_strategy = env.get_alloc_strategy("y").unwrap();
        assert_eq!(*y_strategy, AllocStrategy::RefCounted);
    }

    #[test]
    fn test_get_locals_in_region() {
        let mut env = LifetimeEnv::new();

        // Enter loop and declare multiple locals
        let region_id = env.enter_loop_region();
        env.declare_local("a");
        env.declare_local("b");
        env.exit_loop_region(region_id);

        // Declare c outside loop
        env.declare_local("c");

        // Finalize to assign allocation strategies
        env.finalize_allocation_strategies();

        // Only a and b should be in the region
        let region_locals = env.get_locals_in_region(region_id);
        assert_eq!(region_locals.len(), 2);
        assert!(region_locals.contains(&"a".to_string()));
        assert!(region_locals.contains(&"b".to_string()));
        assert!(!region_locals.contains(&"c".to_string()));
    }

    #[test]
    fn test_allocation_summary_with_regions() {
        let mut env = LifetimeEnv::new();

        // Declare x outside loop
        env.declare_local("x");

        // Enter loop and declare y, z
        let _region_id = env.enter_loop_region();
        env.declare_local("y");
        env.declare_local("z");

        // Mark z as escaping
        env.mark_escape_closure("z");

        // Finalize
        env.finalize_allocation_strategies();

        let summary = env.get_allocation_summary();
        assert_eq!(summary.stack_allocated, 1); // x
        assert_eq!(summary.region_allocated, 1); // y
        assert_eq!(summary.ref_counted, 1); // z
    }
}
