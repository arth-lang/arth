//! Non-Lexical Lifetime (NLL) Analysis
//!
//! This module implements NLL-style borrow checking for Arth. Unlike lexical lifetimes
//! where borrows live until the end of their declaring scope, NLL computes the actual
//! live range of each borrow based on its uses.
//!
//! ## Key Concepts
//!
//! - **Program Point**: A location in the program (statement index within a block)
//! - **Live Range**: The set of program points where a borrow is live
//! - **Region**: A set of program points; borrows have regions, sources have regions
//! - **Outlives Constraint**: `'a: 'b` means region 'a contains all points in 'b
//!
//! ## Algorithm Overview
//!
//! 1. Build a simplified CFG from HIR statements
//! 2. For each borrow, compute its live range (creation to last use)
//! 3. Generate outlives constraints at borrow points
//! 4. Solve constraints via fixed-point iteration
//! 5. Report errors for unsatisfiable constraints

use std::collections::{HashMap, HashSet, VecDeque};

use super::lifetime::RegionId;

/// A program point identifies a specific location in the program.
/// Points are used to track where borrows are live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProgramPoint {
    /// Index of the basic block (0 for entry, incremented at control flow splits)
    pub block: u32,
    /// Index of the statement within the block
    pub stmt: u32,
}

impl ProgramPoint {
    pub fn new(block: u32, stmt: u32) -> Self {
        Self { block, stmt }
    }

    /// The entry point of a function
    pub fn entry() -> Self {
        Self { block: 0, stmt: 0 }
    }
}

impl std::fmt::Display for ProgramPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bb{}:{}", self.block, self.stmt)
    }
}

/// A region is a set of program points where a value or borrow is live.
#[derive(Clone, Debug, Default)]
pub struct Region {
    /// The set of program points in this region
    pub points: HashSet<ProgramPoint>,
}

impl Region {
    pub fn new() -> Self {
        Self {
            points: HashSet::new(),
        }
    }

    /// Add a program point to this region
    pub fn add_point(&mut self, point: ProgramPoint) {
        self.points.insert(point);
    }

    /// Check if this region contains a point
    pub fn contains(&self, point: &ProgramPoint) -> bool {
        self.points.contains(point)
    }

    /// Check if this region is empty
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Union with another region
    pub fn union(&mut self, other: &Region) {
        self.points.extend(other.points.iter().copied());
    }

    /// Check if this region outlives another (contains all its points)
    pub fn outlives(&self, other: &Region) -> bool {
        other.points.iter().all(|p| self.points.contains(p))
    }
}

/// An outlives constraint: the source region must outlive the borrow region.
/// Written as `source: borrow` meaning source's region must contain all points in borrow's region.
#[derive(Clone, Debug)]
pub struct OutlivesConstraint {
    /// The region that must outlive
    pub source: RegionId,
    /// The region that must be outlived
    pub borrow: RegionId,
    /// Where this constraint was created (for error messages)
    pub origin: ProgramPoint,
    /// Description for error messages
    pub reason: String,
}

/// A basic block in the HIR-level CFG.
#[derive(Clone, Debug)]
pub struct HirBlock {
    /// Unique ID for this block
    pub id: u32,
    /// Number of statements in this block
    pub stmt_count: u32,
    /// Successor block IDs
    pub successors: Vec<u32>,
    /// Predecessor block IDs
    pub predecessors: Vec<u32>,
}

impl HirBlock {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            stmt_count: 0,
            successors: Vec::new(),
            predecessors: Vec::new(),
        }
    }
}

/// A simplified CFG built from HIR for lifetime analysis.
#[derive(Clone, Debug)]
pub struct HirCfg {
    /// All basic blocks
    pub blocks: Vec<HirBlock>,
    /// Entry block ID (always 0)
    pub entry: u32,
    /// Exit block IDs (blocks that return or throw)
    pub exits: Vec<u32>,
}

impl HirCfg {
    /// Create a new CFG with a single entry block
    pub fn new() -> Self {
        let entry_block = HirBlock::new(0);
        Self {
            blocks: vec![entry_block],
            entry: 0,
            exits: Vec::new(),
        }
    }

    /// Get the current block being built
    pub fn current_block(&self) -> u32 {
        (self.blocks.len() - 1) as u32
    }

    /// Add a new block and return its ID
    pub fn add_block(&mut self) -> u32 {
        let id = self.blocks.len() as u32;
        self.blocks.push(HirBlock::new(id));
        id
    }

