//! DWARF Exception Handling for Native Arth
//!
//! This module implements proper stack unwinding and exception handling using
//! the Itanium C++ ABI style exception handling (DWARF based). This enables
//! try/catch/throw semantics in native-compiled Arth code.
//!
//! # Architecture
//!
//! The exception handling system consists of:
//! - `ArthException`: The exception object passed during unwinding
//! - `__arth_personality_v0`: Personality function for DWARF unwinding
//! - `arth_rt_throw`: Initiates stack unwinding
//! - `arth_rt_begin_catch` / `arth_rt_end_catch`: Catch block management
//!
//! # LLVM IR Usage
//!
//! Functions that may catch exceptions need:
//! ```llvm
//! define i64 @func() personality ptr @__arth_personality_v0 {
//!   invoke void @might_throw() to label %normal unwind label %lpad
//! lpad:
//!   %ex = landingpad { ptr, i32 } catch ptr @ExceptionTypeInfo
//!   ...
//! }
//! ```

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

// -----------------------------------------------------------------------------
// Exception Constants (Itanium ABI)
// -----------------------------------------------------------------------------

/// Unwind action flags (from unwind.h _Unwind_Action)
#[allow(dead_code)]
mod unwind_action {
    pub const SEARCH_PHASE: u32 = 1;
    pub const CLEANUP_PHASE: u32 = 2;
    pub const HANDLER_FRAME: u32 = 4;
    pub const FORCE_UNWIND: u32 = 8;
}

/// Unwind reason codes (from unwind.h _Unwind_Reason_Code)
#[allow(dead_code)]
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwindReasonCode {
    NoReason = 0,
    ForeignExceptionCaught = 1,
    FatalPhase2Error = 2,
    FatalPhase1Error = 3,
    NormalStop = 4,
    EndOfStack = 5,
    HandlerFound = 6,
    InstallContext = 7,
    ContinueUnwind = 8,
}

/// Exception class identifier for Arth exceptions
/// This is an 8-byte identifier: "ARTH\0\0\0\0" in little-endian
const ARTH_EXCEPTION_CLASS: u64 = 0x0000000048545241; // "ARTH" + padding

// -----------------------------------------------------------------------------
// Unwind Header (Itanium ABI compatible)
// -----------------------------------------------------------------------------

/// The standard _Unwind_Exception header
/// This must be the first field of any exception object
#[repr(C)]
pub struct UnwindException {
    /// 8-byte exception class identifier
    pub exception_class: u64,
    /// Cleanup function called when exception is caught or destroyed
    pub exception_cleanup: Option<unsafe extern "C" fn(UnwindReasonCode, *mut UnwindException)>,
    /// Private data for unwinder (2 pointers)
    pub private: [usize; 2],
}

// -----------------------------------------------------------------------------
// Arth Exception Object
// -----------------------------------------------------------------------------

/// The complete Arth exception object
///
/// Layout: [UnwindException header][ArthException data]
/// The UnwindException header MUST be first for compatibility with libunwind.
#[repr(C)]
pub struct ArthException {
    /// Standard unwind header (MUST be first)
    pub unwind_header: UnwindException,

    /// Exception type identifier (hash of fully qualified exception type name)
    /// This is matched against catch clause type IDs
    pub type_id: u64,

    /// Pointer to exception type name (null-terminated C string)
    /// Points to static string in the binary, do not free
    pub type_name: *const u8,
    /// Length of type name (excluding null terminator)
    pub type_name_len: usize,

    /// Pointer to exception payload (exception object fields)
    /// This is heap-allocated and owned by the exception
    pub payload: *mut u8,
    /// Size of payload in bytes
    pub payload_size: usize,

    /// Whether this exception has been caught
    /// Used to prevent double-free
    caught: bool,
}

