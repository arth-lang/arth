//! JIT Compiler for Arth VM using Cranelift
//!
//! This module implements tiered compilation for the Arth VM:
//! - Tier 0: Interpreter (cold start, full feature support)
//! - Tier 1: Baseline JIT (hot functions, fast compilation, minimal optimization)
//! - Tier 2: Optimized JIT (very hot functions, slower compilation, full optimization)
//!
//! The JIT compiles sequences of VM bytecode operations to native machine code
//! using Cranelift. Functions are compiled on-demand when they become "hot"
//! (exceeded a call count threshold).
//!
//! # Tiered Compilation Strategy
//!
//! - **Tier 0 → Tier 1**: After `TIER1_THRESHOLD` (100) calls, compile with baseline JIT
//! - **Tier 1 → Tier 2**: After `TIER2_THRESHOLD` (1000) additional calls, recompile with optimizations
//!
//! This provides fast startup (interpreter), quick warmup (baseline JIT), and
//! peak performance for truly hot code (optimized JIT).
//!
//! # Architecture
//!
//! - `JitContext`: Global JIT state, code cache, and Cranelift module
//! - `FunctionMeta`: Per-function metadata (call count, tier, compiled code pointers)
//! - `translate_function`: Bytecode → Cranelift IR → native code
//!
//! # Hot Path Detection
//!
//! Functions track their call count. When a function exceeds tier thresholds,
//! it is compiled (or recompiled) to the next tier.

use std::collections::HashMap;
use std::sync::Mutex;

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

use crate::ops::Op;
use crate::program::Program;

/// Threshold for Tier 1 compilation (interpreter → baseline JIT).
/// Functions called more than this many times are compiled with baseline optimization.
pub const TIER1_THRESHOLD: u32 = 100;

/// Threshold for Tier 2 compilation (baseline JIT → optimized JIT).
/// Functions called this many additional times after Tier 1 are recompiled with full optimization.
pub const TIER2_THRESHOLD: u32 = 1000;

/// Legacy alias for backwards compatibility
pub const JIT_THRESHOLD: u32 = TIER1_THRESHOLD;

/// Threshold for OSR (On-Stack Replacement) compilation.
/// When a loop iterates this many times, compile and switch to JIT mid-execution.
pub const OSR_THRESHOLD: u32 = 1000;

/// Compilation tier for a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilationTier {
    /// Tier 0: Interpreted bytecode (no JIT compilation)
    Interpreter,
    /// Tier 1: Baseline JIT (fast compile, minimal optimization)
    BaselineJit,
    /// Tier 2: Optimized JIT (slower compile, full optimization)
    OptimizedJit,
}

/// Reasons for deoptimization (transitioning from JIT back to interpreter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeoptReason {
    /// No deoptimization occurred
    None,
    /// Type assumption was violated (e.g., expected int, got float)
    TypeMismatch,
    /// Overflow occurred during arithmetic
    Overflow,
    /// Division by zero
    DivisionByZero,
    /// Array/list index out of bounds
    IndexOutOfBounds,
    /// Null pointer dereference
    NullPointer,
    /// Stack overflow
    StackOverflow,
    /// Unsupported operation in JIT code
    UnsupportedOperation,
    /// Debug breakpoint hit
    DebugBreakpoint,
    /// Explicit bailout requested
    ExplicitBailout,
    /// Hot loop detected (OSR entry failed)
    OsrFailure,
    /// Too many deoptimizations, giving up on this function
    TooManyDeopts,
    /// Unknown/other reason
    Unknown,
}

impl std::fmt::Display for DeoptReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeoptReason::None => write!(f, "none"),
            DeoptReason::TypeMismatch => write!(f, "type mismatch"),
            DeoptReason::Overflow => write!(f, "overflow"),
            DeoptReason::DivisionByZero => write!(f, "division by zero"),
            DeoptReason::IndexOutOfBounds => write!(f, "index out of bounds"),
            DeoptReason::NullPointer => write!(f, "null pointer"),
            DeoptReason::StackOverflow => write!(f, "stack overflow"),
            DeoptReason::UnsupportedOperation => write!(f, "unsupported operation"),
            DeoptReason::DebugBreakpoint => write!(f, "debug breakpoint"),
            DeoptReason::ExplicitBailout => write!(f, "explicit bailout"),
            DeoptReason::OsrFailure => write!(f, "OSR failure"),
            DeoptReason::TooManyDeopts => write!(f, "too many deoptimizations"),
            DeoptReason::Unknown => write!(f, "unknown"),
        }
    }
}

/// Information about a deoptimization event.
#[derive(Debug, Clone)]
pub struct DeoptInfo {
    /// The reason for deoptimization
    pub reason: DeoptReason,
    /// The bytecode offset where deoptimization occurred
    pub bytecode_offset: u32,
    /// The instruction pointer in native code (if available)
    pub native_ip: Option<usize>,
    /// Number of locals at deopt point
    pub local_count: u32,
    /// Values of locals at deopt point (for state transfer)
    pub locals: Vec<i64>,
    /// Stack values at deopt point
    pub stack: Vec<i64>,
    /// Timestamp of deoptimization
    pub timestamp_us: u64,
}

impl DeoptInfo {
    pub fn new(reason: DeoptReason, bytecode_offset: u32) -> Self {
        Self {
            reason,
            bytecode_offset,
            native_ip: None,
            local_count: 0,
            locals: Vec::new(),
            stack: Vec::new(),
            timestamp_us: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0),
        }
    }

    /// Create deopt info with full state capture.
    pub fn with_state(
        reason: DeoptReason,
        bytecode_offset: u32,
        locals: Vec<i64>,
        stack: Vec<i64>,
    ) -> Self {
        Self {
            reason,
            bytecode_offset,
            native_ip: None,
            local_count: locals.len() as u32,
            locals,
            stack,
            timestamp_us: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0),
        }
    }
}

/// Maximum number of deoptimizations before giving up on a function.
pub const MAX_DEOPTS_PER_FUNCTION: u32 = 10;

/// JIT compilation error types.
#[derive(Debug)]
pub enum JitError {
    /// Cranelift codegen error
    Codegen(String),
    /// Function not found in bytecode
    FunctionNotFound(u32),
    /// Unsupported opcode for JIT
    UnsupportedOpcode(String),
    /// Module error
    Module(String),
}

impl std::fmt::Display for JitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JitError::Codegen(msg) => write!(f, "JIT codegen error: {}", msg),
            JitError::FunctionNotFound(offset) => {
                write!(f, "Function not found at offset {}", offset)
            }
            JitError::UnsupportedOpcode(op) => write!(f, "Unsupported opcode for JIT: {}", op),
            JitError::Module(msg) => write!(f, "JIT module error: {}", msg),
        }
    }
}

/// Metadata for a single function in the bytecode.
#[derive(Debug)]
pub struct FunctionMeta {
    /// Bytecode offset where the function starts
    pub offset: u32,
    /// Number of times this function has been called
    pub call_count: u32,
    /// Current compilation tier
    pub tier: CompilationTier,
    /// Pointer to current native code (highest tier compiled)
    pub native_ptr: Option<*const u8>,
    /// Pointer to Tier 1 native code (kept for deoptimization)
    pub tier1_ptr: Option<*const u8>,
    /// Pointer to Tier 2 native code
    pub tier2_ptr: Option<*const u8>,
    /// Number of parameters this function takes
    pub param_count: u32,
    /// Whether this function is currently being compiled
    pub compiling: bool,
    /// Call count at which Tier 1 was reached
    pub tier1_at_count: u32,
    /// Number of deoptimizations that have occurred
    pub deopt_count: u32,
    /// Most recent deoptimization reason
    pub last_deopt_reason: DeoptReason,
    /// Whether JIT is permanently disabled for this function (too many deopts)
    pub jit_disabled: bool,
}

// Safety: native_ptr is only accessed from single-threaded VM execution
// and the pointer itself is stable once set (code is never deallocated)
unsafe impl Send for FunctionMeta {}
unsafe impl Sync for FunctionMeta {}