    /// Record a statement in the current block
    pub fn add_stmt(&mut self) -> ProgramPoint {
        let block_id = self.current_block();
        let block = &mut self.blocks[block_id as usize];
        let stmt_id = block.stmt_count;
        block.stmt_count += 1;
        ProgramPoint::new(block_id, stmt_id)
    }

    /// Add an edge from current block to target
    pub fn add_edge(&mut self, from: u32, to: u32) {
        if let Some(from_block) = self.blocks.get_mut(from as usize) {
            if !from_block.successors.contains(&to) {
                from_block.successors.push(to);
            }
        }
        if let Some(to_block) = self.blocks.get_mut(to as usize) {
            if !to_block.predecessors.contains(&from) {
                to_block.predecessors.push(from);
            }
        }
    }

    /// Mark a block as an exit
    pub fn mark_exit(&mut self, block: u32) {
        if !self.exits.contains(&block) {
            self.exits.push(block);
        }
    }

    /// Get all program points in the CFG
    pub fn all_points(&self) -> Vec<ProgramPoint> {
        let mut points = Vec::new();
        for block in &self.blocks {
            for stmt in 0..block.stmt_count {
                points.push(ProgramPoint::new(block.id, stmt));
            }
        }
        points
    }

    /// Compute reverse postorder for iteration
    pub fn reverse_postorder(&self) -> Vec<u32> {
        let mut visited = HashSet::new();
        let mut postorder = Vec::new();

        fn dfs(cfg: &HirCfg, block: u32, visited: &mut HashSet<u32>, postorder: &mut Vec<u32>) {
            if visited.contains(&block) {
                return;
            }
            visited.insert(block);

            if let Some(b) = cfg.blocks.get(block as usize) {
                for &succ in &b.successors {
                    dfs(cfg, succ, visited, postorder);
                }
            }
            postorder.push(block);
        }

        dfs(self, self.entry, &mut visited, &mut postorder);
        postorder.reverse();
        postorder
    }
}

impl Default for HirCfg {
    fn default() -> Self {
        Self::new()
    }
}

/// Liveness information for a borrow.
#[derive(Clone, Debug)]
pub struct BorrowLiveness {
    /// The region ID of this borrow
    pub region: RegionId,
    /// Program point where the borrow was created
    pub creation: ProgramPoint,
    /// Program points where the borrow is used
    pub uses: Vec<ProgramPoint>,
    /// Last use point (for NLL - borrow ends here, not at scope end)
    pub last_use: Option<ProgramPoint>,
    /// The computed live region (creation to last use)
    pub live_region: Region,
}

impl BorrowLiveness {
    pub fn new(region: RegionId, creation: ProgramPoint) -> Self {
        let mut live_region = Region::new();
        live_region.add_point(creation);

        Self {
            region,
            creation,
            uses: Vec::new(),
            last_use: None,
            live_region,
        }
    }

    /// Record a use of this borrow
    pub fn add_use(&mut self, point: ProgramPoint) {
        self.uses.push(point);
        self.live_region.add_point(point);
        // Update last use if this is later
        match self.last_use {
            None => self.last_use = Some(point),
            Some(prev) => {
                // Simple comparison: later block or same block with later stmt
                if point.block > prev.block || (point.block == prev.block && point.stmt > prev.stmt)
                {
                    self.last_use = Some(point);
                }
            }
        }
    }
}

/// The NLL analysis context.
#[derive(Clone, Debug)]
pub struct NllContext {
    /// The CFG for the current function
    pub cfg: HirCfg,
    /// Current program point (advances as we type-check statements)
    pub current_point: ProgramPoint,
    /// Regions for each RegionId
    pub regions: HashMap<RegionId, Region>,
    /// Liveness info for each borrow
    pub borrow_liveness: HashMap<RegionId, BorrowLiveness>,
    /// Outlives constraints
    pub constraints: Vec<OutlivesConstraint>,
    /// Region ID for each local variable (for outlives checking)
    pub local_regions: HashMap<String, RegionId>,
    /// Next region ID to assign
    next_region: u32,
}

impl NllContext {
    pub fn new() -> Self {
        Self {
            cfg: HirCfg::new(),
            current_point: ProgramPoint::entry(),
            regions: HashMap::new(),
            borrow_liveness: HashMap::new(),
            constraints: Vec::new(),
            local_regions: HashMap::new(),
            next_region: 0,
        }
    }

    /// Advance to the next statement and return the new program point
    pub fn advance(&mut self) -> ProgramPoint {
        self.current_point = self.cfg.add_stmt();
        self.current_point
    }

    /// Create a new region for a local variable
    pub fn create_local_region(&mut self, name: &str) -> RegionId {
        let region_id = RegionId::new(self.next_region);
        self.next_region += 1;
        self.regions.insert(region_id, Region::new());
        self.local_regions.insert(name.to_string(), region_id);
        region_id
    }