impl ArthException {
    /// Create a new exception
    unsafe fn new(type_id: u64, type_name: *const u8, type_name_len: usize) -> *mut Self {
        unsafe {
            let layout = std::alloc::Layout::new::<ArthException>();
            let ptr = std::alloc::alloc(layout) as *mut ArthException;
            if ptr.is_null() {
                // Out of memory during exception creation - abort
                libc::abort();
            }

            (*ptr).unwind_header.exception_class = ARTH_EXCEPTION_CLASS;
            (*ptr).unwind_header.exception_cleanup = Some(arth_exception_cleanup);
            (*ptr).unwind_header.private = [0; 2];
            (*ptr).type_id = type_id;
            (*ptr).type_name = type_name;
            (*ptr).type_name_len = type_name_len;
            (*ptr).payload = ptr::null_mut();
            (*ptr).payload_size = 0;
            (*ptr).caught = false;

            ptr
        }
    }

    /// Free the exception and its payload
    unsafe fn destroy(ex: *mut Self) {
        unsafe {
            if !ex.is_null() {
                // Free payload if present
                if !(*ex).payload.is_null() && (*ex).payload_size > 0 {
                    let layout =
                        std::alloc::Layout::from_size_align_unchecked((*ex).payload_size, 8);
                    std::alloc::dealloc((*ex).payload, layout);
                }
                // Free exception object
                let layout = std::alloc::Layout::new::<ArthException>();
                std::alloc::dealloc(ex as *mut u8, layout);
            }
        }
    }
}

/// Cleanup function called by unwinder when exception is destroyed
unsafe extern "C" fn arth_exception_cleanup(
    _reason: UnwindReasonCode,
    exception: *mut UnwindException,
) {
    unsafe {
        // The ArthException starts at the same address as UnwindException (it's the first field)
        let arth_ex = exception as *mut ArthException;
        ArthException::destroy(arth_ex);
    }
}

// -----------------------------------------------------------------------------
// Exception Type Info
// -----------------------------------------------------------------------------

/// Type info structure for exception type matching
/// These are generated by the compiler as global constants
#[repr(C)]
pub struct ArthTypeInfo {
    /// Type ID (hash of fully qualified type name)
    pub type_id: u64,
    /// Pointer to type name (null-terminated)
    pub type_name: *const u8,
}