impl FunctionMeta {
    pub fn new(offset: u32, param_count: u32) -> Self {
        Self {
            offset,
            call_count: 0,
            tier: CompilationTier::Interpreter,
            native_ptr: None,
            tier1_ptr: None,
            tier2_ptr: None,
            param_count,
            compiling: false,
            tier1_at_count: 0,
            deopt_count: 0,
            last_deopt_reason: DeoptReason::None,
            jit_disabled: false,
        }
    }

    /// Increment call count and check for tier promotion.
    /// Returns the tier that should be compiled (if any).
    pub fn increment_call_count(&mut self) -> Option<CompilationTier> {
        self.call_count = self.call_count.saturating_add(1);

        // Never promote if JIT is disabled or currently compiling
        if self.jit_disabled || self.compiling {
            return None;
        }

        match self.tier {
            CompilationTier::Interpreter => {
                // Check for Tier 1 promotion
                if self.call_count >= TIER1_THRESHOLD {
                    Some(CompilationTier::BaselineJit)
                } else {
                    None
                }
            }
            CompilationTier::BaselineJit => {
                // Check for Tier 2 promotion (after additional calls)
                let calls_since_tier1 = self.call_count.saturating_sub(self.tier1_at_count);
                if calls_since_tier1 >= TIER2_THRESHOLD {
                    Some(CompilationTier::OptimizedJit)
                } else {
                    None
                }
            }
            CompilationTier::OptimizedJit => {
                // Already at highest tier
                None
            }
        }
    }

    /// Check if this function should be compiled to a higher tier.
    /// Legacy compatibility method - returns true if should compile to Tier 1.
    pub fn should_compile(&self) -> bool {
        !self.compiling
            && !self.jit_disabled
            && self.tier == CompilationTier::Interpreter
            && self.call_count >= TIER1_THRESHOLD
    }

    /// Record a deoptimization event.
    /// Returns true if JIT was disabled due to too many deopts.
    pub fn record_deopt(&mut self, reason: DeoptReason) -> bool {
        self.deopt_count += 1;
        self.last_deopt_reason = reason;

        if self.deopt_count >= MAX_DEOPTS_PER_FUNCTION {
            self.jit_disabled = true;
            // Clear native pointers to force interpreter path
            self.native_ptr = None;
            self.tier = CompilationTier::Interpreter;
            true
        } else {
            false
        }
    }

    /// Deoptimize from current tier to a lower tier.
    /// Returns the tier we deoptimized to.
    pub fn deoptimize(&mut self, reason: DeoptReason) -> CompilationTier {
        let disabled = self.record_deopt(reason);

        if disabled {
            return CompilationTier::Interpreter;
        }

        match self.tier {
            CompilationTier::OptimizedJit => {
                // Fall back to Tier 1 (baseline) if available
                if self.tier1_ptr.is_some() {
                    self.tier = CompilationTier::BaselineJit;
                    self.native_ptr = self.tier1_ptr;
                    self.tier2_ptr = None; // Invalidate Tier 2
                    CompilationTier::BaselineJit
                } else {
                    // Fall back to interpreter
                    self.tier = CompilationTier::Interpreter;
                    self.native_ptr = None;
                    CompilationTier::Interpreter
                }
            }
            CompilationTier::BaselineJit => {
                // Fall back to interpreter
                self.tier = CompilationTier::Interpreter;
                self.native_ptr = None;
                self.tier1_ptr = None; // Invalidate Tier 1
                CompilationTier::Interpreter
            }
            CompilationTier::Interpreter => {
                // Already at interpreter, nothing to do
                CompilationTier::Interpreter
            }
        }
    }

    /// Check if JIT should be used for this function.
    pub fn should_use_jit(&self) -> bool {
        !self.jit_disabled && self.native_ptr.is_some()
    }
}

/// Metadata for a loop detected via back edge.
/// Used for On-Stack Replacement (OSR) to JIT-compile hot loops.
#[derive(Debug)]
pub struct OsrLoopMeta {
    /// Instruction pointer of the back edge (backward Jump instruction)
    pub back_edge_ip: u32,
    /// Loop header (target of backward Jump - where the loop starts)
    pub header_ip: u32,
    /// Number of times this loop has iterated
    pub iteration_count: u32,
    /// Number of locals in scope when entering the loop
    pub local_count: u32,
    /// Native code pointer for OSR entry (if compiled)
    pub osr_entry: Option<*const u8>,
    /// Whether this loop is currently being compiled
    pub compiling: bool,
    /// Whether compilation failed (don't retry)
    pub compilation_failed: bool,
}

// Safety: osr_entry is only accessed from single-threaded VM execution
// and the pointer itself is stable once set
unsafe impl Send for OsrLoopMeta {}
unsafe impl Sync for OsrLoopMeta {}

impl OsrLoopMeta {
    pub fn new(back_edge_ip: u32, header_ip: u32, local_count: u32) -> Self {
        Self {
            back_edge_ip,
            header_ip,
            iteration_count: 0,
            local_count,
            osr_entry: None,
            compiling: false,
            compilation_failed: false,
        }
    }

    /// Increment iteration count and check if OSR should be triggered.
    /// Returns true if the loop is hot enough for OSR compilation.
    pub fn increment_iteration(&mut self) -> bool {
        self.iteration_count = self.iteration_count.saturating_add(1);
        !self.compiling
            && !self.compilation_failed
            && self.osr_entry.is_none()
            && self.iteration_count >= OSR_THRESHOLD
    }

    /// Check if OSR entry is available.
    pub fn has_osr_entry(&self) -> bool {
        self.osr_entry.is_some()
    }
}

/// JIT compilation context.
///
/// Holds the Cranelift JIT module, code cache, and function metadata.
pub struct JitContext {
    /// Cranelift JIT module for code generation
    module: JITModule,
    /// Function builder context (reused across compilations)
    builder_ctx: FunctionBuilderContext,
    /// Cranelift codegen context
    ctx: codegen::Context,
    /// Function metadata indexed by bytecode offset
    functions: HashMap<u32, FunctionMeta>,
    /// Loop metadata indexed by back edge IP (for OSR)
    loops: HashMap<u32, OsrLoopMeta>,
    /// Compilation statistics
    pub stats: JitStats,
    /// Pointer type for the target architecture
    ptr_ty: Type,
}

/// JIT compilation statistics.
#[derive(Debug, Default, Clone)]
pub struct JitStats {
    /// Number of functions compiled (total across all tiers)
    pub functions_compiled: u32,
    /// Number of Tier 1 (baseline) compilations
    pub tier1_compilations: u32,
    /// Number of Tier 2 (optimized) compilations
    pub tier2_compilations: u32,
    /// Total time spent compiling (microseconds)
    pub compile_time_us: u64,
    /// Time spent on Tier 1 compilations (microseconds)
    pub tier1_compile_time_us: u64,
    /// Time spent on Tier 2 compilations (microseconds)
    pub tier2_compile_time_us: u64,
    /// Number of JIT cache hits
    pub cache_hits: u64,
    /// Number of Tier 2 cache hits (optimized code executed)
    pub tier2_cache_hits: u64,
    /// Number of interpreter fallbacks
    pub interpreter_fallbacks: u64,
    /// Number of OSR compilations
    pub osr_compilations: u32,
    /// Number of successful OSR entries (switched to JIT mid-loop)
    pub osr_entries: u64,
    /// Time spent on OSR compilations (microseconds)
    pub osr_compile_time_us: u64,
    /// Total number of deoptimizations
    pub deopt_count: u64,
    /// Number of deoptimizations by reason
    pub deopts_by_reason: [u32; 13], // One slot per DeoptReason variant
    /// Number of functions permanently disabled due to too many deopts
    pub functions_jit_disabled: u32,
}

impl JitContext {
    /// Create a new JIT context.
    pub fn new() -> Result<Self, JitError> {
        // Get the native target ISA
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();

        let isa_builder =
            cranelift_native::builder().map_err(|e| JitError::Codegen(e.to_string()))?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| JitError::Codegen(e.to_string()))?;

        let ptr_ty = isa.pointer_type();

        // Create the JIT module
        let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register runtime symbols for host calls
        Self::register_runtime_symbols(&mut jit_builder);

        let module = JITModule::new(jit_builder);

