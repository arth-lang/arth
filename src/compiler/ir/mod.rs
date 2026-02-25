#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Block(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Value(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderField {
    pub name: String,
    pub ty: Ty,
    pub is_shared: bool,
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub name: String,
    pub fields: Vec<ProviderField>,
}

// =============================================================================
// Struct and Enum Definitions for LLVM Native Compilation
// =============================================================================

/// Field definition for a struct type.
#[derive(Clone, Debug)]
pub struct StructFieldDef {
    pub name: String,
    pub ty: Ty,
}

/// Struct type definition for LLVM codegen.
#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructFieldDef>,
}

/// Variant definition for an enum type.
#[derive(Clone, Debug)]
pub struct EnumVariantDef {
    pub name: String,
    pub payload_types: Vec<Ty>,
}

/// Enum type definition for LLVM codegen.
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<EnumVariantDef>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    I64,
    F64,
    I1,
    Ptr, // Opaque pointer (LLVM opaque pointers)
    Void,
    /// Reference to a named struct type (for nested struct fields)
    Struct(String),
    /// Reference to a named enum type
    Enum(String),
    /// Optional<T> - represented as { i1 is_some, T value }
    Optional(Box<Ty>),
    /// String type - represented as { ptr, i64 } (pointer + length)
    String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    And,
    Or,
    Xor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmpPred {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum InstKind {
    // Constants and simple moves
    ConstI64(i64),
    ConstF64(f64),
    // Pointer to an interned string literal in the IR module string pool
    ConstStr(u32),
    Copy(Value),

    // Integer and bitwise ops
    Binary(BinOp, Value, Value),

    // Comparisons producing i1
    Cmp(CmpPred, Value, Value),

    // String equality comparison: (str_a, str_b) -> i1
    StrEq(Value, Value),

    // String concatenation: (str_a, str_b) -> new_string
    StrConcat(Value, Value),

    // Memory
    Alloca,              // returns Ptr
    Load(Value),         // ptr -> value (assume i64 for now)
    Store(Value, Value), // (ptr, value)

    // Native struct operations (for LLVM backend with GEP-based field access)
    // Allocate a struct on the stack. Returns a pointer to the struct.
    StructAlloc {
        type_name: String,
    },
    // Load a field from a struct pointer. Returns the field value.
    StructFieldGet {
        ptr: Value,
        type_name: String,
        field_name: String,
        field_index: u32,
    },
    // Store a value to a struct field.
    StructFieldSet {
        ptr: Value,
        type_name: String,
        field_name: String,
        field_index: u32,
        value: Value,
    },

    // Native enum operations (for LLVM backend)
    // Allocate an enum on the stack. Returns a pointer to the enum.
    EnumAlloc {
        type_name: String,
    },
    // Get the tag (discriminant) from an enum pointer.
    EnumGetTag {
        ptr: Value,
        type_name: String,
    },
    // Set the tag on an enum.
    EnumSetTag {
        ptr: Value,
        type_name: String,
        tag: i32,
    },
    // Get a payload value from an enum variant.
    EnumGetPayload {
        ptr: Value,
        type_name: String,
        payload_index: u32,
    },
    // Set a payload value on an enum variant.
    EnumSetPayload {
        ptr: Value,
        type_name: String,
        payload_index: u32,
        value: Value,
    },

    // Calls
    Call {
        name: String,
        args: Vec<Value>,
        ret: Ty,
    },

    /// FFI call to an external function (unsafe context required)
    /// Unlike regular Call, extern calls use C calling convention
    /// and do not support Arth exception handling.
    ExternCall {
        name: String,
        args: Vec<Value>,
        /// Parameter ABI types for this call (from the extern declaration).
        /// Kept explicit because IR values are not globally typed.
        params: Vec<Ty>,
        ret: Ty,
    },

    // Closure operations
    // Create a closure object: (function_name, captured_values) -> closure_handle
    MakeClosure {
        func: String,
        captures: Vec<Value>,
    },
    // Call a closure indirectly: (closure_handle, args) -> result
    ClosureCall {
        closure: Value,
        args: Vec<Value>,
        ret: Ty,
    },

    /// Landing pad for exception handling.
    /// Yields an exception token/object for use in catch blocks.
    ///
    /// For LLVM DWARF exception handling:
    /// - `catch_types`: List of exception type names this landing pad catches.
    ///   Each type will generate a `catch ptr @_arth_typeinfo_TypeName` clause.
    /// - `is_catch_all`: If true, adds a `catch ptr null` for catch-all behavior.
    ///
    /// The landingpad instruction returns { ptr, i32 } where:
    /// - ptr is the exception object pointer
    /// - i32 is the type selector (1-indexed into catch_types, or 0 for catch-all)
    LandingPad {
        /// Exception types to catch (in order of catch clauses)
        catch_types: Vec<String>,
        /// Whether this landing pad includes a catch-all clause
        is_catch_all: bool,
    },

    // Exception handler registration: set the unwind handler to the given block.
    // When an exception is thrown, control transfers to this block.
    SetUnwindHandler(Block),

    // Clear the current unwind handler (pop from handler stack).
    ClearUnwindHandler,

    // SSA join of values coming from predecessors.
    // Each operand is paired with the predecessor block it corresponds to.
    Phi(Vec<(Block, Value)>),

    // Drop/RAII: invoke deinit function for a value that needs cleanup.
    // The `ty_name` is the qualified type name used to resolve the deinit function.
    // For a struct `Foo`, this calls `FooFns.deinit(value)`.
    Drop {
        value: Value,
        ty_name: String,
    },

    // Conditional drop: invoke deinit only if the flag is false (value not moved).
    // Used for values that may or may not be moved depending on control flow.
    // The flag is true if the value was moved, false if it still needs dropping.
    CondDrop {
        value: Value,
        flag: Value,
        ty_name: String,
    },

    // Field drop: drop a single field of a struct for partial move cleanup.
    // Used when a struct has some fields moved but needs the remaining fields dropped.
    // The `field_name` identifies which field to access and drop.
    // The `ty_name` is the type of the field (for calling its deinit).
    FieldDrop {
        value: Value,
        field_name: String,
        ty_name: String,
    },

    // Reference counting operations for managed memory
    // Allocate a new reference-counted cell containing initial_value.
    // Returns a handle to the RC cell. The reference count starts at 1.
    RcAlloc {
        initial_value: Value,
    },

    // Increment reference count for an RC handle. Returns the handle unchanged.
    RcInc {
        handle: Value,
    },

    // Decrement reference count for an RC handle. If count reaches 0,
    // the value is deallocated and deinit is called if ty_name is provided.
    // Returns 0 on success.
    RcDec {
        handle: Value,
        ty_name: Option<String>,
    },

    // Load the current value from an RC cell. Returns the contained value.
    RcLoad {
        handle: Value,
    },

    // Store a new value into an RC cell (for mutable RC cells).
    RcStore {
        handle: Value,
        value: Value,
    },

    // Get the current reference count (for debugging/testing). Returns i64.
    RcGetCount {
        handle: Value,
    },

    // Region-based allocation operations for loop-local values
    /// Enter a region - creates an arena for allocations tied to a loop iteration.
    /// All values allocated in this region will be bulk-deallocated when the region exits.
    /// The region_id is a unique identifier generated during type checking.
    RegionEnter {
        region_id: u32,
    },

    /// Exit a region - bulk-deallocates all values allocated in this region.
    /// Calls deinit for each value with a deinit function before deallocation.
    /// The deinit_calls contains (value, ty_name) pairs for values needing cleanup.
    RegionExit {
        region_id: u32,
        deinit_calls: Vec<(Value, String)>,
    },

    /// Get the type name of a struct value. Used for exception type dispatch.
    /// Returns a string representing the struct's type name.
    GetTypeName(Value),

    // ========== Async State Machine Operations ==========
    /// Allocate an async frame on the heap.
    /// Returns a handle (i64) to the allocated frame.
    /// The frame is initialized with state=0.
    AsyncFrameAlloc {
        /// Symbolic name of the frame type (e.g., "myFunc$async_frame")
        frame_name: String,
        /// Total size of the frame in bytes
        frame_size: u32,
    },

    /// Free an async frame. Called when task completes or is cancelled.
    /// Does NOT invoke drop functions - that should be done before this.
    AsyncFrameFree {
        frame_ptr: Value,
    },

    /// Get the current state ID from an async frame.
    /// Returns the state discriminant (u32 as i64).
    AsyncFrameGetState {
        frame_ptr: Value,
    },

    /// Set the state ID in an async frame.
    /// Used when transitioning between states at await points.
    AsyncFrameSetState {
        frame_ptr: Value,
        state_id: u32,
    },

    /// Load a field from the async frame at the given byte offset.
    /// Used to restore locals when resuming a suspended coroutine.
    AsyncFrameLoad {
        frame_ptr: Value,
        field_offset: u32,
        ty: Ty,
    },

    /// Store a value to the async frame at the given byte offset.
    /// Used to save locals before yielding at an await point.
    AsyncFrameStore {
        frame_ptr: Value,
        field_offset: u32,
        value: Value,
    },

    /// Check if the current task has been cancelled.
    /// Returns 1 if cancelled, 0 otherwise.
    /// Poll functions should check this at the start of each state.
    AsyncCheckCancelled {
        task_handle: Value,
    },

    /// Yield to the scheduler, indicating the task is waiting on another task.
    /// The awaited_task is the handle of the task we're waiting for.
    /// This instruction prepares for returning Pending from the poll function.
    AsyncYield {
        awaited_task: Value,
    },

    /// Mark an await point in the IR for analysis purposes.
    /// This instruction is replaced during async lowering with actual
    /// state save/restore logic. Contains metadata about borrows held
    /// across the await point.
    AwaitPoint {
        /// The task handle being awaited
        awaited_task: Value,
        /// Borrows that are live across this await (for borrow checking)
        live_borrows: Vec<AwaitBorrowMeta>,
    },

    // ========== Provider Operations ==========
    /// Create a new provider instance: (provider_name, field_values) -> provider_handle
    ProviderNew {
        name: String,
        values: Vec<(String, Value)>,
    },
    /// Get a field from a provider instance: (obj, provider_name, field_name) -> value
    ProviderFieldGet {
        obj: Value,
        provider: String,
        field: String,
    },
    /// Set a field in a provider instance: (obj, provider_name, field_name, value) -> 0
    ProviderFieldSet {
        obj: Value,
        provider: String,
        field: String,
        value: Value,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Inst {
    pub result: Value,
    pub kind: InstKind,
    pub span: Option<Span>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Terminator {
    Ret(Option<Value>),
    Br(Block),
    CondBr {
        cond: Value,
        then_bb: Block,
        else_bb: Block,
    },
    // Multi-way branch on an integer scrutinee
    Switch {
        scrut: Value,
        default: Block,
        cases: Vec<(i64, Block)>,
    },
    // Marks code paths that cannot be reached
    Unreachable,
    // Start of exception propagation. Placeholder until EH is modeled; treated as no-successor.
    Throw(Option<Value>),
    // Panic: unrecoverable error that unwinds within task boundary.
    // The Value is a string message describing the panic reason.
    // Panics execute drops along the unwind path and propagate to join() as TaskPanicked.
    // Unlike Throw, panics cannot be caught by try/catch blocks.
    Panic(Option<Value>),
    // Call that may unwind: on success, continue at `normal`; on exception, branch to `unwind`.
    // If `ret` is not Void, `result` must contain the SSA value assigned by the invoke.
    Invoke {
        callee: String,
        args: Vec<Value>,
        ret: Ty,
        result: Option<Value>,
        normal: Block,
        unwind: Block,
    },

    // ========== Async Poll Function Terminators ==========
    /// Return from a poll function with a result status.
    /// Used to indicate whether the async function completed (Ready),
    /// needs to wait (Pending), was cancelled, or panicked.
    PollReturn {
        /// The poll result status
        result: PollResult,
        /// The return value (only meaningful when result is Ready)
        value: Option<Value>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct BlockData {
    pub name: String,
    pub insts: Vec<Inst>,
    pub term: Terminator,
    pub span: Option<Span>,
}

/// LLVM linkage types for function symbols.
///
/// Maps Arth visibility to LLVM linkage:
/// - Public → External (visible to other modules)
/// - Internal → Internal (visible within compilation unit)
/// - Private → Private (visible only within defining module)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Linkage {
    /// Externally visible (default for public functions)
    #[default]
    External,
    /// Internal linkage (visible within compilation unit, like C static)
    Internal,
    /// Private linkage (only visible within defining module)
    Private,
    /// Weak linkage (can be overridden by strong symbols)
    Weak,
    /// Link-once ODR (for inline functions, templates)
    LinkOnceODR,
}

impl Linkage {
    /// Convert to LLVM IR linkage keyword
    pub fn to_llvm_str(self) -> &'static str {
        match self {
            Linkage::External => "", // default, no keyword needed
            Linkage::Internal => "internal ",
            Linkage::Private => "private ",
            Linkage::Weak => "weak ",
            Linkage::LinkOnceODR => "linkonce_odr ",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub blocks: Vec<BlockData>,
    /// LLVM linkage for this function
    pub linkage: Linkage,
    /// Source location span for this function (for debug info)
    pub span: Option<Span>,
}

/// External function declaration for FFI
#[derive(Clone, Debug)]
pub struct ExternFunc {
    pub name: String,
    /// ABI specification (e.g., "C", "stdcall")
    pub abi: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub name: String,
    pub funcs: Vec<Func>,
    /// External function declarations (FFI)
    pub extern_funcs: Vec<ExternFunc>,
    pub providers: Vec<Provider>,
    pub strings: Vec<String>,
    /// Struct type definitions for LLVM native compilation
    pub structs: Vec<StructDef>,
    /// Enum type definitions for LLVM native compilation
    pub enums: Vec<EnumDef>,
}

impl Module {
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            funcs: vec![],
            extern_funcs: vec![],
            providers: vec![],
            strings: vec![],
            structs: vec![],
            enums: vec![],
        }
    }
}

// Very small demo function builder to showcase lowering to LLVM text.
pub fn demo_add_module() -> Module {
    // define i64 @add(i64 %a, i64 %b) { entry: %0 = add i64 %a, %b; ret i64 %0 }
    let a = Value(0);
    let b = Value(1);
    let res = Value(2);
    let _entry = Block(0);
    let inst = Inst {
        result: res,
        kind: InstKind::Binary(BinOp::Add, a, b),
        span: None,
    };
    let block = BlockData {
        name: "entry".into(),
        insts: vec![inst],
        term: Terminator::Ret(Some(res)),
        span: None,
    };
    let func = Func {
        name: "add".into(),
        params: vec![Ty::I64, Ty::I64],
        ret: Ty::I64,
        blocks: vec![block],
        linkage: Linkage::External,
        span: None,
    };
    let mut m = Module::new("demo");
    m.funcs.push(func);
    m
}
pub mod async_lower;
pub mod cfg;
pub mod dom;
pub mod opt;
pub mod ssa;
pub mod verify;
// Reuse HIR spans for IR debug info
pub type Span = crate::compiler::hir::core::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BorrowKind {
    ExclusiveLocal,
    Provider,
    Release,
}

#[derive(Clone, Debug)]
pub struct BorrowEvent {
    pub kind: BorrowKind,
    pub span: Option<Span>,
}

// ============================================================================
// Async State Machine Types
// ============================================================================

/// Result of polling an async function.
/// Determines whether the async computation has finished or needs to yield.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PollResult {
    /// Task completed successfully with a value.
    Ready = 0,
    /// Task needs to yield and be resumed later (waiting on another task).
    Pending = 1,
    /// Task was cancelled by the caller.
    Cancelled = 2,
    /// Task encountered an unhandled panic.
    Panicked = 3,
    /// Task threw an exception that should be propagated to the awaiter.
    /// The exception value is stored in the async frame's exception slot.
    Error = 4,
}

/// Metadata about a borrow that must be tracked across an await point.
/// Used for borrow checking to ensure references don't escape async boundaries.
#[derive(Clone, Debug)]
pub struct AwaitBorrowMeta {
    /// The value being borrowed (if tracked)
    pub holder: Option<Value>,
    /// Whether this is an exclusive (mutable) borrow
    pub is_exclusive: bool,
    /// Origin description of the borrow (for error messages)
    pub origin: String,
    /// The SSA value representing the borrow
    pub value: Option<Value>,
    /// Source span for diagnostics
    pub span: Option<Span>,
}

/// A field in the async frame structure.
/// Each field stores either a parameter or a local variable that must
/// survive across await points.
#[derive(Clone, Debug)]
pub struct AsyncFrameField {
    /// Field name (e.g., "param_0", "local_x")
    pub name: String,
    /// IR type of the field
    pub ty: Ty,
    /// Byte offset within the frame (relative to frame start)
    pub offset: u32,
    /// Original SSA value this field captures (for remapping)
    pub original_value: Option<Value>,
    /// Whether this field needs drop() called on cancellation
    pub needs_drop: bool,
    /// Type name for drop resolution (e.g., "MyStruct")
    pub drop_ty_name: Option<String>,
}

/// The async frame structure - holds all state for a suspended async function.
/// This is the "coroutine frame" that persists across yield/resume cycles.
///
/// Frame layout:
/// ```text
/// Offset 0:   state: u32        // Current state ID
/// Offset 4:   (padding)
/// Offset 8:   result: T         // Return value slot (8 bytes)
/// Offset 16:  exception: i64    // Exception handle (if any)
/// Offset 24:  awaited: i64      // Currently awaited task handle
/// Offset 32+: user fields       // Parameters and cross-await locals
/// ```
#[derive(Clone, Debug)]
pub struct AsyncFrame {
    /// Frame type name (e.g., "myFunc$async_frame")
    pub name: String,
    /// Total size of the frame in bytes
    pub size: u32,
    /// User-defined fields (parameters + cross-await locals)
    pub fields: Vec<AsyncFrameField>,
    /// Offset of the state discriminant field (always 0)
    pub state_offset: u32,
    /// Offset of the return value slot
    pub result_offset: u32,
    /// Offset of the exception info slot
    pub exception_offset: u32,
    /// Offset of the awaited task handle slot
    pub awaited_offset: u32,
}

impl AsyncFrame {
    /// Reserved offsets for system fields
    pub const STATE_OFFSET: u32 = 0;
    pub const RESULT_OFFSET: u32 = 8;
    pub const EXCEPTION_OFFSET: u32 = 16;
    pub const AWAITED_OFFSET: u32 = 24;
    pub const USER_FIELDS_START: u32 = 32;
}

/// A state in the async state machine.
/// Each await point creates a new state that the poll function can resume to.
#[derive(Clone, Debug)]
pub struct AsyncState {
    /// Unique state ID (0 = entry state, before any await)
    pub id: u32,
    /// Debug name for this state (e.g., "entry", "resume_await_0")
    pub name: String,
    /// Entry block for this state in the poll function
    pub entry_block: Block,
    /// Index of the await point that transitions TO this state (-1 for entry)
    /// State N corresponds to the resume point after await point N-1.
    pub from_await_idx: i32,
    /// Variables that are live during this state and must be in the frame
    pub preserved_locals: Vec<String>,
    /// Borrows that are live during this state (for borrow checking)
    pub live_borrows: Vec<AwaitBorrowMeta>,
}

/// Kind of state transition in the async state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionKind {
    /// Normal transition: await completed, moving to next state
    AwaitComplete,
    /// Early return from async function
    Return,
    /// Task completed (reached final state)
    Complete,
    /// Transition to cancellation handler
    Cancellation,
    /// Transition to panic handler
    Panic,
}

/// A transition between states in the async state machine.
#[derive(Clone, Debug)]
pub struct StateTransition {
    /// Source state ID
    pub from_state: u32,
    /// Destination state ID
    pub to_state: u32,
    /// Kind of transition
    pub kind: TransitionKind,
    /// Drops to execute during this transition (value, type_name)
    pub drops: Vec<(Value, String)>,
}

/// Complete async state machine for an async function.
/// This is the result of lowering an async function body into a poll-based
/// state machine with an async frame.
#[derive(Clone, Debug)]
pub struct AsyncStateMachine {
    /// Original async function name
    pub original_name: String,
    /// The async frame structure
    pub frame: AsyncFrame,
    /// All states in the machine
    pub states: Vec<AsyncState>,
    /// State transitions
    pub transitions: Vec<StateTransition>,
    /// Generated wrapper function (allocates frame, spawns task)
    pub wrapper_func: Func,
    /// Generated poll function (state machine dispatch)
    pub poll_func: Func,
    /// Generated drop function (cleanup on cancellation), if needed
    pub drop_func: Option<Func>,
}

pub fn collect_borrow_events(func: &Func) -> Vec<BorrowEvent> {
    let mut out = Vec::new();
    for b in &func.blocks {
        for inst in &b.insts {
            if let InstKind::Call { name, .. } = &inst.kind {
                let kind = match name.as_str() {
                    "borrowMut" => Some(BorrowKind::ExclusiveLocal),
                    "borrowFromProvider" => Some(BorrowKind::Provider),
                    "release" => Some(BorrowKind::Release),
                    _ => None,
                };
                if let Some(k) = kind {
                    out.push(BorrowEvent {
                        kind: k,
                        span: inst.span.clone(),
                    });
                }
            }
        }
    }
    out
}
