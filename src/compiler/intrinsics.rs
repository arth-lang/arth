//! Intrinsic Registry
//!
//! This module provides a centralized registry mapping intrinsic names to VM operations.
//! Intrinsics are functions that are implemented directly by the VM runtime rather than
//! as Arth code. They are declared in `.arth` files with the `@intrinsic("name")` attribute.
//!
//! # Design
//!
//! The intrinsic system follows these principles:
//! 1. **Single source of truth**: `stdlib/src/**/*.arth` defines all stdlib APIs
//! 2. **Intrinsic registry**: This file maps intrinsic names to VM ops
//! 3. **Compiler reads stdlib**: No manual signature seeding needed
//! 4. **No pattern matching on names**: Lowering uses `@intrinsic` attribute
//!
//! # Adding a New Intrinsic
//!
//! 1. Add the function to the appropriate `stdlib/src/<domain>/<Module>.arth`
//! 2. Mark it with `@intrinsic("domain.operation")` attribute
//! 3. Add an entry to the `INTRINSICS` array in this file
//! 4. Implement the VM op in `crates/arth-vm/src/`
//! 5. Add tests in `stdlib/tests/`
//!
//! # Naming Convention
//!
//! Intrinsic names follow the pattern `domain.operation`:
//! - `math.sqrt` - Math.sqrt function
//! - `io.file_read` - File.read function
//! - `list.push` - List.push function

use std::collections::HashMap;
use std::sync::OnceLock;

/// Classification of intrinsics by their host capability domain.
///
/// This allows the compiler to emit appropriate host call opcodes and enables
/// capability-based sandboxing for TS guests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntrinsicKind {
    /// Core VM operations (list, map, math, json, etc.) - always available
    CoreVm,
    /// I/O operations (file, directory, console) - requires `io` capability
    HostIo,
    /// Networking operations (HTTP, WebSocket, SSE) - requires `net` capability
    HostNet,
    /// Time operations (DateTime, Instant) - requires `time` capability
    HostTime,
    /// Database operations (SQLite, PostgreSQL) - requires `db` capability
    HostDb,
    /// Cryptographic operations (hashing, encryption, signatures) - requires `crypto` capability
    HostCrypto,
}

/// Represents a VM operation that an intrinsic maps to.
/// This corresponds to `arth_vm::Op` variants.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VmOp {
    // Math operations
    MathSqrt,
    MathPow,
    MathSin,
    MathCos,
    MathTan,
    MathFloor,
    MathCeil,
    MathRound,
    // Removed: MathRoundN, MathMinF, MathMaxF, MathClampF, MathAbsF,
    //          MathMinI, MathMaxI, MathClampI, MathAbsI
    // These are now pure Arth in stdlib/src/math/Math.arth

    // I/O operations
    Print,
    PrintLn,
    FileOpen,
    FileClose,
    FileRead,
    // Removed: FileReadAll - now pure Arth in stdlib/src/io/File.arth
    FileWrite,
    FileWriteStr,
    FileFlush,
    FileSeek,
    FileSize,
    FileExists,
    FileDelete,
    FileCopy,
    FileMove,
    DirCreate,
    DirCreateAll,
    DirDelete,
    DirList,
    DirExists,
    IsDir,
    IsFile,
    // Removed: PathJoin, PathParent, PathFileName, PathExtension - now pure Arth
    PathAbsolute,
    ConsoleReadLine,
    ConsoleWrite,
    ConsoleWriteErr,

    // HTTP operations (4 - actual I/O only)
    // Removed: HttpRequestMethod, HttpRequestPath, HttpRequestHeader, HttpRequestBody,
    //          HttpResponseStatus, HttpResponseBody - now pure Arth struct access
    HttpFetch,
    HttpServe,
    HttpAccept,
    HttpRespond,

    // Collection operations - List (7 core intrinsics)
    // Removed: ListIndexOf, ListContains, ListInsert, ListClear, ListReverse, ListConcat, ListSlice, ListUnique
    ListNew,
    ListPush,
    ListGet,
    ListSet,
    ListLen,
    ListRemove,
    ListSort,

    // Collection operations - Map (7 core intrinsics)
    // Removed: MapContainsValue, MapClear, MapIsEmpty, MapGetOrDefault, MapValues
    MapNew,
    MapPut,
    MapGet,
    MapLen,
    MapContainsKey,
    MapRemove,
    MapKeys,

    // Time operations
    DateTimeNow,
    // Removed: DateTimeFromMillis, DateTimeYear, DateTimeMonth, DateTimeDay,
    //          DateTimeHour, DateTimeMinute, DateTimeSecond, DateTimeDayOfWeek,
    //          DateTimeDayOfYear - now pure Arth in stdlib/src/time/DateTime.arth
    DateTimeParse,
    DateTimeFormat,
    InstantNow,
    InstantElapsed,

    // Numeric operations (BigDecimal, BigInt)
    BigDecimalNew,
    BigDecimalFromInt,
    BigDecimalFromFloat,
    BigDecimalAdd,
    BigDecimalSub,
    BigDecimalMul,
    BigDecimalDiv,
    BigDecimalRem,
    BigDecimalPow,
    // Removed: BigDecimalAbs - now pure Arth in stdlib/src/numeric/BigDecimal.arth
    BigDecimalNegate,
    BigDecimalCompare,
    BigDecimalToString,
    BigDecimalToInt,
    BigDecimalToFloat,
    BigDecimalScale,
    BigDecimalSetScale,
    BigDecimalRound,
    BigIntNew,
    BigIntFromInt,
    BigIntAdd,
    BigIntSub,
    BigIntMul,
    BigIntDiv,
    BigIntRem,
    BigIntPow,
    // Removed: BigIntAbs, BigIntGcd, BigIntModPow - now pure Arth in stdlib/src/numeric/BigInt.arth
    BigIntNegate,
    BigIntCompare,
    BigIntToString,
    BigIntToInt,

    // JSON/encoding operations
    JsonStringify,
    JsonParse,
    StructToJson,
    JsonToStruct,

    // HTML parsing operations (markup package)
    HtmlParse,
    HtmlParseFragment,
    HtmlStringify,
    HtmlStringifyPretty,
    HtmlFree,
    HtmlNodeType,
    HtmlTagName,
    HtmlTextContent,
    HtmlInnerHtml,
    HtmlOuterHtml,
    HtmlGetAttr,
    HtmlHasAttr,
    HtmlAttrNames,
    HtmlParent,
    HtmlChildren,
    HtmlElementChildren,
    HtmlFirstChild,
    HtmlLastChild,
    HtmlNextSibling,
    HtmlPrevSibling,
    HtmlQuerySelector,
    HtmlQuerySelectorAll,
    HtmlGetById,
    HtmlGetByTag,
    HtmlGetByClass,
    HtmlHasClass,

    // Template engine operations (template package)
    TemplateCompile,
    TemplateCompileFile,
    TemplateRender,
    TemplateRegisterPartial,
    TemplateGetPartial,
    TemplateUnregisterPartial,
    TemplateFree,
    TemplateEscapeHtml,
    TemplateUnescapeHtml,

    // Concurrency operations
    TaskSpawn,
    TaskAwait,
    TaskJoin,
    TaskCancel,
    TaskDetach,
    TaskCurrent,
    ChannelCreate,
    ChannelSend,
    ChannelTrySend,
    ChannelClose,
    ChannelRecv,
    ChannelTryRecv,
    ActorCreate,
    ActorSend,
    ActorTrySend,
    ActorStop,

    // Logging operations
    LogEmit,

    // Shared memory operations
    SharedNew,
    SharedStore,
    SharedLoad,

    // Panic/unwinding operations
    Panic,
    SetUnwindHandler,
    ClearUnwindHandler,
    GetPanicMessage,

    // Enum operations
    EnumTag,
    EnumGet,

    // Executor operations (concurrent thread pool)
    ExecutorInit,
    ExecutorThreadCount,
    ExecutorActiveWorkers,
    ExecutorSpawn,
    ExecutorJoin,

    // MPMC Channel operations (C06 - thread-safe channels)
    MpmcChanCreate,
    MpmcChanSend,
    MpmcChanSendBlocking,
    MpmcChanRecv,
    MpmcChanRecvBlocking,
    MpmcChanClose,
    MpmcChanLen,
    MpmcChanIsEmpty,
    MpmcChanIsFull,
    MpmcChanIsClosed,
    MpmcChanCapacity,

    // C07: Executor-integrated MPMC channel operations
    MpmcChanSendWithTask,
    MpmcChanRecvWithTask,
    MpmcChanRecvAndWake,
    MpmcChanPopWaitingSender,
    MpmcChanGetWaitingSenderValue,
    MpmcChanPopWaitingReceiver,
    MpmcChanWaitingSenderCount,
    MpmcChanWaitingReceiverCount,
    MpmcChanGetWokenSender,

    // C08: Blocking receive - send and wake operations
    MpmcChanSendAndWake,
    MpmcChanGetWokenReceiver,

    // C09: Channel select operations
    MpmcChanSelectClear,
    MpmcChanSelectAdd,
    MpmcChanSelectCount,
    MpmcChanTrySelectRecv,
    MpmcChanSelectRecvBlocking,
    MpmcChanSelectRecvWithTask,
    MpmcChanSelectGetReadyIndex,
    MpmcChanSelectGetValue,
    MpmcChanSelectDeregister,
    MpmcChanSelectGetHandle,

    // C11: Actor operations (Actor = Task + Channel)
    ActorSpawn,
    ActorSendBlocking,
    ActorRecv,
    ActorRecvBlocking,
    ActorClose,
    ActorGetTask,
    ActorGetMailbox,
    ActorIsRunning,
    ActorGetState,
    ActorMessageCount,
    ActorMailboxEmpty,
    ActorMailboxLen,
    ActorSetTask,
    ActorMarkStopped,
    ActorMarkFailed,
    ActorIsFailed,

    // WebSocket operations (net.ws)
    WsServe,
    WsAccept,
    WsSendText,
    WsSendBinary,
    WsRecv,
    WsClose,
    WsIsOpen,

    // Server-Sent Events operations (net.sse)
    SseServe,
    SseAccept,
    SseSend,
    // Removed: SseSendData - now pure Arth in stdlib/src/net/sse/Sse.arth
    SseClose,
    SseIsOpen,

    // SQLite database operations (db.sqlite)
    SqliteOpen,
    SqliteClose,
    SqlitePrepare,
    SqliteStep,
    SqliteFinalize,
    SqliteReset,
    SqliteBindInt,
    SqliteBindInt64,
    SqliteBindDouble,
    SqliteBindText,
    SqliteBindBlob,
    SqliteBindNull,
    SqliteColumnInt,
    SqliteColumnInt64,
    SqliteColumnDouble,
    SqliteColumnText,
    SqliteColumnBlob,
    SqliteColumnType,
    SqliteColumnCount,
    SqliteColumnName,
    SqliteIsNull,
    SqliteChanges,
    SqliteLastInsertRowid,
    SqliteErrmsg,
    SqliteBegin,
    SqliteCommit,
    SqliteRollback,
    SqliteSavepoint,
    SqliteReleaseSavepoint,
    SqliteRollbackToSavepoint,

    // PostgreSQL database operations (db.postgres)
    PgConnect,
    PgDisconnect,
    PgStatus,
    PgQuery,
    PgExecute,
    PgPrepare,
    PgExecutePrepared,
    PgRowCount,
    PgColumnCount,
    PgColumnName,
    PgColumnType,
    PgGetValue,
    PgGetInt,
    PgGetInt64,
    PgGetDouble,
    PgGetText,
    PgGetBytes,
    PgGetBool,
    PgIsNull,
    PgAffectedRows,
    PgBegin,
    PgCommit,
    PgRollback,
    PgSavepoint,
    PgReleaseSavepoint,
    PgRollbackToSavepoint,
    PgErrmsg,
    PgEscape,
    PgFreeResult,

    // Async PostgreSQL database operations (db.postgres.async)
    PgConnectAsync,
    PgDisconnectAsync,
    PgStatusAsync,
    PgQueryAsync,
    PgExecuteAsync,
    PgPrepareAsync,
    PgExecutePreparedAsync,
    PgIsReady,
    PgGetAsyncResult,
    PgCancelAsync,
    PgBeginAsync,
    PgCommitAsync,
    PgRollbackAsync,
    // SQLite pool operations
    SqlitePoolCreate,
    SqlitePoolClose,
    SqlitePoolAcquire,
    SqlitePoolRelease,
    SqlitePoolStats,
    // PostgreSQL pool operations
    PgPoolCreate,
    PgPoolClose,
    PgPoolAcquire,
    PgPoolRelease,
    PgPoolStats,
    // SQLite transaction helpers
    SqliteTxScopeBegin,
    SqliteTxScopeEnd,
    SqliteTxDepth,
    SqliteTxActive,
    // PostgreSQL transaction helpers
    PgTxScopeBegin,
    PgTxScopeEnd,
    PgTxDepth,
    PgTxActive,

    // Cryptographic operations - Secure Memory
    CryptoSecureAlloc,
    CryptoSecureFree,
    CryptoSecurePtr,
    CryptoSecureLen,
    CryptoSecureWrite,
    CryptoSecureRead,
    CryptoSecureZero,
    CryptoSecureCompare,

    // Cryptographic operations - Hashing
    CryptoHash,
    CryptoHasherNew,
    CryptoHasherUpdate,
    CryptoHasherFinalize,

    // Cryptographic operations - Nonce/Salt
    CryptoNonceRandom,
    CryptoSaltRandom,

    // Cryptographic operations - Signatures
    CryptoSignatureSign,
    CryptoSignatureSignHash,
    CryptoSignatureVerify,
    CryptoSignatureVerifyHash,

    // Cryptographic operations - Key Exchange
    CryptoSharedSecretExchange,

    // Cryptographic operations - Ciphertext
    CryptoCiphertextFromParts,

    // Cryptographic operations - Encoding
    CryptoEncodingToHex,
    CryptoEncodingFromHex,
    CryptoEncodingToBase64,
    CryptoEncodingFromBase64,
    CryptoEncodingToBase64Url,
    CryptoEncodingFromBase64Url,
}