        Ok(Self {
            module,
            builder_ctx: FunctionBuilderContext::new(),
            ctx: codegen::Context::new(),
            functions: HashMap::new(),
            loops: HashMap::new(),
            stats: JitStats::default(),
            ptr_ty,
        })
    }

    /// Register runtime helper symbols that JIT code can call.
    fn register_runtime_symbols(builder: &mut JITBuilder) {
        // Register print functions
        builder.symbol("arth_rt_print_i64", arth_jit_print_i64 as *const u8);
        builder.symbol("arth_rt_print_str", arth_jit_print_str as *const u8);

        // Register list operations
        builder.symbol("arth_rt_list_new", arth_jit_list_new as *const u8);
        builder.symbol("arth_rt_list_push", arth_jit_list_push as *const u8);
        builder.symbol("arth_rt_list_get", arth_jit_list_get as *const u8);
        builder.symbol("arth_rt_list_len", arth_jit_list_len as *const u8);

        // Register map operations
        builder.symbol("arth_rt_map_new", arth_jit_map_new as *const u8);
        builder.symbol("arth_rt_map_put", arth_jit_map_put as *const u8);
        builder.symbol("arth_rt_map_get", arth_jit_map_get as *const u8);
    }

    /// Check if a function is already registered for JIT tracking.
    pub fn is_function_registered(&self, offset: u32) -> bool {
        self.functions.contains_key(&offset)
    }

    /// Register a function at the given bytecode offset.
    pub fn register_function(&mut self, offset: u32, param_count: u32) {
        self.functions
            .entry(offset)
            .or_insert_with(|| FunctionMeta::new(offset, param_count));
    }

    /// Get function metadata, incrementing call count.
    /// Returns (native_ptr, tier_to_compile) where tier_to_compile indicates if
    /// the function should be compiled (or recompiled) to a higher tier.
    pub fn get_function(&mut self, offset: u32) -> (Option<*const u8>, Option<CompilationTier>) {
        if let Some(meta) = self.functions.get_mut(&offset) {
            let tier_to_compile = meta.increment_call_count();
            (meta.native_ptr, tier_to_compile)
        } else {
            (None, None)
        }
    }

    /// Check if a function has JIT-compiled code available.
    pub fn has_native_code(&self, offset: u32) -> bool {
        self.functions
            .get(&offset)
            .map(|m| m.native_ptr.is_some())
            .unwrap_or(false)
    }

    /// Get the native code pointer for a function.
    pub fn get_native_ptr(&self, offset: u32) -> Option<*const u8> {
        self.functions.get(&offset).and_then(|m| m.native_ptr)
    }

    /// Get the parameter count for a function.
    pub fn get_param_count(&self, offset: u32) -> Option<u32> {
        self.functions.get(&offset).map(|m| m.param_count)
    }

    /// Increment the cache hits counter.
    pub fn increment_cache_hits(&mut self) {
        self.stats.cache_hits += 1;
    }

    // ========================================================================
    // OSR (On-Stack Replacement) Methods
    // ========================================================================

    /// Register a loop back edge for OSR tracking.
    ///
    /// # Arguments
    /// * `back_edge_ip` - The IP of the backward Jump instruction
    /// * `header_ip` - The target of the jump (loop header)
    /// * `local_count` - Number of locals in scope
    pub fn register_loop(&mut self, back_edge_ip: u32, header_ip: u32, local_count: u32) {
        self.loops
            .entry(back_edge_ip)
            .or_insert_with(|| OsrLoopMeta::new(back_edge_ip, header_ip, local_count));
    }

    /// Check if a loop is registered at the given back edge IP.
    pub fn is_loop_registered(&self, back_edge_ip: u32) -> bool {
        self.loops.contains_key(&back_edge_ip)
    }

    /// Increment loop iteration and check if OSR should trigger.
    /// Returns (should_compile, has_osr_entry).
    pub fn increment_loop_iteration(&mut self, back_edge_ip: u32) -> (bool, bool) {
        if let Some(meta) = self.loops.get_mut(&back_edge_ip) {
            let should_compile = meta.increment_iteration();
            let has_entry = meta.has_osr_entry();
            (should_compile, has_entry)
        } else {
            (false, false)
        }
    }

    /// Get the OSR entry pointer for a loop (if compiled).
    pub fn get_osr_entry(&self, back_edge_ip: u32) -> Option<*const u8> {
        self.loops.get(&back_edge_ip).and_then(|m| m.osr_entry)
    }

    /// Get loop metadata for a back edge.
    pub fn get_loop_meta(&self, back_edge_ip: u32) -> Option<&OsrLoopMeta> {
        self.loops.get(&back_edge_ip)
    }

    /// Increment OSR entries counter.
    pub fn increment_osr_entries(&mut self) {
        self.stats.osr_entries += 1;
    }

    /// Compile an OSR entry point for a hot loop.
    ///
    /// The OSR-compiled code:
    /// - Takes current locals as parameters
    /// - Starts execution from the loop header
    /// - Returns when the loop exits (hitting a forward Jump or Ret)
    /// - Returns a tuple: (loop_exited, exit_ip, ...updated_locals)
    ///
    /// # Arguments
    /// * `program` - The program bytecode
    /// * `back_edge_ip` - The back edge IP identifying the loop
    ///
    /// # Returns
    /// The OSR entry code pointer on success.
    pub fn compile_osr(
        &mut self,
        program: &Program,
        back_edge_ip: u32,
    ) -> Result<*const u8, JitError> {
        let start_time = std::time::Instant::now();

        // Get loop metadata
        let (header_ip, local_count) = {
            let meta = self
                .loops
                .get_mut(&back_edge_ip)
                .ok_or(JitError::Codegen("Loop not registered".into()))?;

            if meta.osr_entry.is_some() {
                return Ok(meta.osr_entry.unwrap());
            }
            if meta.compilation_failed {
                return Err(JitError::Codegen("Previous OSR compilation failed".into()));
            }
            meta.compiling = true;
            (meta.header_ip, meta.local_count)
        };

        // Extract loop body from header to back edge (inclusive)
        let ops = Self::extract_loop_ops(program, header_ip, back_edge_ip)?;

        // Build OSR function signature: (locals...) -> i64
        // Returns: packed result with exit info and updated locals
        self.ctx.func.signature.params.clear();
        self.ctx.func.signature.returns.clear();

        let ptr_ty = self.ptr_ty;
        for _ in 0..local_count {
            self.ctx.func.signature.params.push(AbiParam::new(ptr_ty));
        }
        // Return: single i64 (for simple loops, this is the result or continuation flag)
        self.ctx.func.signature.returns.push(AbiParam::new(ptr_ty));

        // Build the OSR function
        let compile_result = {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);
            let result =
                Self::translate_osr_loop(&mut builder, &ops, local_count, back_edge_ip, ptr_ty);
            if result.is_ok() {
                builder.finalize();
            }
            result
        };

        if let Err(e) = compile_result {
            // Mark compilation as failed
            if let Some(meta) = self.loops.get_mut(&back_edge_ip) {
                meta.compiling = false;
                meta.compilation_failed = true;
            }
            self.ctx.clear();
            return Err(e);
        }

        // Declare and define the OSR function
        let func_name = format!("arth_osr_{}", back_edge_ip);
        let func_id = self
            .module
            .declare_function(&func_name, Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| JitError::Module(e.to_string()))?;

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| JitError::Codegen(e.to_string()))?;

        self.module.clear_context(&mut self.ctx);

        // Finalize and get the code pointer
        self.module
            .finalize_definitions()
            .map_err(|e| JitError::Module(format!("{:?}", e)))?;

        let code_ptr = self.module.get_finalized_function(func_id);

        // Update loop metadata
        if let Some(meta) = self.loops.get_mut(&back_edge_ip) {
            meta.osr_entry = Some(code_ptr);
            meta.compiling = false;
        }

        // Update stats
        let compile_time = start_time.elapsed().as_micros() as u64;
        self.stats.osr_compilations += 1;
        self.stats.osr_compile_time_us += compile_time;
        self.stats.compile_time_us += compile_time;

        Ok(code_ptr)
    }

    /// Extract the loop body operations from header to back edge.
    fn extract_loop_ops(
        program: &Program,
        header_ip: u32,
        back_edge_ip: u32,
    ) -> Result<Vec<Op>, JitError> {
        let mut ops = Vec::new();
        let mut ip = header_ip as usize;

        // Extract ops from header through back edge
        while ip <= back_edge_ip as usize && ip < program.code.len() {
            ops.push(program.code[ip].clone());
            ip += 1;
        }

        if ops.is_empty() {
            return Err(JitError::Codegen("Empty loop body".into()));
        }

        Ok(ops)
    }

    /// Translate loop body to Cranelift IR for OSR.
    ///
    /// The OSR function structure:
    /// - Entry block: receives locals as params, jumps to loop header
    /// - Loop header: main loop body
    /// - Back edge: conditional/unconditional jump back to header
    /// - Exit: return from function
    fn translate_osr_loop(
        builder: &mut FunctionBuilder,
        ops: &[Op],
        local_count: u32,
        back_edge_ip: u32,
        ptr_ty: Type,
    ) -> Result<(), JitError> {
        let entry_block = builder.create_block();
        let loop_block = builder.create_block();
        let exit_block = builder.create_block();

        // Entry block: receive locals as parameters
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);

        // Initialize locals from parameters
        let mut locals: HashMap<u32, Variable> = HashMap::new();
        for i in 0..local_count {
            let var = builder.declare_var(ptr_ty);
            let param = builder.block_params(entry_block)[i as usize];
            builder.def_var(var, param);
            locals.insert(i, var);
        }

        // Jump to loop body
        builder.ins().jump(loop_block, &[]);
        builder.seal_block(entry_block);

        // Loop block
        builder.switch_to_block(loop_block);
        let mut stack: Vec<Value> = Vec::new();

        // Translate loop body ops
        for (idx, op) in ops.iter().enumerate() {
            let is_last = idx == ops.len() - 1;

            match op {
                Op::PushI64(val) => {
                    let v = builder.ins().iconst(ptr_ty, *val);
                    stack.push(v);
                }

                Op::PushBool(val) => {
                    let v = builder.ins().iconst(ptr_ty, *val as i64);
                    stack.push(v);
                }

                Op::AddI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().iadd(a, b);
                    stack.push(r);
                }

                Op::SubI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().isub(a, b);
                    stack.push(r);
                }

                Op::MulI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().imul(a, b);
                    stack.push(r);
                }

                Op::DivI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().sdiv(a, b);
                    stack.push(r);
                }

                Op::ModI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().srem(a, b);
                    stack.push(r);
                }

                Op::LtI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let cmp = builder.ins().icmp(IntCC::SignedLessThan, a, b);
                    let r = builder.ins().uextend(ptr_ty, cmp);
                    stack.push(r);
                }

                Op::EqI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let cmp = builder.ins().icmp(IntCC::Equal, a, b);
                    let r = builder.ins().uextend(ptr_ty, cmp);
                    stack.push(r);
                }

                Op::LocalGet(idx) => {
                    let var = locals
                        .get(idx)
                        .ok_or(JitError::Codegen(format!("Undefined local {}", idx)))?;
                    let v = builder.use_var(*var);
                    stack.push(v);
                }

                Op::LocalSet(idx) => {
                    let v = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let var = *locals
                        .entry(*idx)
                        .or_insert_with(|| builder.declare_var(ptr_ty));
                    builder.def_var(var, v);
                }

                Op::Pop => {
                    stack.pop();
                }

                Op::Jump(tgt) => {
                    // This is the back edge - loop back
                    if *tgt == back_edge_ip - (ops.len() as u32 - 1) {
                        // Back edge: continue loop
                        builder.ins().jump(loop_block, &[]);
                    } else {
                        // Forward jump: exit loop
                        builder.ins().jump(exit_block, &[]);
                    }
                }

                Op::JumpIfFalse(tgt) => {
                    let cond = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, cond, 0);

                    if is_last {
                        // This is at the back edge position - if false, exit; if true, continue
                        builder
                            .ins()
                            .brif(cond_bool, loop_block, &[], exit_block, &[]);
                    } else {
                        // Mid-loop conditional - need more complex handling
                        // For simplicity, treat forward jumps as exits
                        let header_offset = back_edge_ip - (ops.len() as u32 - 1);
                        if *tgt <= header_offset {
                            // Backward: continue loop
                            let continue_block = builder.create_block();
                            builder
                                .ins()
                                .brif(cond_bool, continue_block, &[], loop_block, &[]);
                            builder.seal_block(continue_block);
                            builder.switch_to_block(continue_block);
                        } else {
                            // Forward: might exit
                            let continue_block = builder.create_block();
                            builder
                                .ins()
                                .brif(cond_bool, continue_block, &[], exit_block, &[]);
                            builder.seal_block(continue_block);
                            builder.switch_to_block(continue_block);
                        }
                    }
                }

                _ => {
                    // Unsupported opcode - mark for interpreter fallback
                    return Err(JitError::UnsupportedOpcode(format!("{:?}", op)));
                }
            }
        }

        // Seal the loop block (it has back edges)
        builder.seal_block(loop_block);

        // Exit block: return result (top of stack or 0)
        builder.switch_to_block(exit_block);
        let ret_val = stack
            .pop()
            .unwrap_or_else(|| builder.ins().iconst(ptr_ty, 0));
        builder.ins().return_(&[ret_val]);
        builder.seal_block(exit_block);

        // Note: finalize() is called by the caller after we return
        Ok(())
    }

    /// Call an OSR-compiled loop with the current locals.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - The OSR entry has been compiled
    /// - The locals array matches the expected local_count
    pub unsafe fn call_osr(&self, back_edge_ip: u32, locals: &[i64]) -> Result<i64, JitError> {
        let meta = self
            .loops
            .get(&back_edge_ip)
            .ok_or(JitError::Codegen("Loop not found".into()))?;

        let ptr = meta
            .osr_entry
            .ok_or(JitError::Codegen("OSR entry not compiled".into()))?;

        if locals.len() != meta.local_count as usize {
            return Err(JitError::Codegen(format!(
                "Expected {} locals, got {}",
                meta.local_count,
                locals.len()
            )));
        }

        // Call based on local count (similar to call_native)
        // Each transmute is wrapped in its own unsafe block for Rust 2024 compliance
        let result = match locals.len() {
            0 => {
                let func: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
                func()
            }
            1 => {
                let func: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(locals[0])
            }
            2 => {
                let func: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(locals[0], locals[1])
            }
            3 => {
                let func: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(locals[0], locals[1], locals[2])
            }
            4 => {
                let func: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(locals[0], locals[1], locals[2], locals[3])
            }
            5 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(locals[0], locals[1], locals[2], locals[3], locals[4])
            }
            6 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(
                    locals[0], locals[1], locals[2], locals[3], locals[4], locals[5],
                )
            }
            7 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(
                    locals[0], locals[1], locals[2], locals[3], locals[4], locals[5], locals[6],
                )
            }
            8 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(
                    locals[0], locals[1], locals[2], locals[3], locals[4], locals[5], locals[6],
                    locals[7],
                )
            }
            _ => {
                return Err(JitError::Codegen(format!(
                    "Too many locals for OSR: {} (max 8)",
                    locals.len()
                )));
            }
        };

        Ok(result)
    }

    /// Call a JIT-compiled function with the given arguments.
    /// Returns the function result as i64.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - The function at `offset` has been compiled (native_ptr is Some)
    /// - The number of arguments matches the function's param_count
    /// - The arguments are valid i64 values
    pub unsafe fn call_native(&self, offset: u32, args: &[i64]) -> Result<i64, JitError> {
        let meta = self
            .functions
            .get(&offset)
            .ok_or(JitError::FunctionNotFound(offset))?;

        let ptr = meta
            .native_ptr
            .ok_or(JitError::Codegen("Function not yet compiled".into()))?;

        if args.len() != meta.param_count as usize {
            return Err(JitError::Codegen(format!(
                "Expected {} args, got {}",
                meta.param_count,
                args.len()
            )));
        }

        // Call the function based on arity
        // All transmutes are inside this unsafe function, wrapping each in its own block
        let result = match args.len() {
            0 => {
                let func: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
                func()
            }
            1 => {
                let func: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(args[0])
            }
            2 => {
                let func: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(args[0], args[1])
            }
            3 => {
                let func: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
                func(args[0], args[1], args[2])
            }
            4 => {
                let func: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(args[0], args[1], args[2], args[3])
            }
            5 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(args[0], args[1], args[2], args[3], args[4])
            }
            6 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(args[0], args[1], args[2], args[3], args[4], args[5])
            }
            7 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(
                    args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                )
            }
            8 => {
                let func: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(ptr) };
                func(
                    args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
                )
            }
            _ => {
                return Err(JitError::Codegen(format!(
                    "Too many arguments: {} (max 8)",
                    args.len()
                )));
            }
        };

        Ok(result)
    }

    /// Try to execute a function via JIT if compiled, otherwise return None.
    /// This is the main entry point for the interpreter integration.
    ///
    /// Returns:
    /// - Some(Ok(result)) if function was executed via JIT
    /// - Some(Err(e)) if JIT execution failed
    /// - None if function is not compiled (fall back to interpreter)
    pub fn try_call_jit(&mut self, offset: u32, args: &[i64]) -> Option<Result<i64, JitError>> {
        let meta = self.functions.get_mut(&offset)?;

        // Check if JIT is disabled for this function
        if meta.jit_disabled {
            self.stats.interpreter_fallbacks += 1;
            return None;
        }

        // Increment call count and check for tier promotion
        let tier_to_compile = meta.increment_call_count();

        // Check if we have native code
        if let Some(_ptr) = meta.native_ptr {
            // Use the native code
            self.stats.cache_hits += 1;
            // Safety: we've verified the function is compiled
            Some(unsafe { self.call_native(offset, args) })
        } else if tier_to_compile.is_some() {
            // Mark that we should compile, but don't block - return None to use interpreter
            // Compilation will happen on the next call after we've accumulated enough calls
            self.stats.interpreter_fallbacks += 1;
            None
        } else {
            self.stats.interpreter_fallbacks += 1;
            None
        }
    }

    // ========================================================================
    // Deoptimization Methods
    // ========================================================================

    /// Record a deoptimization event for a function.
    ///
    /// This updates both function metadata and global stats.
    /// Returns the tier that was deoptimized to.
    ///
    /// # Arguments
    /// * `func_offset` - The bytecode offset of the function
    /// * `reason` - The reason for deoptimization
    /// * `info` - Optional detailed deopt info (for debugging/profiling)
    pub fn record_deopt(
        &mut self,
        func_offset: u32,
        reason: DeoptReason,
        _info: Option<DeoptInfo>,
    ) -> Option<CompilationTier> {
        let meta = self.functions.get_mut(&func_offset)?;

        // Update stats
        self.stats.deopt_count += 1;
        let reason_idx = reason as usize;
        if reason_idx < self.stats.deopts_by_reason.len() {
            self.stats.deopts_by_reason[reason_idx] += 1;
        }

        // Deoptimize the function
        let new_tier = meta.deoptimize(reason);

        // Check if JIT was disabled
        if meta.jit_disabled {
            self.stats.functions_jit_disabled += 1;
        }

        Some(new_tier)
    }

    /// Force deoptimization to interpreter for a function.
    ///
    /// This is a more aggressive deopt that always goes to interpreter,
    /// regardless of whether Tier 1 code is available.
    pub fn force_deopt_to_interpreter(&mut self, func_offset: u32, reason: DeoptReason) -> bool {
        let meta = match self.functions.get_mut(&func_offset) {
            Some(m) => m,
            None => return false,
        };

        // Update stats
        self.stats.deopt_count += 1;
        let reason_idx = reason as usize;
        if reason_idx < self.stats.deopts_by_reason.len() {
            self.stats.deopts_by_reason[reason_idx] += 1;
        }

        // Record deopt
        let disabled = meta.record_deopt(reason);
        if disabled {
            self.stats.functions_jit_disabled += 1;
        }

        // Force to interpreter
        meta.tier = CompilationTier::Interpreter;
        meta.native_ptr = None;

        true
    }

    /// Check if a function has JIT disabled due to too many deopts.
    pub fn is_jit_disabled(&self, func_offset: u32) -> bool {
        self.functions
            .get(&func_offset)
            .map(|m| m.jit_disabled)
            .unwrap_or(false)
    }

    /// Get deoptimization statistics for a function.
    pub fn get_function_deopt_stats(&self, func_offset: u32) -> Option<(u32, DeoptReason, bool)> {
        self.functions
            .get(&func_offset)
            .map(|m| (m.deopt_count, m.last_deopt_reason, m.jit_disabled))
    }

    /// Compile a function from bytecode to native code.
    ///
    /// This is the main entry point for JIT compilation.
    /// Returns the native code pointer on success.
    ///
    /// The `tier` parameter controls the optimization level:
    /// - `BaselineJit` (Tier 1): Fast compilation, minimal optimization
    /// - `OptimizedJit` (Tier 2): Slower compilation, full optimization
    pub fn compile_function(
        &mut self,
        program: &Program,
        func_offset: u32,
    ) -> Result<*const u8, JitError> {
        // Default to Tier 1 for backwards compatibility
        self.compile_function_at_tier(program, func_offset, CompilationTier::BaselineJit)
    }

    /// Compile a function at a specific tier.
    pub fn compile_function_at_tier(
        &mut self,
        program: &Program,
        func_offset: u32,
        tier: CompilationTier,
    ) -> Result<*const u8, JitError> {
        let start_time = std::time::Instant::now();

        // Mark function as being compiled
        if let Some(meta) = self.functions.get_mut(&func_offset) {
            // For Tier 1, skip if already compiled at any tier
            if tier == CompilationTier::BaselineJit && meta.tier1_ptr.is_some() {
                return Ok(meta.tier1_ptr.unwrap());
            }
            // For Tier 2, skip if already at Tier 2
            if tier == CompilationTier::OptimizedJit && meta.tier2_ptr.is_some() {
                return Ok(meta.tier2_ptr.unwrap());
            }
            meta.compiling = true;
        }

        // Analyze function boundaries
        let (ops, param_count) = Self::extract_function_ops(program, func_offset)?;

        // Generate Cranelift IR
        self.ctx.func.signature.params.clear();
        self.ctx.func.signature.returns.clear();

        // Function signature: (i64*...) -> i64
        // All values are i64 (stack values)
        let ptr_ty = self.ptr_ty;
        for _ in 0..param_count {
            self.ctx.func.signature.params.push(AbiParam::new(ptr_ty));
        }
        self.ctx.func.signature.returns.push(AbiParam::new(ptr_ty));

        // Build the function
        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);
            Self::translate_ops(&mut builder, &ops, param_count, ptr_ty)?;
            builder.finalize();
        }

        // Declare and define the function with tier-specific name
        let tier_suffix = match tier {
            CompilationTier::BaselineJit => "t1",
            CompilationTier::OptimizedJit => "t2",
            CompilationTier::Interpreter => "interp", // shouldn't happen
        };
        let func_name = format!("arth_fn_{}_{}", func_offset, tier_suffix);
        let func_id = self
            .module
            .declare_function(&func_name, Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| JitError::Module(e.to_string()))?;

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| JitError::Codegen(e.to_string()))?;

        self.module.clear_context(&mut self.ctx);

        // Finalize and get the code pointer
        self.module
            .finalize_definitions()
            .map_err(|e| JitError::Module(format!("{:?}", e)))?;

        let code_ptr = self.module.get_finalized_function(func_id);

        // Update function metadata based on tier
        if let Some(meta) = self.functions.get_mut(&func_offset) {
            match tier {
                CompilationTier::BaselineJit => {
                    meta.tier1_ptr = Some(code_ptr);
                    meta.tier = CompilationTier::BaselineJit;
                    meta.tier1_at_count = meta.call_count;
                }
                CompilationTier::OptimizedJit => {
                    meta.tier2_ptr = Some(code_ptr);
                    meta.tier = CompilationTier::OptimizedJit;
                }
                CompilationTier::Interpreter => {}
            }
            // Always update native_ptr to point to the highest tier compiled
            meta.native_ptr = Some(code_ptr);
            meta.compiling = false;
        }

        // Update stats
        let compile_time = start_time.elapsed().as_micros() as u64;
        self.stats.functions_compiled += 1;
        self.stats.compile_time_us += compile_time;

        match tier {
            CompilationTier::BaselineJit => {
                self.stats.tier1_compilations += 1;
                self.stats.tier1_compile_time_us += compile_time;
            }
            CompilationTier::OptimizedJit => {
                self.stats.tier2_compilations += 1;
                self.stats.tier2_compile_time_us += compile_time;
            }
            CompilationTier::Interpreter => {}
        }

        Ok(code_ptr)
    }

    /// Extract the bytecode operations for a function.
    fn extract_function_ops(program: &Program, offset: u32) -> Result<(Vec<Op>, u32), JitError> {
        let mut ops = Vec::new();
        let mut ip = offset as usize;
        let mut param_count = 0;

        // Count parameters by scanning for LocalGet with highest index
        while ip < program.code.len() {
            let op = &program.code[ip];
            match op {
                Op::LocalGet(idx) | Op::LocalSet(idx) => {
                    param_count = param_count.max(*idx + 1);
                }
                Op::Ret => {
                    ops.push(op.clone());
                    break;
                }
                _ => {}
            }
            ops.push(op.clone());
            ip += 1;
        }

        if ops.is_empty() {
            return Err(JitError::FunctionNotFound(offset));
        }

        Ok((ops, param_count))
    }

    /// Translate bytecode operations to Cranelift IR.
    fn translate_ops(
        builder: &mut FunctionBuilder,
        ops: &[Op],
        param_count: u32,
        ptr_ty: Type,
    ) -> Result<(), JitError> {
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        // Local storage for VM stack simulation
        let mut stack: Vec<Value> = Vec::new();
        let mut locals: HashMap<u32, Variable> = HashMap::new();

        // Initialize locals from parameters
        for i in 0..param_count {
            let var = builder.declare_var(ptr_ty);
            let param = builder.block_params(entry_block)[i as usize];
            builder.def_var(var, param);
            locals.insert(i, var);
        }

        // Skip prologue LocalSet ops that pop args into locals
        // The interpreter uses LocalSet(n-1), LocalSet(n-2), ..., LocalSet(0) to pop args
        // Since JIT initializes locals from params, we skip this prologue
        let mut skip_count = 0;
        let mut expected_local = param_count.saturating_sub(1) as i64;
        for op in ops.iter() {
            if let Op::LocalSet(idx) = op {
                if expected_local >= 0 && *idx == expected_local as u32 {
                    skip_count += 1;
                    expected_local -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Translate each opcode (skipping prologue)
        for op in ops.iter().skip(skip_count) {
            match op {
                Op::PushI64(val) => {
                    let v = builder.ins().iconst(ptr_ty, *val);
                    stack.push(v);
                }

                Op::PushBool(val) => {
                    let v = builder.ins().iconst(ptr_ty, *val as i64);
                    stack.push(v);
                }

                Op::AddI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().iadd(a, b);
                    stack.push(r);
                }

                Op::SubI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().isub(a, b);
                    stack.push(r);
                }

                Op::MulI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().imul(a, b);
                    stack.push(r);
                }

                Op::DivI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().sdiv(a, b);
                    stack.push(r);
                }

                Op::ModI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().srem(a, b);
                    stack.push(r);
                }

                Op::ShlI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().ishl(a, b);
                    stack.push(r);
                }

                Op::ShrI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().sshr(a, b);
                    stack.push(r);
                }

                Op::AndI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().band(a, b);
                    stack.push(r);
                }

                Op::OrI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().bor(a, b);
                    stack.push(r);
                }

                Op::XorI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let r = builder.ins().bxor(a, b);
                    stack.push(r);
                }

                Op::LtI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let cmp = builder.ins().icmp(IntCC::SignedLessThan, a, b);
                    let r = builder.ins().uextend(ptr_ty, cmp);
                    stack.push(r);
                }

                Op::EqI64 => {
                    let b = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let a = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let cmp = builder.ins().icmp(IntCC::Equal, a, b);
                    let r = builder.ins().uextend(ptr_ty, cmp);
                    stack.push(r);
                }

                Op::LocalGet(idx) => {
                    let var = locals
                        .get(idx)
                        .ok_or(JitError::Codegen(format!("Undefined local {}", idx)))?;
                    let v = builder.use_var(*var);
                    stack.push(v);
                }

                Op::LocalSet(idx) => {
                    let v = stack
                        .pop()
                        .ok_or(JitError::Codegen("Stack underflow".into()))?;
                    let var = *locals
                        .entry(*idx)
                        .or_insert_with(|| builder.declare_var(ptr_ty));
                    builder.def_var(var, v);
                }

                Op::Pop => {
                    stack.pop();
                }

                Op::Ret => {
                    let ret_val = stack
                        .pop()
                        .unwrap_or_else(|| builder.ins().iconst(ptr_ty, 0));
                    builder.ins().return_(&[ret_val]);
                }

                _ => {
                    // Unsupported opcodes cause fallback to interpreter
                    return Err(JitError::UnsupportedOpcode(format!("{:?}", op)));
                }
            }
        }

        Ok(())
    }
}