/// Hash a type name to produce a type ID
/// Uses FNV-1a for speed and reasonable distribution
#[inline]
pub fn hash_type_name(name: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for &byte in name {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// -----------------------------------------------------------------------------
// Thread-Local Exception State
// -----------------------------------------------------------------------------

/// Currently caught exception (for `arth_rt_get_current_exception`)
static CURRENT_EXCEPTION: AtomicPtr<ArthException> = AtomicPtr::new(ptr::null_mut());

/// Set the current exception for the thread
fn set_current_exception(ex: *mut ArthException) {
    CURRENT_EXCEPTION.store(ex, Ordering::SeqCst);
}

/// Get the current exception
fn get_current_exception() -> *mut ArthException {
    CURRENT_EXCEPTION.load(Ordering::SeqCst)
}

// -----------------------------------------------------------------------------
// External Unwind Functions (from libunwind/libgcc)
// -----------------------------------------------------------------------------

#[allow(improper_ctypes)]
unsafe extern "C" {
    /// Raise an exception and begin unwinding
    fn _Unwind_RaiseException(exception: *mut UnwindException) -> UnwindReasonCode;

    /// Resume unwinding after a cleanup (non-catching landing pad)
    fn _Unwind_Resume(exception: *mut UnwindException) -> !;

    /// Delete an exception object (calls cleanup)
    fn _Unwind_DeleteException(exception: *mut UnwindException);

    /// Get the language-specific data area (LSDA) pointer
    fn _Unwind_GetLanguageSpecificData(context: *mut u8) -> *const u8;

    /// Get the instruction pointer in the current frame
    fn _Unwind_GetIP(context: *mut u8) -> usize;

    /// Get the region start (function start address)
    fn _Unwind_GetRegionStart(context: *mut u8) -> usize;

    /// Set the return registers for landing pad
    fn _Unwind_SetGR(context: *mut u8, index: i32, value: usize);

    /// Set the instruction pointer for landing pad
    fn _Unwind_SetIP(context: *mut u8, value: usize);
}

// -----------------------------------------------------------------------------
// Personality Function
// -----------------------------------------------------------------------------

/// The Arth personality function for DWARF exception handling
///
/// This is called by the unwinder for each frame during unwinding.
/// It examines the LSDA to determine if this frame can handle the exception.
///
/// # Arguments
/// - `version`: ABI version (must be 1)
/// - `actions`: Bitmask of _Unwind_Action flags
/// - `exception_class`: 8-byte exception class identifier
/// - `exception_object`: Pointer to the exception being thrown
/// - `context`: Unwind context for this frame
///
/// # Returns
/// - `_URC_CONTINUE_UNWIND`: This frame doesn't handle it, continue unwinding
/// - `_URC_HANDLER_FOUND`: Found a handler, ready to install context
/// - `_URC_INSTALL_CONTEXT`: Install the context (jump to landing pad)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_personality_v0(
    version: i32,
    actions: u32,
    exception_class: u64,
    exception_object: *mut UnwindException,
    context: *mut u8,
) -> UnwindReasonCode {
    // Version check
    if version != 1 {
        return UnwindReasonCode::FatalPhase1Error;
    }

    // Get the LSDA for this frame
    let lsda = unsafe { _Unwind_GetLanguageSpecificData(context) };
    if lsda.is_null() {
        // No LSDA means no exception handling in this frame
        return UnwindReasonCode::ContinueUnwind;
    }

    // Check if this is an Arth exception
    let is_arth_exception = exception_class == ARTH_EXCEPTION_CLASS;
    let arth_exception = if is_arth_exception {
        exception_object as *mut ArthException
    } else {
        ptr::null_mut()
    };

    // Get instruction pointer and region start
    let ip = unsafe { _Unwind_GetIP(context) };
    let func_start = unsafe { _Unwind_GetRegionStart(context) };

    // Parse LSDA to find matching handler
    match parse_lsda_and_find_handler(lsda, ip, func_start, arth_exception) {
        LsdaResult::NoHandler => UnwindReasonCode::ContinueUnwind,
        LsdaResult::Cleanup { landing_pad } => {
            if actions & unwind_action::CLEANUP_PHASE != 0 {
                install_landing_pad(context, exception_object, landing_pad, 0);
                UnwindReasonCode::InstallContext
            } else {
                UnwindReasonCode::ContinueUnwind
            }
        }
        LsdaResult::Catch {
            landing_pad,
            selector,
        } => {
            if actions & unwind_action::SEARCH_PHASE != 0 {
                UnwindReasonCode::HandlerFound
            } else {
                install_landing_pad(context, exception_object, landing_pad, selector);
                UnwindReasonCode::InstallContext
            }
        }
    }
}

/// Result of parsing LSDA
enum LsdaResult {
    /// No handler found in this frame
    NoHandler,
    /// Cleanup code found (finally block, destructors)
    Cleanup { landing_pad: usize },
    /// Catch handler found
    Catch { landing_pad: usize, selector: i32 },
}

/// Parse the LSDA and find a matching handler for the current IP
fn parse_lsda_and_find_handler(
    lsda: *const u8,
    ip: usize,
    func_start: usize,
    exception: *mut ArthException,
) -> LsdaResult {
    unsafe {
        let mut reader = LsdaReader::new(lsda);

        // Read header
        let landing_pad_base_encoding = reader.read_u8();
        let landing_pad_base = if landing_pad_base_encoding != 0xff {
            reader.read_uleb128() as usize
        } else {
            func_start
        };

        let type_table_encoding = reader.read_u8();
        let type_table_offset = if type_table_encoding != 0xff {
            reader.read_uleb128() as usize
        } else {
            0
        };

        let type_table = if type_table_offset != 0 {
            reader.ptr.add(type_table_offset)
        } else {
            ptr::null()
        };

        // Read call-site table
        let call_site_encoding = reader.read_u8();
        let call_site_table_length = reader.read_uleb128() as usize;
        let call_site_table_end = reader.ptr.add(call_site_table_length);

        // IP is relative to function start.
        // Per Itanium EH ABI, the IP points to the instruction *after* the throw site
        // during search phase, so we subtract 1 when matching call-site ranges.
        let relative_ip = ip.saturating_sub(func_start).saturating_sub(1);

        // Search call-site table
        while reader.ptr < call_site_table_end {
            let cs_start = reader.read_encoded(call_site_encoding);
            let cs_len = reader.read_encoded(call_site_encoding);
            let cs_lpad = reader.read_encoded(call_site_encoding);
            let cs_action = reader.read_uleb128();

            // Check if IP is in this call site
            if relative_ip >= cs_start && relative_ip < cs_start + cs_len {
                if cs_lpad == 0 {
                    // No landing pad - continue unwinding
                    return LsdaResult::NoHandler;
                }

                let landing_pad = landing_pad_base + cs_lpad;

                if cs_action == 0 {
                    // Cleanup only (finally block)
                    return LsdaResult::Cleanup { landing_pad };
                }

                let _ = (type_table, exception, call_site_table_end);
                // For phase-1 native semantics, treat any non-zero action as a catch
                // and let the generated catch-chain code perform type filtering.
                return LsdaResult::Catch {
                    landing_pad,
                    selector: 1,
                };
            }
        }

        LsdaResult::NoHandler
    }
}

/// Install the landing pad and prepare to jump
fn install_landing_pad(
    context: *mut u8,
    exception: *mut UnwindException,
    landing_pad: usize,
    selector: i32,
) {
    unsafe {
        // GR 0 = exception pointer (first return value from landingpad)
        _Unwind_SetGR(context, 0, exception as usize);
        // GR 1 = selector (second return value from landingpad)
        _Unwind_SetGR(context, 1, selector as usize);
        // Set IP to landing pad
        _Unwind_SetIP(context, landing_pad);
    }
}

// -----------------------------------------------------------------------------
// LSDA Reader Helper
// -----------------------------------------------------------------------------

struct LsdaReader {
    ptr: *const u8,
}

impl LsdaReader {
    fn new(ptr: *const u8) -> Self {
        LsdaReader { ptr }
    }

    unsafe fn read_u8(&mut self) -> u8 {
        unsafe {
            let val = *self.ptr;
            self.ptr = self.ptr.add(1);
            val
        }
    }

    unsafe fn read_uleb128(&mut self) -> u64 {
        unsafe {
            let mut result: u64 = 0;
            let mut shift = 0;
            loop {
                let byte = self.read_u8();
                result |= ((byte & 0x7f) as u64) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            result
        }
    }

    unsafe fn read_encoded(&mut self, encoding: u8) -> usize {
        unsafe {
            if encoding == 0xff {
                return 0;
            }

            let base = encoding & 0x0f;
            match base {
                0x00 => {
                    // DW_EH_PE_absptr - pointer-sized absolute
                    let ptr = self.ptr as *const usize;
                    self.ptr = self.ptr.add(std::mem::size_of::<usize>());
                    *ptr
                }
                0x01 => {
                    // DW_EH_PE_uleb128
                    self.read_uleb128() as usize
                }
                0x02 => {
                    // DW_EH_PE_udata2
                    let val = *(self.ptr as *const u16);
                    self.ptr = self.ptr.add(2);
                    val as usize
                }
                0x03 => {
                    // DW_EH_PE_udata4
                    let val = *(self.ptr as *const u32);
                    self.ptr = self.ptr.add(4);
                    val as usize
                }
                0x04 => {
                    // DW_EH_PE_udata8
                    let val = *(self.ptr as *const u64);
                    self.ptr = self.ptr.add(8);
                    val as usize
                }
                _ => self.read_uleb128() as usize,
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Public Exception API
// -----------------------------------------------------------------------------

/// Throw an exception
///
/// # Arguments
/// - `type_id`: Hash of the exception type name
/// - `type_name`: Pointer to exception type name (static string)
/// - `type_name_len`: Length of type name
/// - `payload`: Pointer to exception data (will be copied)
/// - `payload_size`: Size of payload in bytes
///
/// # Never Returns
/// This function initiates stack unwinding and never returns.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn arth_rt_throw(
    type_id: u64,
    type_name: *const u8,
    type_name_len: usize,
    payload: *const u8,
    payload_size: usize,
) -> ! {
    unsafe {
        // Allocate exception
        let exception = ArthException::new(type_id, type_name, type_name_len);

        // Copy payload if present
        if !payload.is_null() && payload_size > 0 {
            let layout = std::alloc::Layout::from_size_align_unchecked(payload_size, 8);
            let payload_copy = std::alloc::alloc(layout);
            if payload_copy.is_null() {
                ArthException::destroy(exception);
                libc::abort();
            }
            ptr::copy_nonoverlapping(payload, payload_copy, payload_size);
            (*exception).payload = payload_copy;
            (*exception).payload_size = payload_size;
        }

        // Raise the exception
        let result = _Unwind_RaiseException(&mut (*exception).unwind_header);

        // If we get here, unwinding failed
        eprintln!(
            "arth_rt_throw: _Unwind_RaiseException returned {:?}",
            result
        );
        libc::abort();
    }
}

/// Begin catching an exception
///
/// Called at the start of a catch block. Returns the exception object.
///
/// # Arguments
/// - `exception_ptr`: Pointer to exception from landing pad
///
/// # Returns
/// Pointer to ArthException (caller extracts payload as needed)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_begin_catch(exception_ptr: *mut u8) -> *mut ArthException {
    let exception = exception_ptr as *mut ArthException;

    // Mark as caught
    unsafe {
        if !exception.is_null() {
            (*exception).caught = true;
            set_current_exception(exception);
        }
    }

    exception
}

/// End catching an exception
///
/// Called at the end of a catch block. Cleans up the exception.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_end_catch() {
    let exception = get_current_exception();
    if !exception.is_null() {
        unsafe {
            _Unwind_DeleteException(&mut (*exception).unwind_header);
        }
        set_current_exception(ptr::null_mut());
    }
}

/// Rethrow the current exception
///
/// Can only be called from within a catch block.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn arth_rt_rethrow() -> ! {
    let exception = get_current_exception();
    if exception.is_null() {
        // No exception to rethrow - abort
        eprintln!("arth_rt_rethrow: no exception to rethrow");
        unsafe { libc::abort() };
    }

    unsafe {
        (*exception).caught = false;
        set_current_exception(ptr::null_mut());
        _Unwind_Resume(&mut (*exception).unwind_header);
    }
}

/// Resume unwinding after a cleanup (finally block)
///
/// Called when a finally block completes and unwinding should continue.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn arth_rt_resume_unwind(exception_ptr: *mut u8) -> ! {
    let exception = exception_ptr as *mut UnwindException;
    if exception.is_null() {
        unsafe { libc::abort() };
    }
    unsafe {
        _Unwind_Resume(exception);
    }
}

/// Get the type ID of the current exception
///
/// # Returns
/// Type ID, or 0 if no exception
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_exception_type_id() -> u64 {
    let exception = get_current_exception();
    if exception.is_null() {
        0
    } else {
        unsafe { (*exception).type_id }
    }
}

/// Get the payload pointer of the current exception
///
/// # Returns
/// Pointer to payload, or null if no exception
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_exception_payload() -> *mut u8 {
    let exception = get_current_exception();
    if exception.is_null() {
        ptr::null_mut()
    } else {
        unsafe { (*exception).payload }
    }
}

/// Get the payload size of the current exception
///
/// # Returns
/// Payload size in bytes, or 0 if no exception
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_exception_payload_size() -> usize {
    let exception = get_current_exception();
    if exception.is_null() {
        0
    } else {
        unsafe { (*exception).payload_size }
    }
}

// -----------------------------------------------------------------------------
// Helper: Compute Type ID at Runtime
// -----------------------------------------------------------------------------

/// Compute a type ID from a type name string
///
/// Exposed so Arth can compute type IDs for dynamic dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_type_id(type_name: *const u8, type_name_len: usize) -> u64 {
    if type_name.is_null() {
        return 0;
    }
    let name = unsafe { std::slice::from_raw_parts(type_name, type_name_len) };
    hash_type_name(name)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_type_name() {
        let h1 = hash_type_name(b"IoError");
        let h2 = hash_type_name(b"IoError");
        let h3 = hash_type_name(b"TimeoutError");

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_ne!(h1, 0);
    }

    #[test]
    fn test_arth_exception_class() {
        // Verify the exception class spells "ARTH"
        let bytes = ARTH_EXCEPTION_CLASS.to_le_bytes();
        assert_eq!(&bytes[0..4], b"ARTH");
    }
}