/// Metadata for an intrinsic function.
#[derive(Clone, Debug)]
pub struct Intrinsic {
    /// The intrinsic name (e.g., "math.sqrt")
    pub name: &'static str,
    /// The VM operation this intrinsic maps to
    pub vm_op: VmOp,
    /// Number of arguments the intrinsic takes
    pub arg_count: usize,
    /// Whether the intrinsic returns a value (false for void functions)
    pub returns: bool,
    /// Classification of the intrinsic by host capability domain
    pub kind: IntrinsicKind,
    /// Native symbol name for LLVM/native compilation (e.g., "arth_rt_file_open").
    /// None for CoreVm intrinsics that use LLVM intrinsics directly (e.g., math.sqrt -> llvm.sqrt).
    /// When set, the native backend emits `call @<native_symbol>` instead of host call opcodes.
    pub native_symbol: Option<&'static str>,
}

/// Static registry of all intrinsics.
///
/// This array is the single source of truth for intrinsic -> VM op mappings.
/// When adding new intrinsics, add them here.
pub static INTRINSICS: &[Intrinsic] = &[
    // ─────────────────────────────────────────────────────────────────────────
    // Math operations (use LLVM intrinsics in native mode)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "math.sqrt",
        vm_op: VmOp::MathSqrt,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.sqrt
    },
    Intrinsic {
        name: "math.pow",
        vm_op: VmOp::MathPow,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.pow
    },
    Intrinsic {
        name: "math.sin",
        vm_op: VmOp::MathSin,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.sin
    },
    Intrinsic {
        name: "math.cos",
        vm_op: VmOp::MathCos,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.cos
    },
    Intrinsic {
        name: "math.tan",
        vm_op: VmOp::MathTan,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses tan from libm
    },
    Intrinsic {
        name: "math.floor",
        vm_op: VmOp::MathFloor,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.floor
    },
    Intrinsic {
        name: "math.ceil",
        vm_op: VmOp::MathCeil,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.ceil
    },
    Intrinsic {
        name: "math.round",
        vm_op: VmOp::MathRound,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None, // Uses llvm.round
    },
    // Removed 9 math helpers - now pure Arth in stdlib/src/math/Math.arth:
    // math.round_n, math.min_f, math.max_f, math.clamp_f, math.abs_f,
    // math.min_i, math.max_i, math.clamp_i, math.abs_i
    // ─────────────────────────────────────────────────────────────────────────
    // I/O operations - Console
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "io.print",
        vm_op: VmOp::Print,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_console_write"),
    },
    Intrinsic {
        name: "io.println",
        vm_op: VmOp::PrintLn,
        arg_count: 0,
        returns: false,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_console_write"), // Writes newline
    },
    Intrinsic {
        name: "io.console_read_line",
        vm_op: VmOp::ConsoleReadLine,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_console_read_line"),
    },
    Intrinsic {
        name: "io.console_write",
        vm_op: VmOp::ConsoleWrite,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_console_write"),
    },
    Intrinsic {
        name: "io.console_write_err",
        vm_op: VmOp::ConsoleWriteErr,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_console_write_err"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // I/O operations - File
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "io.file_open",
        vm_op: VmOp::FileOpen,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_open"),
    },
    Intrinsic {
        name: "io.file_close",
        vm_op: VmOp::FileClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_close"),
    },
    Intrinsic {
        name: "io.file_read",
        vm_op: VmOp::FileRead,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_read"),
    },
    // Removed: io.file_read_all - now pure Arth in stdlib/src/io/File.arth
    Intrinsic {
        name: "io.file_write",
        vm_op: VmOp::FileWrite,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_write"),
    },
    Intrinsic {
        name: "io.file_write_str",
        vm_op: VmOp::FileWriteStr,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_write"),
    },
    Intrinsic {
        name: "io.file_flush",
        vm_op: VmOp::FileFlush,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_flush"),
    },
    Intrinsic {
        name: "io.file_seek",
        vm_op: VmOp::FileSeek,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_seek"),
    },
    Intrinsic {
        name: "io.file_size",
        vm_op: VmOp::FileSize,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_size"),
    },
    Intrinsic {
        name: "io.file_exists",
        vm_op: VmOp::FileExists,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_exists"),
    },
    Intrinsic {
        name: "io.file_delete",
        vm_op: VmOp::FileDelete,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_delete"),
    },
    Intrinsic {
        name: "io.file_copy",
        vm_op: VmOp::FileCopy,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_copy"),
    },
    Intrinsic {
        name: "io.file_move",
        vm_op: VmOp::FileMove,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_file_move"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // I/O operations - Directory
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "io.dir_create",
        vm_op: VmOp::DirCreate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_dir_create"),
    },
    Intrinsic {
        name: "io.dir_create_all",
        vm_op: VmOp::DirCreateAll,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_dir_create_all"),
    },
    Intrinsic {
        name: "io.dir_delete",
        vm_op: VmOp::DirDelete,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_dir_delete"),
    },
    Intrinsic {
        name: "io.dir_list",
        vm_op: VmOp::DirList,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_dir_list"),
    },
    Intrinsic {
        name: "io.dir_exists",
        vm_op: VmOp::DirExists,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_dir_exists"),
    },
    Intrinsic {
        name: "io.is_dir",
        vm_op: VmOp::IsDir,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_is_dir"),
    },
    Intrinsic {
        name: "io.is_file",
        vm_op: VmOp::IsFile,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_is_file"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // I/O operations - Path
    // Removed: path_join, path_parent, path_file_name, path_extension
    // These are now pure Arth string operations in stdlib/src/io/Path.arth
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "io.path_absolute",
        vm_op: VmOp::PathAbsolute,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: Some("arth_rt_path_absolute"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // HTTP operations (4 intrinsics - actual I/O only)
    // Request/Response helpers are pure Arth in stdlib/src/net/http/Http.arth
    //
    // Native Backend Notes:
    // These intrinsics work with high-level Arth objects (Request, Response).
    // arth-rt provides lower-level C FFI functions:
    //   - arth_rt_http_connect, arth_rt_http_request, arth_rt_http_close
    //   - arth_rt_http_response_* for accessing response data
    // For native compilation, the LLVM backend needs glue code to:
    //   1. Extract URL, method, headers, body from Arth Request object
    //   2. Call arth_rt_http_connect + arth_rt_http_request
    //   3. Build Arth Response object from arth_rt_http_response_* calls
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "http.fetch",
        vm_op: VmOp::HttpFetch,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // Requires LLVM glue code - see notes above
    },
    Intrinsic {
        name: "http.serve",
        vm_op: VmOp::HttpServe,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // Server not yet implemented in arth-rt
    },
    Intrinsic {
        name: "http.accept",
        vm_op: VmOp::HttpAccept,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // Server not yet implemented in arth-rt
    },
    Intrinsic {
        name: "http.respond",
        vm_op: VmOp::HttpRespond,
        arg_count: 4,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // Server not yet implemented in arth-rt
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Collection operations - List (7 intrinsics)
    // Removed: index_of, contains, insert, clear, reverse, concat, slice, unique
    // These are now pure Arth code in stdlib/src/arth/array.arth
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "list.new",
        vm_op: VmOp::ListNew,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.push",
        vm_op: VmOp::ListPush,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.get",
        vm_op: VmOp::ListGet,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.set",
        vm_op: VmOp::ListSet,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.len",
        vm_op: VmOp::ListLen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.remove",
        vm_op: VmOp::ListRemove,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "list.sort",
        vm_op: VmOp::ListSort,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Collection operations - Map (7 intrinsics)
    // Removed: contains_value, clear, is_empty, get_or_default, values
    // These are now pure Arth code in stdlib/src/arth/map.arth
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "map.new",
        vm_op: VmOp::MapNew,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.put",
        vm_op: VmOp::MapPut,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.get",
        vm_op: VmOp::MapGet,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.len",
        vm_op: VmOp::MapLen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.contains_key",
        vm_op: VmOp::MapContainsKey,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.remove",
        vm_op: VmOp::MapRemove,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "map.keys",
        vm_op: VmOp::MapKeys,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Time operations - DateTime
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "time.now",
        vm_op: VmOp::DateTimeNow,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::HostTime,
        native_symbol: Some("arth_rt_time_now"),
    },
    // Removed: time.from_millis, time.year, time.month, time.day, time.hour,
    //          time.minute, time.second, time.day_of_week, time.day_of_year
    //          - now pure Arth in stdlib/src/time/DateTime.arth
    Intrinsic {
        name: "time.parse",
        vm_op: VmOp::DateTimeParse,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostTime,
        native_symbol: Some("arth_rt_time_parse"),
    },
    Intrinsic {
        name: "time.format",
        vm_op: VmOp::DateTimeFormat,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostTime,
        native_symbol: Some("arth_rt_time_format"),
    },
    // Duration operations removed - all Duration ops are now pure Arth code
    // ─────────────────────────────────────────────────────────────────────────
    // Time operations - Instant
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "instant.now",
        vm_op: VmOp::InstantNow,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::HostTime,
        native_symbol: Some("arth_rt_instant_now"),
    },
    Intrinsic {
        name: "instant.elapsed",
        vm_op: VmOp::InstantElapsed,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostTime,
        native_symbol: Some("arth_rt_instant_elapsed"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Numeric operations - BigDecimal
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "bigdecimal.new",
        vm_op: VmOp::BigDecimalNew,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.from_int",
        vm_op: VmOp::BigDecimalFromInt,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.from_float",
        vm_op: VmOp::BigDecimalFromFloat,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.add",
        vm_op: VmOp::BigDecimalAdd,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.sub",
        vm_op: VmOp::BigDecimalSub,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.mul",
        vm_op: VmOp::BigDecimalMul,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.div",
        vm_op: VmOp::BigDecimalDiv,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.rem",
        vm_op: VmOp::BigDecimalRem,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.pow",
        vm_op: VmOp::BigDecimalPow,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // Removed: bigdecimal.abs - now pure Arth in stdlib/src/numeric/BigDecimal.arth
    Intrinsic {
        name: "bigdecimal.negate",
        vm_op: VmOp::BigDecimalNegate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.compare",
        vm_op: VmOp::BigDecimalCompare,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.to_string",
        vm_op: VmOp::BigDecimalToString,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.to_int",
        vm_op: VmOp::BigDecimalToInt,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.to_float",
        vm_op: VmOp::BigDecimalToFloat,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.scale",
        vm_op: VmOp::BigDecimalScale,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.set_scale",
        vm_op: VmOp::BigDecimalSetScale,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigdecimal.round",
        vm_op: VmOp::BigDecimalRound,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Numeric operations - BigInt
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "bigint.new",
        vm_op: VmOp::BigIntNew,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.from_int",
        vm_op: VmOp::BigIntFromInt,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.add",
        vm_op: VmOp::BigIntAdd,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.sub",
        vm_op: VmOp::BigIntSub,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.mul",
        vm_op: VmOp::BigIntMul,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.div",
        vm_op: VmOp::BigIntDiv,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.rem",
        vm_op: VmOp::BigIntRem,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.pow",
        vm_op: VmOp::BigIntPow,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // Removed: bigint.abs, bigint.gcd, bigint.mod_pow - now pure Arth in stdlib/src/numeric/BigInt.arth
    Intrinsic {
        name: "bigint.negate",
        vm_op: VmOp::BigIntNegate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.compare",
        vm_op: VmOp::BigIntCompare,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.to_string",
        vm_op: VmOp::BigIntToString,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "bigint.to_int",
        vm_op: VmOp::BigIntToInt,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // JSON/encoding operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "json.stringify",
        vm_op: VmOp::JsonStringify,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "json.parse",
        vm_op: VmOp::JsonParse,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "json.struct_to_json",
        vm_op: VmOp::StructToJson,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "json.json_to_struct",
        vm_op: VmOp::JsonToStruct,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // HTML parsing operations (markup package)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "html.parse",
        vm_op: VmOp::HtmlParse,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.parse_fragment",
        vm_op: VmOp::HtmlParseFragment,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.stringify",
        vm_op: VmOp::HtmlStringify,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.stringify_pretty",
        vm_op: VmOp::HtmlStringifyPretty,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.free",
        vm_op: VmOp::HtmlFree,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.node_type",
        vm_op: VmOp::HtmlNodeType,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.tag_name",
        vm_op: VmOp::HtmlTagName,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.text_content",
        vm_op: VmOp::HtmlTextContent,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.inner_html",
        vm_op: VmOp::HtmlInnerHtml,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.outer_html",
        vm_op: VmOp::HtmlOuterHtml,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.get_attr",
        vm_op: VmOp::HtmlGetAttr,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.has_attr",
        vm_op: VmOp::HtmlHasAttr,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.attr_names",
        vm_op: VmOp::HtmlAttrNames,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.parent",
        vm_op: VmOp::HtmlParent,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.children",
        vm_op: VmOp::HtmlChildren,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.element_children",
        vm_op: VmOp::HtmlElementChildren,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.first_child",
        vm_op: VmOp::HtmlFirstChild,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.last_child",
        vm_op: VmOp::HtmlLastChild,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.next_sibling",
        vm_op: VmOp::HtmlNextSibling,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.prev_sibling",
        vm_op: VmOp::HtmlPrevSibling,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.query_selector",
        vm_op: VmOp::HtmlQuerySelector,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.query_selector_all",
        vm_op: VmOp::HtmlQuerySelectorAll,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.get_by_id",
        vm_op: VmOp::HtmlGetById,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.get_by_tag",
        vm_op: VmOp::HtmlGetByTag,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.get_by_class",
        vm_op: VmOp::HtmlGetByClass,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "html.has_class",
        vm_op: VmOp::HtmlHasClass,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Template engine operations (template package)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "template.compile",
        vm_op: VmOp::TemplateCompile,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.compile_file",
        vm_op: VmOp::TemplateCompileFile,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostIo,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.render",
        vm_op: VmOp::TemplateRender,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.register_partial",
        vm_op: VmOp::TemplateRegisterPartial,
        arg_count: 2,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.get_partial",
        vm_op: VmOp::TemplateGetPartial,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.unregister_partial",
        vm_op: VmOp::TemplateUnregisterPartial,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.free",
        vm_op: VmOp::TemplateFree,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.escape_html",
        vm_op: VmOp::TemplateEscapeHtml,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "template.unescape_html",
        vm_op: VmOp::TemplateUnescapeHtml,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Concurrency operations - Task
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "task.spawn",
        vm_op: VmOp::TaskSpawn,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "task.await",
        vm_op: VmOp::TaskAwait,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "task.join",
        vm_op: VmOp::TaskJoin,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "task.cancel",
        vm_op: VmOp::TaskCancel,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "task.detach",
        vm_op: VmOp::TaskDetach,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "task.current",
        vm_op: VmOp::TaskCurrent,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Concurrency operations - Channel
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "channel.create",
        vm_op: VmOp::ChannelCreate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "channel.send",
        vm_op: VmOp::ChannelSend,
        arg_count: 2,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "channel.try_send",
        vm_op: VmOp::ChannelTrySend,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "channel.close",
        vm_op: VmOp::ChannelClose,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "channel.recv",
        vm_op: VmOp::ChannelRecv,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "channel.try_recv",
        vm_op: VmOp::ChannelTryRecv,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // C11: Actor operations (Actor = Task + Channel)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "actor.create",
        vm_op: VmOp::ActorCreate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.spawn",
        vm_op: VmOp::ActorSpawn,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.send",
        vm_op: VmOp::ActorSend,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.send_blocking",
        vm_op: VmOp::ActorSendBlocking,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.try_send",
        vm_op: VmOp::ActorTrySend,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.recv",
        vm_op: VmOp::ActorRecv,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.recv_blocking",
        vm_op: VmOp::ActorRecvBlocking,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.close",
        vm_op: VmOp::ActorClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.stop",
        vm_op: VmOp::ActorStop,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.get_task",
        vm_op: VmOp::ActorGetTask,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.get_mailbox",
        vm_op: VmOp::ActorGetMailbox,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.is_running",
        vm_op: VmOp::ActorIsRunning,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.get_state",
        vm_op: VmOp::ActorGetState,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.message_count",
        vm_op: VmOp::ActorMessageCount,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.mailbox_empty",
        vm_op: VmOp::ActorMailboxEmpty,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.mailbox_len",
        vm_op: VmOp::ActorMailboxLen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.set_task",
        vm_op: VmOp::ActorSetTask,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.mark_stopped",
        vm_op: VmOp::ActorMarkStopped,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.mark_failed",
        vm_op: VmOp::ActorMarkFailed,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "actor.is_failed",
        vm_op: VmOp::ActorIsFailed,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Logging operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "log.emit",
        vm_op: VmOp::LogEmit,
        arg_count: 4,
        returns: false,
        kind: IntrinsicKind::HostIo,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Shared memory operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "shared.new",
        vm_op: VmOp::SharedNew,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "shared.store",
        vm_op: VmOp::SharedStore,
        arg_count: 2,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "shared.load",
        vm_op: VmOp::SharedLoad,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Panic/unwinding operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "panic.panic",
        vm_op: VmOp::Panic,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "panic.set_unwind_handler",
        vm_op: VmOp::SetUnwindHandler,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "panic.clear_unwind_handler",
        vm_op: VmOp::ClearUnwindHandler,
        arg_count: 0,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "panic.get_message",
        vm_op: VmOp::GetPanicMessage,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Enum operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "enum.tag",
        vm_op: VmOp::EnumTag,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "enum.get",
        vm_op: VmOp::EnumGet,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Executor operations (concurrent thread pool)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "executor.init",
        vm_op: VmOp::ExecutorInit,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "executor.thread_count",
        vm_op: VmOp::ExecutorThreadCount,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "executor.active_workers",
        vm_op: VmOp::ExecutorActiveWorkers,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "executor.spawn",
        vm_op: VmOp::ExecutorSpawn,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "executor.join",
        vm_op: VmOp::ExecutorJoin,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // MPMC Channel operations (C06 - thread-safe channels)
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "mpmc.create",
        vm_op: VmOp::MpmcChanCreate,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.send",
        vm_op: VmOp::MpmcChanSend,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.send_blocking",
        vm_op: VmOp::MpmcChanSendBlocking,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.recv",
        vm_op: VmOp::MpmcChanRecv,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.recv_blocking",
        vm_op: VmOp::MpmcChanRecvBlocking,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.close",
        vm_op: VmOp::MpmcChanClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.len",
        vm_op: VmOp::MpmcChanLen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.is_empty",
        vm_op: VmOp::MpmcChanIsEmpty,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.is_full",
        vm_op: VmOp::MpmcChanIsFull,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.is_closed",
        vm_op: VmOp::MpmcChanIsClosed,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.capacity",
        vm_op: VmOp::MpmcChanCapacity,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // C07: Executor-integrated MPMC channel operations
    Intrinsic {
        name: "mpmc.send_with_task",
        vm_op: VmOp::MpmcChanSendWithTask,
        arg_count: 3, // handle, value, task_id
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.recv_with_task",
        vm_op: VmOp::MpmcChanRecvWithTask,
        arg_count: 2, // handle, task_id
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.recv_and_wake",
        vm_op: VmOp::MpmcChanRecvAndWake,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.pop_waiting_sender",
        vm_op: VmOp::MpmcChanPopWaitingSender,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.get_waiting_sender_value",
        vm_op: VmOp::MpmcChanGetWaitingSenderValue,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.pop_waiting_receiver",
        vm_op: VmOp::MpmcChanPopWaitingReceiver,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.waiting_sender_count",
        vm_op: VmOp::MpmcChanWaitingSenderCount,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.waiting_receiver_count",
        vm_op: VmOp::MpmcChanWaitingReceiverCount,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.get_woken_sender",
        vm_op: VmOp::MpmcChanGetWokenSender,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // C08: Blocking receive - send and wake operations
    Intrinsic {
        name: "mpmc.send_and_wake",
        vm_op: VmOp::MpmcChanSendAndWake,
        arg_count: 2, // handle, value
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.get_woken_receiver",
        vm_op: VmOp::MpmcChanGetWokenReceiver,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // C09: Channel Select operations
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "mpmc.select_clear",
        vm_op: VmOp::MpmcChanSelectClear,
        arg_count: 0,
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_add",
        vm_op: VmOp::MpmcChanSelectAdd,
        arg_count: 1, // handle
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_count",
        vm_op: VmOp::MpmcChanSelectCount,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.try_select_recv",
        vm_op: VmOp::MpmcChanTrySelectRecv,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_recv_blocking",
        vm_op: VmOp::MpmcChanSelectRecvBlocking,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_recv_with_task",
        vm_op: VmOp::MpmcChanSelectRecvWithTask,
        arg_count: 1, // task_id
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_get_ready_index",
        vm_op: VmOp::MpmcChanSelectGetReadyIndex,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_get_value",
        vm_op: VmOp::MpmcChanSelectGetValue,
        arg_count: 0,
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_deregister",
        vm_op: VmOp::MpmcChanSelectDeregister,
        arg_count: 2, // task_id, except_index
        returns: false,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    Intrinsic {
        name: "mpmc.select_get_handle",
        vm_op: VmOp::MpmcChanSelectGetHandle,
        arg_count: 1, // index
        returns: true,
        kind: IntrinsicKind::CoreVm,
        native_symbol: None,
    },
    // ─────────────────────────────────────────────────────────────────────────
    // WebSocket operations (net.ws)
    //
    // WebSocket support is implemented at the VM level using synchronous TCP
    // sockets from arth-rt. The VM handles WebSocket protocol (RFC 6455):
    // handshake, frame encoding/decoding, and message dispatch. These are
    // higher-level abstractions not directly mapped to single C FFI calls.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "ws.serve",
        vm_op: VmOp::WsServe,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level WebSocket server using arth_rt sockets
    },
    Intrinsic {
        name: "ws.accept",
        vm_op: VmOp::WsAccept,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level WebSocket accept using arth_rt sockets
    },
    Intrinsic {
        name: "ws.send_text",
        vm_op: VmOp::WsSendText,
        arg_count: 2,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM encodes WebSocket frame and sends via socket
    },
    Intrinsic {
        name: "ws.send_binary",
        vm_op: VmOp::WsSendBinary,
        arg_count: 2,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM encodes WebSocket frame and sends via socket
    },
    Intrinsic {
        name: "ws.recv",
        vm_op: VmOp::WsRecv,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM reads and decodes WebSocket frames
    },
    Intrinsic {
        name: "ws.close",
        vm_op: VmOp::WsClose,
        arg_count: 3,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM sends close frame and closes socket
    },
    Intrinsic {
        name: "ws.is_open",
        vm_op: VmOp::WsIsOpen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level connection state tracking
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Server-Sent Events operations (net.sse)
    //
    // SSE support is implemented at the VM level using synchronous TCP sockets
    // from arth-rt. The VM handles HTTP response formatting and SSE protocol
    // (text/event-stream content type, data/event/id fields).
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "sse.serve",
        vm_op: VmOp::SseServe,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level SSE server using arth_rt sockets
    },
    Intrinsic {
        name: "sse.accept",
        vm_op: VmOp::SseAccept,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level SSE accept using arth_rt sockets
    },
    Intrinsic {
        name: "sse.send",
        vm_op: VmOp::SseSend,
        arg_count: 4,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM formats and sends SSE event via socket
    },
    // Removed: sse.send_data - now pure Arth in stdlib/src/net/sse/Sse.arth
    Intrinsic {
        name: "sse.close",
        vm_op: VmOp::SseClose,
        arg_count: 1,
        returns: false,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM closes socket
    },
    Intrinsic {
        name: "sse.is_open",
        vm_op: VmOp::SseIsOpen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostNet,
        native_symbol: None, // VM-level connection state tracking
    },
    // ─────────────────────────────────────────────────────────────────────────
    // SQLite database operations (db.sqlite)
    // ─────────────────────────────────────────────────────────────────────────
    // Connection management
    Intrinsic {
        name: "db.sqlite.open",
        vm_op: VmOp::SqliteOpen,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_open"),
    },
    Intrinsic {
        name: "db.sqlite.close",
        vm_op: VmOp::SqliteClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_close"),
    },
    // Statement management
    Intrinsic {
        name: "db.sqlite.prepare",
        vm_op: VmOp::SqlitePrepare,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_prepare"),
    },
    Intrinsic {
        name: "db.sqlite.step",
        vm_op: VmOp::SqliteStep,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_step"),
    },
    Intrinsic {
        name: "db.sqlite.finalize",
        vm_op: VmOp::SqliteFinalize,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_finalize"),
    },
    Intrinsic {
        name: "db.sqlite.reset",
        vm_op: VmOp::SqliteReset,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_reset"),
    },
    // Parameter binding
    Intrinsic {
        name: "db.sqlite.bind_int",
        vm_op: VmOp::SqliteBindInt,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_int"),
    },
    Intrinsic {
        name: "db.sqlite.bind_int64",
        vm_op: VmOp::SqliteBindInt64,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_int64"),
    },
    Intrinsic {
        name: "db.sqlite.bind_double",
        vm_op: VmOp::SqliteBindDouble,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_double"),
    },
    Intrinsic {
        name: "db.sqlite.bind_text",
        vm_op: VmOp::SqliteBindText,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_text"),
    },
    Intrinsic {
        name: "db.sqlite.bind_blob",
        vm_op: VmOp::SqliteBindBlob,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_blob"),
    },
    Intrinsic {
        name: "db.sqlite.bind_null",
        vm_op: VmOp::SqliteBindNull,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_bind_null"),
    },
    // Column access
    Intrinsic {
        name: "db.sqlite.column_int",
        vm_op: VmOp::SqliteColumnInt,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_int"),
    },
    Intrinsic {
        name: "db.sqlite.column_int64",
        vm_op: VmOp::SqliteColumnInt64,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_int64"),
    },
    Intrinsic {
        name: "db.sqlite.column_double",
        vm_op: VmOp::SqliteColumnDouble,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_double"),
    },
    Intrinsic {
        name: "db.sqlite.column_text",
        vm_op: VmOp::SqliteColumnText,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_text"),
    },
    Intrinsic {
        name: "db.sqlite.column_blob",
        vm_op: VmOp::SqliteColumnBlob,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_blob"),
    },
    Intrinsic {
        name: "db.sqlite.column_type",
        vm_op: VmOp::SqliteColumnType,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_type"),
    },
    Intrinsic {
        name: "db.sqlite.column_count",
        vm_op: VmOp::SqliteColumnCount,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_count"),
    },
    Intrinsic {
        name: "db.sqlite.column_name",
        vm_op: VmOp::SqliteColumnName,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_column_name"),
    },
    Intrinsic {
        name: "db.sqlite.is_null",
        vm_op: VmOp::SqliteIsNull,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_is_null"),
    },
    // Utility functions
    Intrinsic {
        name: "db.sqlite.changes",
        vm_op: VmOp::SqliteChanges,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_changes"),
    },
    Intrinsic {
        name: "db.sqlite.last_insert_rowid",
        vm_op: VmOp::SqliteLastInsertRowid,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_last_insert_rowid"),
    },
    Intrinsic {
        name: "db.sqlite.errmsg",
        vm_op: VmOp::SqliteErrmsg,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_errmsg"),
    },
    // Transaction management
    Intrinsic {
        name: "db.sqlite.begin",
        vm_op: VmOp::SqliteBegin,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_begin"),
    },
    Intrinsic {
        name: "db.sqlite.commit",
        vm_op: VmOp::SqliteCommit,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_commit"),
    },
    Intrinsic {
        name: "db.sqlite.rollback",
        vm_op: VmOp::SqliteRollback,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_rollback"),
    },
    Intrinsic {
        name: "db.sqlite.savepoint",
        vm_op: VmOp::SqliteSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_savepoint"),
    },
    Intrinsic {
        name: "db.sqlite.release_savepoint",
        vm_op: VmOp::SqliteReleaseSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_release_savepoint"),
    },
    Intrinsic {
        name: "db.sqlite.rollback_to_savepoint",
        vm_op: VmOp::SqliteRollbackToSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_sqlite_rollback_to_savepoint"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // PostgreSQL database operations (db.postgres)
    // ─────────────────────────────────────────────────────────────────────────
    // Connection management
    Intrinsic {
        name: "db.postgres.connect",
        vm_op: VmOp::PgConnect,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_connect"),
    },
    Intrinsic {
        name: "db.postgres.disconnect",
        vm_op: VmOp::PgDisconnect,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_finish"),
    },
    Intrinsic {
        name: "db.postgres.status",
        vm_op: VmOp::PgStatus,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_status"),
    },
    // Query execution
    Intrinsic {
        name: "db.postgres.query",
        vm_op: VmOp::PgQuery,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_exec"),
    },
    Intrinsic {
        name: "db.postgres.execute",
        vm_op: VmOp::PgExecute,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_exec"),
    },
    Intrinsic {
        name: "db.postgres.prepare",
        vm_op: VmOp::PgPrepare,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_prepare"),
    },
    Intrinsic {
        name: "db.postgres.execute_prepared",
        vm_op: VmOp::PgExecutePrepared,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_exec_prepared"),
    },
    // Result access
    Intrinsic {
        name: "db.postgres.row_count",
        vm_op: VmOp::PgRowCount,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_ntuples"),
    },
    Intrinsic {
        name: "db.postgres.column_count",
        vm_op: VmOp::PgColumnCount,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_nfields"),
    },
    Intrinsic {
        name: "db.postgres.column_name",
        vm_op: VmOp::PgColumnName,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_fname"),
    },
    Intrinsic {
        name: "db.postgres.column_type",
        vm_op: VmOp::PgColumnType,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_ftype"),
    },
    Intrinsic {
        name: "db.postgres.get_value",
        vm_op: VmOp::PgGetValue,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_getvalue"),
    },
    Intrinsic {
        name: "db.postgres.get_int",
        vm_op: VmOp::PgGetInt,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires text parsing, handled by VM
    },
    Intrinsic {
        name: "db.postgres.get_int64",
        vm_op: VmOp::PgGetInt64,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires text parsing, handled by VM
    },
    Intrinsic {
        name: "db.postgres.get_double",
        vm_op: VmOp::PgGetDouble,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires text parsing, handled by VM
    },
    Intrinsic {
        name: "db.postgres.get_text",
        vm_op: VmOp::PgGetText,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_getvalue"),
    },
    Intrinsic {
        name: "db.postgres.get_bytes",
        vm_op: VmOp::PgGetBytes,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires bytea decoding, handled by VM
    },
    Intrinsic {
        name: "db.postgres.get_bool",
        vm_op: VmOp::PgGetBool,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires text parsing, handled by VM
    },
    Intrinsic {
        name: "db.postgres.is_null",
        vm_op: VmOp::PgIsNull,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_getisnull"),
    },
    Intrinsic {
        name: "db.postgres.affected_rows",
        vm_op: VmOp::PgAffectedRows,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_cmd_tuples"),
    },
    // Transaction management
    Intrinsic {
        name: "db.postgres.begin",
        vm_op: VmOp::PgBegin,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_begin"),
    },
    Intrinsic {
        name: "db.postgres.commit",
        vm_op: VmOp::PgCommit,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_commit"),
    },
    Intrinsic {
        name: "db.postgres.rollback",
        vm_op: VmOp::PgRollback,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_rollback"),
    },
    Intrinsic {
        name: "db.postgres.savepoint",
        vm_op: VmOp::PgSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires SQL formatting, handled by VM
    },
    Intrinsic {
        name: "db.postgres.release_savepoint",
        vm_op: VmOp::PgReleaseSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires SQL formatting, handled by VM
    },
    Intrinsic {
        name: "db.postgres.rollback_to_savepoint",
        vm_op: VmOp::PgRollbackToSavepoint,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Requires SQL formatting, handled by VM
    },
    // Utility
    Intrinsic {
        name: "db.postgres.errmsg",
        vm_op: VmOp::PgErrmsg,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_error_message"),
    },
    Intrinsic {
        name: "db.postgres.escape",
        vm_op: VmOp::PgEscape,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // String manipulation, handled by VM
    },
    Intrinsic {
        name: "db.postgres.free_result",
        vm_op: VmOp::PgFreeResult,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_clear"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Async PostgreSQL database operations
    //
    // These use libpq's non-blocking/async API. Connection and status functions
    // share the same C FFI as sync operations. Query functions use PQsend*
    // variants which return immediately, with results fetched via get_result.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "db.postgres.async.connect",
        vm_op: VmOp::PgConnectAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_connect"), // Same connect, caller sets non-blocking
    },
    Intrinsic {
        name: "db.postgres.async.disconnect",
        vm_op: VmOp::PgDisconnectAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_finish"),
    },
    Intrinsic {
        name: "db.postgres.async.status",
        vm_op: VmOp::PgStatusAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_status"),
    },
    Intrinsic {
        name: "db.postgres.async.query",
        vm_op: VmOp::PgQueryAsync,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_send_query"), // Non-blocking send
    },
    Intrinsic {
        name: "db.postgres.async.execute",
        vm_op: VmOp::PgExecuteAsync,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_send_query"), // Non-blocking send
    },
    Intrinsic {
        name: "db.postgres.async.prepare",
        vm_op: VmOp::PgPrepareAsync,
        arg_count: 3,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_send_prepare"), // Non-blocking prepare
    },
    Intrinsic {
        name: "db.postgres.async.execute_prepared",
        vm_op: VmOp::PgExecutePreparedAsync,
        arg_count: 2,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_send_query_prepared"), // Non-blocking exec
    },
    Intrinsic {
        name: "db.postgres.async.is_ready",
        vm_op: VmOp::PgIsReady,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Uses is_busy + consume_input, needs glue code
    },
    Intrinsic {
        name: "db.postgres.async.get_result",
        vm_op: VmOp::PgGetAsyncResult,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: Some("arth_rt_pg_get_result"),
    },
    Intrinsic {
        name: "db.postgres.async.cancel",
        vm_op: VmOp::PgCancelAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Uses PQcancel, not yet in arth-rt
    },
    Intrinsic {
        name: "db.postgres.async.begin",
        vm_op: VmOp::PgBeginAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Constructs "BEGIN" SQL, handled by VM
    },
    Intrinsic {
        name: "db.postgres.async.commit",
        vm_op: VmOp::PgCommitAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Constructs "COMMIT" SQL, handled by VM
    },
    Intrinsic {
        name: "db.postgres.async.rollback",
        vm_op: VmOp::PgRollbackAsync,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // Constructs "ROLLBACK" SQL, handled by VM
    },
    // ─────────────────────────────────────────────────────────────────────────
    // SQLite Connection Pool Operations
    //
    // Pool operations are VM-level abstractions that manage multiple connections.
    // They maintain internal state (available/in-use queues, waiters, timeouts)
    // and call individual arth_rt_sqlite_* functions for connection lifecycle.
    // No direct C FFI equivalent - pools are a runtime/VM concept.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "db.sqlite.pool.create",
        vm_op: VmOp::SqlitePoolCreate,
        arg_count: 7, // conn_str, min, max, acquire_timeout_ms, idle_timeout_ms, max_lifetime_ms, test_on_acquire
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.sqlite.pool.close",
        vm_op: VmOp::SqlitePoolClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.sqlite.pool.acquire",
        vm_op: VmOp::SqlitePoolAcquire,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.sqlite.pool.release",
        vm_op: VmOp::SqlitePoolRelease,
        arg_count: 2, // pool_handle, conn_handle
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.sqlite.pool.stats",
        vm_op: VmOp::SqlitePoolStats,
        arg_count: 1,
        returns: true, // Returns 4 values: available, in_use, total, waiters
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    // ─────────────────────────────────────────────────────────────────────────
    // PostgreSQL Connection Pool Operations
    //
    // Same as SQLite pools - VM-level abstractions managing connection lifecycle.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "db.postgres.pool.create",
        vm_op: VmOp::PgPoolCreate,
        arg_count: 7, // conn_str, min, max, acquire_timeout_ms, idle_timeout_ms, max_lifetime_ms, test_on_acquire
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.postgres.pool.close",
        vm_op: VmOp::PgPoolClose,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.postgres.pool.acquire",
        vm_op: VmOp::PgPoolAcquire,
        arg_count: 1,
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.postgres.pool.release",
        vm_op: VmOp::PgPoolRelease,
        arg_count: 2, // pool_handle, conn_handle
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    Intrinsic {
        name: "db.postgres.pool.stats",
        vm_op: VmOp::PgPoolStats,
        arg_count: 1,
        returns: true, // Returns 4 values: available, in_use, total, waiters
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level pool management
    },
    // ─────────────────────────────────────────────────────────────────────────
    // SQLite Transaction Helpers
    //
    // Transaction scope helpers provide RAII-style transaction management with
    // nesting support (using savepoints). They track transaction depth per
    // connection and automatically commit/rollback based on success flag.
    // These are VM-level state tracking, not direct C FFI calls.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "db.sqlite.tx.begin",
        vm_op: VmOp::SqliteTxScopeBegin,
        arg_count: 1,  // conn_handle
        returns: true, // scope_id
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.sqlite.tx.end",
        vm_op: VmOp::SqliteTxScopeEnd,
        arg_count: 3, // conn_handle, scope_id, success
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.sqlite.tx.depth",
        vm_op: VmOp::SqliteTxDepth,
        arg_count: 1,  // conn_handle
        returns: true, // depth
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.sqlite.tx.active",
        vm_op: VmOp::SqliteTxActive,
        arg_count: 1,  // conn_handle
        returns: true, // 1 if active, 0 if not
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    // ─────────────────────────────────────────────────────────────────────────
    // PostgreSQL Transaction Helpers
    //
    // Same as SQLite - VM-level transaction scope tracking with savepoint nesting.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "db.postgres.tx.begin",
        vm_op: VmOp::PgTxScopeBegin,
        arg_count: 1,  // conn_handle
        returns: true, // scope_id
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.postgres.tx.end",
        vm_op: VmOp::PgTxScopeEnd,
        arg_count: 3, // conn_handle, scope_id, success
        returns: true,
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.postgres.tx.depth",
        vm_op: VmOp::PgTxDepth,
        arg_count: 1,  // conn_handle
        returns: true, // depth
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    Intrinsic {
        name: "db.postgres.tx.active",
        vm_op: VmOp::PgTxActive,
        arg_count: 1,  // conn_handle
        returns: true, // 1 if active, 0 if not
        kind: IntrinsicKind::HostDb,
        native_symbol: None, // VM-level transaction tracking
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Secure Memory
    //
    // These operations provide secure memory allocation and handling for
    // cryptographic secrets. Memory is locked to prevent swapping and
    // automatically zeroed when freed.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.secure_alloc",
        vm_op: VmOp::CryptoSecureAlloc,
        arg_count: 1,  // size
        returns: true, // handle (-1 on failure)
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_alloc"),
    },
    Intrinsic {
        name: "crypto.secure_free",
        vm_op: VmOp::CryptoSecureFree,
        arg_count: 1,  // handle
        returns: true, // 0 on success
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_free"),
    },
    Intrinsic {
        name: "crypto.secure_ptr",
        vm_op: VmOp::CryptoSecurePtr,
        arg_count: 1,  // handle
        returns: true, // pointer
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_ptr"),
    },
    Intrinsic {
        name: "crypto.secure_len",
        vm_op: VmOp::CryptoSecureLen,
        arg_count: 1,  // handle
        returns: true, // length
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_len"),
    },
    Intrinsic {
        name: "crypto.secure_write",
        vm_op: VmOp::CryptoSecureWrite,
        arg_count: 3,  // handle, src_ptr, len
        returns: true, // bytes written
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_write"),
    },
    Intrinsic {
        name: "crypto.secure_read",
        vm_op: VmOp::CryptoSecureRead,
        arg_count: 3,  // handle, dst_ptr, len
        returns: true, // bytes read
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_read"),
    },
    Intrinsic {
        name: "crypto.secure_zero",
        vm_op: VmOp::CryptoSecureZero,
        arg_count: 2, // ptr, len
        returns: false,
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_zero"),
    },
    Intrinsic {
        name: "crypto.secure_compare",
        vm_op: VmOp::CryptoSecureCompare,
        arg_count: 3,  // ptr_a, ptr_b, len
        returns: true, // 1 if equal, 0 if not
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_secure_compare"),
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Hashing
    //
    // Hash functions for SHA-256, SHA-384, SHA-512, SHA3, and BLAKE3.
    // Supports both one-shot hashing and incremental/streaming hashing.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.hash",
        vm_op: VmOp::CryptoHash,
        arg_count: 2,  // algorithm, data
        returns: true, // Hash
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    Intrinsic {
        name: "crypto.hasher_new",
        vm_op: VmOp::CryptoHasherNew,
        arg_count: 1,  // algorithm
        returns: true, // Hasher
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level state management
    },
    Intrinsic {
        name: "crypto.hasher_update",
        vm_op: VmOp::CryptoHasherUpdate,
        arg_count: 2, // hasher, data
        returns: false,
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level state management
    },
    Intrinsic {
        name: "crypto.hasher_finalize",
        vm_op: VmOp::CryptoHasherFinalize,
        arg_count: 1,  // hasher
        returns: true, // Hash
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level state management
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Nonce/Salt
    //
    // Random nonce and salt generation for AEAD and KDF operations.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.nonce_random",
        vm_op: VmOp::CryptoNonceRandom,
        arg_count: 1,  // algorithm
        returns: true, // Nonce
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, uses CSPRNG
    },
    Intrinsic {
        name: "crypto.salt_random",
        vm_op: VmOp::CryptoSaltRandom,
        arg_count: 1,  // size
        returns: true, // Salt
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, uses CSPRNG
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Signatures
    //
    // Digital signature operations for Ed25519, ECDSA, and RSA-PSS.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.signature_sign",
        vm_op: VmOp::CryptoSignatureSign,
        arg_count: 2,  // private_key, message
        returns: true, // Signature
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    Intrinsic {
        name: "crypto.signature_sign_hash",
        vm_op: VmOp::CryptoSignatureSignHash,
        arg_count: 2,  // private_key, hash
        returns: true, // Signature
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    Intrinsic {
        name: "crypto.signature_verify",
        vm_op: VmOp::CryptoSignatureVerify,
        arg_count: 3,  // public_key, message, signature
        returns: true, // bool
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    Intrinsic {
        name: "crypto.signature_verify_hash",
        vm_op: VmOp::CryptoSignatureVerifyHash,
        arg_count: 3,  // public_key, hash, signature
        returns: true, // bool
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Key Exchange
    //
    // Diffie-Hellman key exchange for X25519.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.shared_secret_exchange",
        vm_op: VmOp::CryptoSharedSecretExchange,
        arg_count: 2,  // private_key, peer_public_key
        returns: true, // SharedSecret
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level, delegates to crypto library
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Ciphertext
    //
    // Ciphertext construction from separate ciphertext and tag.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.ciphertext_from_parts",
        vm_op: VmOp::CryptoCiphertextFromParts,
        arg_count: 3,  // algorithm, ciphertext, tag
        returns: true, // Ciphertext
        kind: IntrinsicKind::HostCrypto,
        native_symbol: None, // VM-level byte concatenation
    },
    // ─────────────────────────────────────────────────────────────────────────
    // Cryptographic operations - Encoding
    //
    // Binary encoding/decoding for hex and Base64.
    // ─────────────────────────────────────────────────────────────────────────
    Intrinsic {
        name: "crypto.encoding_to_hex",
        vm_op: VmOp::CryptoEncodingToHex,
        arg_count: 1,  // bytes
        returns: true, // String
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_hex_encode"),
    },
    Intrinsic {
        name: "crypto.encoding_from_hex",
        vm_op: VmOp::CryptoEncodingFromHex,
        arg_count: 1,  // hex_string
        returns: true, // bytes (or error)
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_hex_decode"),
    },
    Intrinsic {
        name: "crypto.encoding_to_base64",
        vm_op: VmOp::CryptoEncodingToBase64,
        arg_count: 1,  // bytes
        returns: true, // String
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_base64_encode"),
    },
    Intrinsic {
        name: "crypto.encoding_from_base64",
        vm_op: VmOp::CryptoEncodingFromBase64,
        arg_count: 1,  // base64_string
        returns: true, // bytes (or error)
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_base64_decode"),
    },
    Intrinsic {
        name: "crypto.encoding_to_base64url",
        vm_op: VmOp::CryptoEncodingToBase64Url,
        arg_count: 1,  // bytes
        returns: true, // String
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_base64url_encode"),
    },
    Intrinsic {
        name: "crypto.encoding_from_base64url",
        vm_op: VmOp::CryptoEncodingFromBase64Url,
        arg_count: 1,  // base64url_string
        returns: true, // bytes (or error)
        kind: IntrinsicKind::HostCrypto,
        native_symbol: Some("arth_rt_base64url_decode"),
    },
];

/// Global lazily-initialized lookup map for fast intrinsic lookup by name.
static INTRINSIC_MAP: OnceLock<HashMap<&'static str, &'static Intrinsic>> = OnceLock::new();

/// Initialize the intrinsic lookup map.
fn init_intrinsic_map() -> HashMap<&'static str, &'static Intrinsic> {
    let mut map = HashMap::with_capacity(INTRINSICS.len());
    for intrinsic in INTRINSICS {
        map.insert(intrinsic.name, intrinsic);
    }
    map
}

/// Look up an intrinsic by name.
///
/// Returns `Some(&Intrinsic)` if the intrinsic exists, `None` otherwise.
///
/// # Example
///
/// ```ignore
/// if let Some(intrinsic) = lookup("math.sqrt") {
///     println!("Found intrinsic: {:?}", intrinsic.vm_op);
/// }
/// ```
pub fn lookup(name: &str) -> Option<&'static Intrinsic> {
    let map = INTRINSIC_MAP.get_or_init(init_intrinsic_map);
    map.get(name).copied()
}

/// Check if a name is a valid intrinsic.
pub fn is_intrinsic(name: &str) -> bool {
    lookup(name).is_some()
}

/// Get all registered intrinsics.
pub fn all_intrinsics() -> &'static [Intrinsic] {
    INTRINSICS
}

/// Extract the intrinsic name from an `@intrinsic("name")` attribute argument string.
///
/// The argument is expected to be a quoted string like `"math.sqrt"`.
/// Returns `None` if the format is invalid.
pub fn parse_intrinsic_attr(args: &str) -> Option<&str> {
    let trimmed = args.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        Some(&trimmed[1..trimmed.len() - 1])
    } else {
        None
    }
}

/// Get the intrinsic for a function based on its attributes.
///
/// Looks for an `@intrinsic("name")` attribute and returns the corresponding intrinsic.
pub fn get_intrinsic_from_attrs(
    attrs: &[crate::compiler::hir::HirAttr],
) -> Option<&'static Intrinsic> {
    for attr in attrs {
        if attr.name == "intrinsic" {
            if let Some(args) = &attr.args {
                if let Some(name) = parse_intrinsic_attr(args) {
                    return lookup(name);
                }
            }
        }
    }
    None
}

/// Global lazily-initialized lookup map for fast VmOp to Intrinsic lookup.
static VMOP_INTRINSIC_MAP: OnceLock<HashMap<VmOp, &'static Intrinsic>> = OnceLock::new();

/// Initialize the VmOp -> Intrinsic lookup map.
fn init_vmop_map() -> HashMap<VmOp, &'static Intrinsic> {
    let mut map = HashMap::with_capacity(INTRINSICS.len());
    for intrinsic in INTRINSICS {
        map.insert(intrinsic.vm_op.clone(), intrinsic);
    }
    map
}

/// Look up an intrinsic by its VmOp.
///
/// Returns `Some(&Intrinsic)` if found, `None` otherwise.
///
/// # Example
///
/// ```ignore
/// if let Some(intrinsic) = lookup_by_vmop(&VmOp::MathSqrt) {
///     assert_eq!(intrinsic.name, "math.sqrt");
/// }
/// ```
pub fn lookup_by_vmop(op: &VmOp) -> Option<&'static Intrinsic> {
    let map = VMOP_INTRINSIC_MAP.get_or_init(init_vmop_map);
    map.get(op).copied()
}

/// Get the native C FFI symbol for a VmOp, if one exists.
///
/// This is used by the LLVM backend to emit direct calls to arth-rt functions
/// instead of going through VM host call opcodes.
///
/// Returns `Some("arth_rt_*")` if the operation has a direct C FFI mapping,
/// or `None` if:
/// - The operation uses LLVM intrinsics (e.g., math.sqrt -> llvm.sqrt)
/// - The operation requires VM-level state management (pools, tx scopes)
/// - The operation needs glue code (struct marshalling, multiple C calls)
///
/// # Example
///
/// ```ignore
/// if let Some(symbol) = native_symbol_for_vmop(&VmOp::FileOpen) {
///     // Emit: call @arth_rt_file_open(...)
///     assert_eq!(symbol, "arth_rt_file_open");
/// }
/// ```
pub fn native_symbol_for_vmop(op: &VmOp) -> Option<&'static str> {
    lookup_by_vmop(op).and_then(|i| i.native_symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_existing() {
        assert!(lookup("math.sqrt").is_some());
        assert!(lookup("math.pow").is_some());
        assert!(lookup("list.new").is_some());
        assert!(lookup("http.fetch").is_some());
    }

    #[test]
    fn test_lookup_nonexistent() {
        assert!(lookup("nonexistent.func").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("math").is_none());
    }

    #[test]
    fn test_intrinsic_properties() {
        let sqrt = lookup("math.sqrt").unwrap();
        assert_eq!(sqrt.name, "math.sqrt");
        assert_eq!(sqrt.arg_count, 1);
        assert!(sqrt.returns);
        assert_eq!(sqrt.vm_op, VmOp::MathSqrt);

        let pow = lookup("math.pow").unwrap();
        assert_eq!(pow.name, "math.pow");
        assert_eq!(pow.arg_count, 2);
        assert!(pow.returns);
    }

    #[test]
    fn test_parse_intrinsic_attr() {
        assert_eq!(parse_intrinsic_attr(r#""math.sqrt""#), Some("math.sqrt"));
        assert_eq!(parse_intrinsic_attr(r#"  "list.new"  "#), Some("list.new"));
        assert_eq!(parse_intrinsic_attr("math.sqrt"), None);
        assert_eq!(parse_intrinsic_attr(""), None);
        assert_eq!(parse_intrinsic_attr(r#"""#), None);
    }

    #[test]
    fn test_is_intrinsic() {
        assert!(is_intrinsic("math.sqrt"));
        assert!(is_intrinsic("list.push"));
        assert!(!is_intrinsic("not.an.intrinsic"));
    }

    #[test]
    fn test_all_intrinsics_not_empty() {
        assert!(!all_intrinsics().is_empty());
        assert!(all_intrinsics().len() > 50); // We have many intrinsics
    }

    #[test]
    fn test_no_duplicate_names() {
        let mut seen = std::collections::HashSet::new();
        for intrinsic in INTRINSICS {
            assert!(
                seen.insert(intrinsic.name),
                "Duplicate intrinsic name: {}",
                intrinsic.name
            );
        }
    }

    #[test]
    fn test_lookup_by_vmop() {
        let intrinsic = lookup_by_vmop(&VmOp::MathSqrt);
        assert!(intrinsic.is_some());
        assert_eq!(intrinsic.unwrap().name, "math.sqrt");

        let intrinsic = lookup_by_vmop(&VmOp::FileOpen);
        assert!(intrinsic.is_some());
        assert_eq!(intrinsic.unwrap().name, "io.file_open");
    }

    #[test]
    fn test_native_symbol_for_vmop() {
        // I/O operations should have native symbols
        assert_eq!(
            native_symbol_for_vmop(&VmOp::FileOpen),
            Some("arth_rt_file_open")
        );
        assert_eq!(
            native_symbol_for_vmop(&VmOp::FileClose),
            Some("arth_rt_file_close")
        );

        // Math operations use LLVM intrinsics, so no native symbol
        assert_eq!(native_symbol_for_vmop(&VmOp::MathSqrt), None);
        assert_eq!(native_symbol_for_vmop(&VmOp::MathSin), None);

        // CoreVm operations have no native symbol
        assert_eq!(native_symbol_for_vmop(&VmOp::ListNew), None);
        assert_eq!(native_symbol_for_vmop(&VmOp::MapPut), None);

        // SQLite operations have native symbols
        assert_eq!(
            native_symbol_for_vmop(&VmOp::SqliteOpen),
            Some("arth_rt_sqlite_open")
        );

        // PostgreSQL sync operations have native symbols
        assert_eq!(
            native_symbol_for_vmop(&VmOp::PgConnect),
            Some("arth_rt_pg_connect")
        );

        // PostgreSQL async operations that map directly
        assert_eq!(
            native_symbol_for_vmop(&VmOp::PgQueryAsync),
            Some("arth_rt_pg_send_query")
        );
        assert_eq!(
            native_symbol_for_vmop(&VmOp::PgGetAsyncResult),
            Some("arth_rt_pg_get_result")
        );

        // Pool operations are VM-level, no native symbol
        assert_eq!(native_symbol_for_vmop(&VmOp::SqlitePoolCreate), None);
        assert_eq!(native_symbol_for_vmop(&VmOp::PgPoolAcquire), None);

        // Transaction scope helpers are VM-level, no native symbol
        assert_eq!(native_symbol_for_vmop(&VmOp::SqliteTxScopeBegin), None);
        assert_eq!(native_symbol_for_vmop(&VmOp::PgTxDepth), None);
    }
}