    /// Create a new region for a borrow
    pub fn create_borrow_region(&mut self) -> RegionId {
        let region_id = RegionId::new(self.next_region);
        self.next_region += 1;

        let liveness = BorrowLiveness::new(region_id, self.current_point);
        self.borrow_liveness.insert(region_id, liveness);

        let mut region = Region::new();
        region.add_point(self.current_point);
        self.regions.insert(region_id, region);

        region_id
    }

    /// Record a use of a borrow at the current point
    pub fn record_borrow_use(&mut self, region: RegionId) {
        if let Some(liveness) = self.borrow_liveness.get_mut(&region) {
            liveness.add_use(self.current_point);
        }
        if let Some(region_set) = self.regions.get_mut(&region) {
            region_set.add_point(self.current_point);
        }
    }

    /// Add an outlives constraint
    pub fn add_constraint(&mut self, source: RegionId, borrow: RegionId, reason: &str) {
        self.constraints.push(OutlivesConstraint {
            source,
            borrow,
            origin: self.current_point,
            reason: reason.to_string(),
        });
    }

    /// Enter a new control flow block (if/else branch, loop body)
    pub fn enter_block(&mut self) -> u32 {
        let new_block = self.cfg.add_block();
        let current = self.cfg.current_block();
        if current != new_block {
            self.cfg.add_edge(current - 1, new_block);
        }
        self.current_point = ProgramPoint::new(new_block, 0);
        new_block
    }

    /// Exit current block and create a join point
    pub fn create_join_block(&mut self, predecessors: &[u32]) -> u32 {
        let join = self.cfg.add_block();
        for &pred in predecessors {
            self.cfg.add_edge(pred, join);
        }
        self.current_point = ProgramPoint::new(join, 0);
        join
    }

    /// Mark current block as function exit
    pub fn mark_exit(&mut self) {
        self.cfg.mark_exit(self.cfg.current_block());
    }

    /// Solve constraints and return errors for unsatisfiable ones
    pub fn solve_constraints(&self) -> Vec<NllError> {
        let mut errors = Vec::new();

        for constraint in &self.constraints {
            let source_region = self.regions.get(&constraint.source);
            let borrow_region = self.regions.get(&constraint.borrow);

            match (source_region, borrow_region) {
                (Some(source), Some(borrow)) => {
                    if !source.outlives(borrow) {
                        // Find the offending point
                        let missing_points: Vec<_> = borrow
                            .points
                            .iter()
                            .filter(|p| !source.points.contains(p))
                            .collect();

                        errors.push(NllError::BorrowOutlivesSource {
                            source_region: constraint.source,
                            borrow_region: constraint.borrow,
                            at_point: constraint.origin,
                            missing_points: missing_points.into_iter().copied().collect(),
                            reason: constraint.reason.clone(),
                        });
                    }
                }
                _ => {
                    // Region not found - should not happen in well-formed code
                }
            }
        }

        errors
    }

    /// Get the last use point for a borrow (for NLL-style shorter lifetimes)
    pub fn get_borrow_last_use(&self, region: RegionId) -> Option<ProgramPoint> {
        self.borrow_liveness.get(&region).and_then(|l| l.last_use)
    }

    /// Check if a borrow is still live at the current point
    pub fn is_borrow_live(&self, region: RegionId) -> bool {
        if let Some(liveness) = self.borrow_liveness.get(&region) {
            // A borrow is live from creation to last use
            if let Some(last_use) = liveness.last_use {
                // Current point is before or at last use
                self.current_point.block < last_use.block
                    || (self.current_point.block == last_use.block
                        && self.current_point.stmt <= last_use.stmt)
            } else {
                // No uses yet - still live from creation
                self.current_point.block >= liveness.creation.block
            }
        } else {
            false
        }
    }
}

impl Default for NllContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from NLL analysis
#[derive(Clone, Debug)]
pub enum NllError {
    /// A borrow outlives its source value
    BorrowOutlivesSource {
        source_region: RegionId,
        borrow_region: RegionId,
        at_point: ProgramPoint,
        missing_points: Vec<ProgramPoint>,
        reason: String,
    },
    /// Conflicting borrows at the same program point
    ConflictingBorrows {
        first_region: RegionId,
        second_region: RegionId,
        at_point: ProgramPoint,
        first_reason: String,
        second_reason: String,
    },
    /// Use after move detected by NLL
    UseAfterMove {
        variable: String,
        move_point: ProgramPoint,
        use_point: ProgramPoint,
    },
}