impl Default for JitContext {
    fn default() -> Self {
        Self::new().expect("Failed to create JIT context")
    }
}

// ============================================================================
// Runtime Helper Functions (called by JIT-compiled code)
// ============================================================================

/// Print an i64 value to stdout.
extern "C" fn arth_jit_print_i64(val: i64) {
    println!("{}", val);
}

/// Print a string to stdout (string is identified by handle).
extern "C" fn arth_jit_print_str(handle: i64) {
    // This would need access to the string pool - for now just print the handle
    println!("[string handle: {}]", handle);
}

/// Create a new list, return handle.
extern "C" fn arth_jit_list_new() -> i64 {
    // Placeholder - actual implementation would allocate a list
    0
}

/// Push a value to a list.
extern "C" fn arth_jit_list_push(list: i64, value: i64) -> i64 {
    // Placeholder
    let _ = (list, value);
    0
}

/// Get a value from a list by index.
extern "C" fn arth_jit_list_get(list: i64, index: i64) -> i64 {
    // Placeholder
    let _ = (list, index);
    0
}

/// Get list length.
extern "C" fn arth_jit_list_len(list: i64) -> i64 {
    // Placeholder
    let _ = list;
    0
}

/// Create a new map, return handle.
extern "C" fn arth_jit_map_new() -> i64 {
    // Placeholder
    0
}