/// Detailed error information for diagnostics
#[derive(Clone, Debug)]
pub struct NllDiagnostic {
    /// The primary error
    pub error: NllError,
    /// Additional context about the error
    pub notes: Vec<String>,
    /// Suggested fixes (if any)
    pub suggestions: Vec<String>,
}

impl NllError {
    pub fn to_message(&self) -> String {
        match self {
            NllError::BorrowOutlivesSource {
                at_point,
                missing_points,
                reason,
                ..
            } => {
                let points_str = missing_points
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} at {}: borrow used at points [{}] but source is not live there",
                    reason, at_point, points_str
                )
            }
            NllError::ConflictingBorrows {
                at_point,
                first_reason,
                second_reason,
                ..
            } => {
                format!(
                    "conflicting borrows at {}: {} conflicts with {}",
                    at_point, first_reason, second_reason
                )
            }
            NllError::UseAfterMove {
                variable,
                move_point,
                use_point,
            } => {
                format!(
                    "use of '{}' after move: moved at {}, used at {}",
                    variable, move_point, use_point
                )
            }
        }
    }

    /// Get the primary program point where this error occurred
    pub fn at_point(&self) -> ProgramPoint {
        match self {
            NllError::BorrowOutlivesSource { at_point, .. } => *at_point,
            NllError::ConflictingBorrows { at_point, .. } => *at_point,
            NllError::UseAfterMove { use_point, .. } => *use_point,
        }
    }

    /// Convert to a diagnostic with additional context
    pub fn to_diagnostic(&self) -> NllDiagnostic {
        let mut notes = Vec::new();
        let mut suggestions = Vec::new();

        match self {
            NllError::BorrowOutlivesSource {
                missing_points,
                reason,
                ..
            } => {
                notes.push(format!("the borrow was created for: {}", reason));
                if !missing_points.is_empty() {
                    let first = missing_points[0];
                    notes.push(format!("consider releasing the borrow before {}", first));
                }
                suggestions.push(
                    "add a call to release() before the source goes out of scope".to_string(),
                );
            }
            NllError::ConflictingBorrows {
                first_reason,
                second_reason,
                ..
            } => {
                notes.push(format!("first borrow: {}", first_reason));
                notes.push(format!("second borrow: {}", second_reason));
                suggestions.push("release the first borrow before taking the second".to_string());
            }
            NllError::UseAfterMove {
                variable,
                move_point,
                ..
            } => {
                notes.push(format!("'{}' was moved at {}", variable, move_point));
                suggestions.push("consider using a reference instead of moving".to_string());
                suggestions.push(format!("or clone '{}' before the move", variable));
            }
        }

        NllDiagnostic {
            error: self.clone(),
            notes,
            suggestions,
        }
    }
}