/// Put a key-value pair in a map.
extern "C" fn arth_jit_map_put(map: i64, key: i64, value: i64) -> i64 {
    // Placeholder
    let _ = (map, key, value);
    0
}

/// Get a value from a map by key.
extern "C" fn arth_jit_map_get(map: i64, key: i64) -> i64 {
    // Placeholder
    let _ = (map, key);
    0
}

// ============================================================================
// Global JIT Context (thread-safe singleton)
// ============================================================================

use std::sync::OnceLock;

/// Global JIT context protected by a mutex.
/// Uses OnceLock for lazy initialization.
static GLOBAL_JIT: OnceLock<Mutex<JitContext>> = OnceLock::new();

/// Initialize the global JIT context.
pub fn init_jit() -> Result<(), JitError> {
    GLOBAL_JIT.get_or_init(|| Mutex::new(JitContext::new().expect("Failed to create JIT context")));
    Ok(())
}

/// Get the global JIT context.
pub fn with_jit<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut JitContext) -> R,
{
    let mutex = GLOBAL_JIT.get()?;
    let mut guard = mutex.lock().ok()?;
    Some(f(&mut guard))
}

/// Get JIT statistics.
pub fn get_jit_stats() -> Option<JitStats> {
    let mutex = GLOBAL_JIT.get()?;
    let guard = mutex.lock().ok()?;
    Some(guard.stats.clone())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    #[test]
    fn test_jit_context_creation() {
        let ctx = JitContext::new();
        assert!(ctx.is_ok());
    }

    #[test]
    fn test_function_meta() {
        let mut meta = FunctionMeta::new(100, 2);
        assert_eq!(meta.call_count, 0);
        assert!(meta.native_ptr.is_none());
        assert_eq!(meta.tier, CompilationTier::Interpreter);

        // Before threshold, no tier promotion
        for _ in 0..TIER1_THRESHOLD - 1 {
            assert!(meta.increment_call_count().is_none());
        }

        // At threshold, promote to Tier 1
        assert_eq!(
            meta.increment_call_count(),
            Some(CompilationTier::BaselineJit)
        );

        // Simulate Tier 1 compilation
        meta.tier = CompilationTier::BaselineJit;
        meta.tier1_at_count = meta.call_count;

        // After Tier 1, no immediate promotion
        assert!(meta.increment_call_count().is_none());

        // After TIER2_THRESHOLD more calls, promote to Tier 2
        for _ in 0..TIER2_THRESHOLD - 2 {
            assert!(meta.increment_call_count().is_none());
        }
        assert_eq!(
            meta.increment_call_count(),
            Some(CompilationTier::OptimizedJit)
        );

        // Simulate Tier 2 compilation
        meta.tier = CompilationTier::OptimizedJit;

        // At highest tier, no more promotions
        assert!(meta.increment_call_count().is_none());
    }

    #[test]
    fn test_compile_simple_function() {
        // Create a simple program: add two numbers and return
        // Function at offset 0: takes no args, returns 42
        let code = vec![Op::PushI64(40), Op::PushI64(2), Op::AddI64, Op::Ret];
        let program = Program::new(vec![], code);

        // Create JIT context and compile
        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0); // Register function at offset 0 with 0 params

        // Compile the function
        let result = jit.compile_function(&program, 0);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());

        // Call the native function
        let call_result = unsafe { jit.call_native(0, &[]) };
        assert!(
            call_result.is_ok(),
            "Native call failed: {:?}",
            call_result.err()
        );
        assert_eq!(call_result.unwrap(), 42, "Expected 40 + 2 = 42");
    }

    #[test]
    fn test_compile_function_with_params() {
        // Function that adds two parameters: (a, b) -> a + b
        let code = vec![
            Op::LocalGet(0), // Get first param
            Op::LocalGet(1), // Get second param
            Op::AddI64,
            Op::Ret,
        ];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 2); // Register function at offset 0 with 2 params

        // Compile the function
        let result = jit.compile_function(&program, 0);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());

        // Call with arguments 10 and 32
        let call_result = unsafe { jit.call_native(0, &[10, 32]) };
        assert!(
            call_result.is_ok(),
            "Native call failed: {:?}",
            call_result.err()
        );
        assert_eq!(call_result.unwrap(), 42, "Expected 10 + 32 = 42");

        // Call with different arguments
        let call_result = unsafe { jit.call_native(0, &[100, -50]) };
        assert!(call_result.is_ok());
        assert_eq!(call_result.unwrap(), 50, "Expected 100 + (-50) = 50");
    }

    #[test]
    fn test_compile_arithmetic_operations() {
        // Test various arithmetic operations
        // Compute: ((10 + 5) * 2) - 6 = 24
        let code = vec![
            Op::PushI64(10),
            Op::PushI64(5),
            Op::AddI64, // 15
            Op::PushI64(2),
            Op::MulI64, // 30
            Op::PushI64(6),
            Op::SubI64, // 24
            Op::Ret,
        ];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        let result = jit.compile_function(&program, 0);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());

        let call_result = unsafe { jit.call_native(0, &[]) };
        assert!(call_result.is_ok());
        assert_eq!(call_result.unwrap(), 24);
    }

    #[test]
    fn test_jit_stats() {
        let mut jit = JitContext::new().expect("JIT context creation failed");
        assert_eq!(jit.stats.functions_compiled, 0);

        let code = vec![Op::PushI64(42), Op::Ret];
        let program = Program::new(vec![], code);
        jit.register_function(0, 0);

        jit.compile_function(&program, 0).expect("compile");
        assert_eq!(jit.stats.functions_compiled, 1);
        assert!(jit.stats.compile_time_us > 0);
    }

    #[test]
    fn test_tiered_compilation() {
        // Test that functions are compiled at both Tier 1 and Tier 2
        let code = vec![Op::LocalGet(0), Op::LocalGet(1), Op::AddI64, Op::Ret];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 2);

        // Compile at Tier 1 (baseline)
        let result1 = jit.compile_function_at_tier(&program, 0, CompilationTier::BaselineJit);
        assert!(
            result1.is_ok(),
            "Tier 1 compilation failed: {:?}",
            result1.err()
        );

        // Verify Tier 1 stats
        assert_eq!(jit.stats.tier1_compilations, 1);
        assert_eq!(jit.stats.tier2_compilations, 0);
        assert!(jit.stats.tier1_compile_time_us > 0);

        // Function should work at Tier 1
        let call_result = unsafe { jit.call_native(0, &[10, 32]) };
        assert!(call_result.is_ok());
        assert_eq!(call_result.unwrap(), 42);

        // Compile at Tier 2 (optimized)
        let result2 = jit.compile_function_at_tier(&program, 0, CompilationTier::OptimizedJit);
        assert!(
            result2.is_ok(),
            "Tier 2 compilation failed: {:?}",
            result2.err()
        );

        // Verify Tier 2 stats
        assert_eq!(jit.stats.tier1_compilations, 1);
        assert_eq!(jit.stats.tier2_compilations, 1);
        assert_eq!(jit.stats.functions_compiled, 2);
        assert!(jit.stats.tier2_compile_time_us > 0);

        // Function should work at Tier 2
        let call_result = unsafe { jit.call_native(0, &[100, -50]) };
        assert!(call_result.is_ok());
        assert_eq!(call_result.unwrap(), 50);

        // Verify metadata reflects Tier 2
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::OptimizedJit);
        assert!(meta.tier1_ptr.is_some());
        assert!(meta.tier2_ptr.is_some());
    }

    #[test]
    fn test_tier_promotion_flow() {
        // Test the complete tier promotion flow: Interpreter → Tier 1 → Tier 2
        let code = vec![Op::PushI64(42), Op::Ret];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        // Initially at Interpreter tier
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::Interpreter);

        // Simulate calls up to (but not including) Tier 1 threshold
        for _ in 0..TIER1_THRESHOLD - 1 {
            let (ptr, tier) = jit.get_function(0);
            assert!(ptr.is_none(), "Should not have native ptr before threshold");
            assert!(
                tier.is_none(),
                "Should not trigger compilation before threshold"
            );
        }

        // Next call should trigger Tier 1 compilation
        let (ptr, tier) = jit.get_function(0);
        assert!(
            ptr.is_none(),
            "Should not have native ptr yet (just triggered)"
        );
        assert_eq!(
            tier,
            Some(CompilationTier::BaselineJit),
            "Should trigger Tier 1"
        );

        // Simulate Tier 1 compilation
        jit.compile_function_at_tier(&program, 0, CompilationTier::BaselineJit)
            .expect("Tier 1 compile should succeed");

        // Now we should have native code
        let (ptr, _) = jit.get_function(0);
        assert!(ptr.is_some(), "Should have native ptr after Tier 1");

        // Simulate TIER2_THRESHOLD more calls
        for _ in 0..TIER2_THRESHOLD - 2 {
            let (ptr, tier) = jit.get_function(0);
            assert!(ptr.is_some(), "Should have native ptr");
            assert!(tier.is_none(), "Should not trigger Tier 2 yet");
        }

        // Next call should trigger Tier 2 compilation
        let (ptr, tier) = jit.get_function(0);
        assert!(ptr.is_some(), "Should still have native ptr");
        assert_eq!(
            tier,
            Some(CompilationTier::OptimizedJit),
            "Should trigger Tier 2"
        );
    }

    #[test]
    fn test_deopt_reason_display() {
        // Test that DeoptReason has proper Display impl
        assert_eq!(format!("{}", DeoptReason::None), "none");
        assert_eq!(format!("{}", DeoptReason::TypeMismatch), "type mismatch");
        assert_eq!(format!("{}", DeoptReason::Overflow), "overflow");
        assert_eq!(
            format!("{}", DeoptReason::DivisionByZero),
            "division by zero"
        );
        assert_eq!(
            format!("{}", DeoptReason::TooManyDeopts),
            "too many deoptimizations"
        );
    }

    #[test]
    fn test_deopt_info_creation() {
        // Test DeoptInfo creation
        let info = DeoptInfo::new(DeoptReason::TypeMismatch, 100);
        assert_eq!(info.reason, DeoptReason::TypeMismatch);
        assert_eq!(info.bytecode_offset, 100);
        assert!(info.locals.is_empty());
        assert!(info.stack.is_empty());
        assert!(info.timestamp_us > 0);

        // Test with state
        let info_with_state =
            DeoptInfo::with_state(DeoptReason::Overflow, 200, vec![1, 2, 3], vec![4, 5]);
        assert_eq!(info_with_state.reason, DeoptReason::Overflow);
        assert_eq!(info_with_state.bytecode_offset, 200);
        assert_eq!(info_with_state.locals, vec![1, 2, 3]);
        assert_eq!(info_with_state.stack, vec![4, 5]);
        assert_eq!(info_with_state.local_count, 3);
    }

    #[test]
    fn test_function_meta_record_deopt() {
        let mut meta = FunctionMeta::new(100, 2);
        assert_eq!(meta.deopt_count, 0);
        assert_eq!(meta.last_deopt_reason, DeoptReason::None);
        assert!(!meta.jit_disabled);

        // Record first deopt
        let disabled = meta.record_deopt(DeoptReason::TypeMismatch);
        assert!(!disabled, "Should not disable after first deopt");
        assert_eq!(meta.deopt_count, 1);
        assert_eq!(meta.last_deopt_reason, DeoptReason::TypeMismatch);
        assert!(!meta.jit_disabled);

        // Record more deopts up to threshold - 1
        for _ in 1..MAX_DEOPTS_PER_FUNCTION - 1 {
            let disabled = meta.record_deopt(DeoptReason::Overflow);
            assert!(!disabled, "Should not disable before threshold");
        }
        assert_eq!(meta.deopt_count, MAX_DEOPTS_PER_FUNCTION - 1);
        assert!(!meta.jit_disabled);

        // At threshold, should disable JIT
        let disabled = meta.record_deopt(DeoptReason::DivisionByZero);
        assert!(disabled, "Should disable at threshold");
        assert_eq!(meta.deopt_count, MAX_DEOPTS_PER_FUNCTION);
        assert_eq!(meta.last_deopt_reason, DeoptReason::DivisionByZero);
        assert!(meta.jit_disabled);
        assert!(meta.native_ptr.is_none(), "Native ptr should be cleared");
        assert_eq!(meta.tier, CompilationTier::Interpreter);
    }

    #[test]
    fn test_function_meta_deoptimize() {
        let mut meta = FunctionMeta::new(100, 2);

        // Simulate being at Tier 2
        meta.tier = CompilationTier::OptimizedJit;
        meta.tier1_ptr = Some(0x1000 as *const u8);
        meta.tier2_ptr = Some(0x2000 as *const u8);
        meta.native_ptr = meta.tier2_ptr;

        // Deoptimize from Tier 2 - should fall back to Tier 1
        let new_tier = meta.deoptimize(DeoptReason::TypeMismatch);
        assert_eq!(new_tier, CompilationTier::BaselineJit);
        assert_eq!(meta.tier, CompilationTier::BaselineJit);
        assert_eq!(meta.native_ptr, meta.tier1_ptr);
        assert!(meta.tier2_ptr.is_none(), "Tier 2 ptr should be invalidated");
        assert_eq!(meta.deopt_count, 1);

        // Deoptimize from Tier 1 - should fall back to interpreter
        let new_tier = meta.deoptimize(DeoptReason::Overflow);
        assert_eq!(new_tier, CompilationTier::Interpreter);
        assert_eq!(meta.tier, CompilationTier::Interpreter);
        assert!(meta.native_ptr.is_none());
        assert!(meta.tier1_ptr.is_none(), "Tier 1 ptr should be invalidated");
        assert_eq!(meta.deopt_count, 2);
    }

    #[test]
    fn test_function_meta_should_use_jit() {
        let mut meta = FunctionMeta::new(100, 2);

        // Initially, should not use JIT (no native ptr)
        assert!(!meta.should_use_jit());

        // After compilation, should use JIT
        meta.native_ptr = Some(0x1000 as *const u8);
        assert!(meta.should_use_jit());

        // After disabling JIT, should not use JIT
        meta.jit_disabled = true;
        assert!(!meta.should_use_jit());
    }

    #[test]
    fn test_jit_context_record_deopt() {
        let code = vec![Op::LocalGet(0), Op::LocalGet(1), Op::AddI64, Op::Ret];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 2);

        // Compile to Tier 2
        jit.compile_function_at_tier(&program, 0, CompilationTier::BaselineJit)
            .expect("Tier 1 compile should succeed");
        jit.compile_function_at_tier(&program, 0, CompilationTier::OptimizedJit)
            .expect("Tier 2 compile should succeed");

        // Verify function is at Tier 2
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::OptimizedJit);

        // Record deopt - should fall back to Tier 1
        let new_tier = jit.record_deopt(0, DeoptReason::TypeMismatch, None);
        assert_eq!(new_tier, Some(CompilationTier::BaselineJit));

        // Verify stats
        assert_eq!(jit.stats.deopt_count, 1);
        assert_eq!(
            jit.stats.deopts_by_reason[DeoptReason::TypeMismatch as usize],
            1
        );

        // Verify function state
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::BaselineJit);
        assert_eq!(meta.deopt_count, 1);
    }

    #[test]
    fn test_jit_context_force_deopt_to_interpreter() {
        let code = vec![Op::PushI64(42), Op::Ret];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        // Compile to Tier 1
        jit.compile_function(&program, 0)
            .expect("compile should succeed");

        // Verify function is at Tier 1
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::BaselineJit);
        assert!(meta.native_ptr.is_some());

        // Force deopt to interpreter
        let success = jit.force_deopt_to_interpreter(0, DeoptReason::ExplicitBailout);
        assert!(success);

        // Verify function is at interpreter
        let meta = jit.functions.get(&0).expect("function should exist");
        assert_eq!(meta.tier, CompilationTier::Interpreter);
        assert!(meta.native_ptr.is_none());
        assert_eq!(meta.deopt_count, 1);
        assert_eq!(jit.stats.deopt_count, 1);
    }

    #[test]
    fn test_jit_context_is_jit_disabled() {
        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        // Initially not disabled
        assert!(!jit.is_jit_disabled(0));

        // After many deopts, should be disabled
        {
            let meta = jit.functions.get_mut(&0).expect("function should exist");
            for _ in 0..MAX_DEOPTS_PER_FUNCTION {
                meta.record_deopt(DeoptReason::Unknown);
            }
        }

        assert!(jit.is_jit_disabled(0));
    }

    #[test]
    fn test_jit_context_get_function_deopt_stats() {
        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        // Initially no deopts
        let stats = jit.get_function_deopt_stats(0);
        assert_eq!(stats, Some((0, DeoptReason::None, false)));

        // After a deopt
        jit.record_deopt(0, DeoptReason::Overflow, None);
        let stats = jit.get_function_deopt_stats(0);
        assert_eq!(stats, Some((1, DeoptReason::Overflow, false)));

        // Unknown function
        let stats = jit.get_function_deopt_stats(999);
        assert!(stats.is_none());
    }

    #[test]
    fn test_jit_disabled_prevents_promotion() {
        let mut meta = FunctionMeta::new(100, 2);

        // Disable JIT
        meta.jit_disabled = true;

        // Even with many calls, should not trigger promotion
        for _ in 0..TIER1_THRESHOLD + TIER2_THRESHOLD {
            assert!(meta.increment_call_count().is_none());
        }

        // should_compile should also return false
        assert!(!meta.should_compile());
    }

    #[test]
    fn test_try_call_jit_with_disabled_function() {
        let code = vec![Op::PushI64(42), Op::Ret];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");
        jit.register_function(0, 0);

        // Compile the function
        jit.compile_function(&program, 0)
            .expect("compile should succeed");

        // Disable JIT for this function
        {
            let meta = jit.functions.get_mut(&0).expect("function should exist");
            meta.jit_disabled = true;
        }

        // try_call_jit should return None (use interpreter)
        let result = jit.try_call_jit(0, &[]);
        assert!(result.is_none());
        assert!(jit.stats.interpreter_fallbacks > 0);
    }
}