impl NllDiagnostic {
    /// Format the diagnostic for display
    pub fn format(&self) -> String {
        let mut output = self.error.to_message();
        for note in &self.notes {
            output.push_str(&format!("\n  note: {}", note));
        }
        for suggestion in &self.suggestions {
            output.push_str(&format!("\n  help: {}", suggestion));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_program_point_creation() {
        let p = ProgramPoint::new(1, 2);
        assert_eq!(p.block, 1);
        assert_eq!(p.stmt, 2);
        assert_eq!(p.to_string(), "bb1:2");
    }

    #[test]
    fn test_region_operations() {
        let mut r1 = Region::new();
        let mut r2 = Region::new();

        let p1 = ProgramPoint::new(0, 0);
        let p2 = ProgramPoint::new(0, 1);
        let p3 = ProgramPoint::new(0, 2);

        r1.add_point(p1);
        r1.add_point(p2);
        r1.add_point(p3);

        r2.add_point(p1);
        r2.add_point(p2);

        assert!(r1.outlives(&r2)); // r1 contains all of r2's points
        assert!(!r2.outlives(&r1)); // r2 doesn't contain p3
    }

    #[test]
    fn test_hir_cfg_construction() {
        let mut cfg = HirCfg::new();
        assert_eq!(cfg.blocks.len(), 1);
        assert_eq!(cfg.entry, 0);

        // Add some statements
        let p1 = cfg.add_stmt();
        let p2 = cfg.add_stmt();
        assert_eq!(p1, ProgramPoint::new(0, 0));
        assert_eq!(p2, ProgramPoint::new(0, 1));

        // Add a new block
        let b1 = cfg.add_block();
        assert_eq!(b1, 1);
        cfg.add_edge(0, 1);

        assert!(cfg.blocks[0].successors.contains(&1));
        assert!(cfg.blocks[1].predecessors.contains(&0));
    }

    #[test]
    fn test_nll_context_borrow_tracking() {
        let mut ctx = NllContext::new();

        // Create a local
        let _local_region = ctx.create_local_region("x");

        // Advance and create a borrow at point (0, 0)
        ctx.advance(); // now at (0, 0)
        let borrow_region = ctx.create_borrow_region();

        // Record some uses
        ctx.advance(); // now at (0, 1)
        ctx.record_borrow_use(borrow_region);

        ctx.advance(); // now at (0, 2)
        ctx.record_borrow_use(borrow_region);

        // Check last use - should be at stmt 2 (third advance)
        let last_use = ctx.get_borrow_last_use(borrow_region);
        assert!(last_use.is_some());
        assert_eq!(last_use.unwrap(), ProgramPoint::new(0, 2));
    }

    #[test]
    fn test_outlives_constraint_solving() {
        let mut ctx = NllContext::new();

        // Source region covers points 0, 1, 2
        let source = ctx.create_local_region("source");
        ctx.advance();
        if let Some(r) = ctx.regions.get_mut(&source) {
            r.add_point(ProgramPoint::new(0, 0));
            r.add_point(ProgramPoint::new(0, 1));
            r.add_point(ProgramPoint::new(0, 2));
        }

        // Borrow region covers points 0, 1 (subset of source)
        let borrow = ctx.create_borrow_region();
        ctx.record_borrow_use(borrow);
        ctx.advance();
        ctx.record_borrow_use(borrow);

        // Add constraint: source must outlive borrow
        ctx.add_constraint(source, borrow, "test borrow");

        // Should have no errors since source covers all borrow points
        let errors = ctx.solve_constraints();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_outlives_constraint_violation() {
        let mut ctx = NllContext::new();

        // Source region only covers point 0
        let source = ctx.create_local_region("source");
        if let Some(r) = ctx.regions.get_mut(&source) {
            r.add_point(ProgramPoint::new(0, 0));
        }

        // Borrow region covers points 0, 1, 2
        ctx.advance();
        let borrow = ctx.create_borrow_region();
        ctx.advance();
        ctx.record_borrow_use(borrow);
        ctx.advance();
        ctx.record_borrow_use(borrow);

        // Add constraint: source must outlive borrow
        ctx.add_constraint(source, borrow, "test borrow");

        // Should have error since source doesn't cover all borrow points
        let errors = ctx.solve_constraints();
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_nll_error_diagnostic() {
        // Test BorrowOutlivesSource diagnostic
        let error = NllError::BorrowOutlivesSource {
            source_region: RegionId::new(0),
            borrow_region: RegionId::new(1),
            at_point: ProgramPoint::new(0, 1),
            missing_points: vec![ProgramPoint::new(0, 2), ProgramPoint::new(0, 3)],
            reason: "shared borrow of 'x'".to_string(),
        };

        let diagnostic = error.to_diagnostic();
        let formatted = diagnostic.format();
        assert!(formatted.contains("shared borrow of 'x'"));
        assert!(formatted.contains("note:"));
        assert!(formatted.contains("help:"));
    }

    #[test]
    fn test_nll_error_at_point() {
        let error1 = NllError::BorrowOutlivesSource {
            source_region: RegionId::new(0),
            borrow_region: RegionId::new(1),
            at_point: ProgramPoint::new(1, 2),
            missing_points: vec![],
            reason: "test".to_string(),
        };
        assert_eq!(error1.at_point(), ProgramPoint::new(1, 2));

        let error2 = NllError::ConflictingBorrows {
            first_region: RegionId::new(0),
            second_region: RegionId::new(1),
            at_point: ProgramPoint::new(2, 3),
            first_reason: "first".to_string(),
            second_reason: "second".to_string(),
        };
        assert_eq!(error2.at_point(), ProgramPoint::new(2, 3));

        let error3 = NllError::UseAfterMove {
            variable: "x".to_string(),
            move_point: ProgramPoint::new(0, 1),
            use_point: ProgramPoint::new(0, 5),
        };
        assert_eq!(error3.at_point(), ProgramPoint::new(0, 5));
    }

    #[test]
    fn test_use_after_move_diagnostic() {
        let error = NllError::UseAfterMove {
            variable: "data".to_string(),
            move_point: ProgramPoint::new(0, 2),
            use_point: ProgramPoint::new(0, 5),
        };

        let message = error.to_message();
        assert!(message.contains("data"));
        assert!(message.contains("bb0:2"));
        assert!(message.contains("bb0:5"));

        let diagnostic = error.to_diagnostic();
        assert!(!diagnostic.notes.is_empty());
        assert!(!diagnostic.suggestions.is_empty());
    }
}
