//! Encoding/Decoding functions (Base64, etc.)
//!
//! Provides C FFI wrappers for encoding operations used by mail protocols,
//! WebSocket handshakes, and other parts of the Arth stdlib.

// Base64 encoding table (RFC 4648)
const BASE64_ENCODE_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Base64 decoding table (255 = invalid, 64 = padding '=')
const BASE64_DECODE_TABLE: [u8; 256] = {
    let mut table = [255u8; 256];
    let mut i = 0u8;
    while i < 64 {
        table[BASE64_ENCODE_TABLE[i as usize] as usize] = i;
        i += 1;
    }
    table[b'=' as usize] = 64; // padding marker
    table
};

/// Calculate the output length for base64 encoding
/// Returns the number of bytes needed for the encoded output (including padding)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64_encode_len(input_len: usize) -> usize {
    if input_len == 0 {
        return 0;
    }
    // Base64 encoding produces 4 output bytes for every 3 input bytes, rounded up
    input_len.div_ceil(3) * 4
}

/// Calculate the maximum output length for base64 decoding
/// Returns the maximum number of bytes needed for the decoded output
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64_decode_len(input_len: usize) -> usize {
    if input_len == 0 {
        return 0;
    }
    // Base64 decoding produces at most 3 output bytes for every 4 input bytes
    (input_len / 4) * 3
}

/// Encode data to base64
///
/// # Arguments
/// * `input` - Pointer to input data
/// * `input_len` - Length of input data
/// * `output` - Pointer to output buffer (must be at least arth_rt_base64_encode_len bytes)
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if output buffer is too small
/// * -2 if input or output is null
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64_encode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    if input_len == 0 {
        return 0;
    }

    let required_len = arth_rt_base64_encode_len(input_len);
    if output_len < required_len {
        return -1;
    }

    let input_slice = unsafe { std::slice::from_raw_parts(input, input_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, required_len) };

    let mut out_idx = 0;
    let mut in_idx = 0;

    // Process 3 bytes at a time
    while in_idx + 3 <= input_len {
        let b0 = input_slice[in_idx] as usize;
        let b1 = input_slice[in_idx + 1] as usize;
        let b2 = input_slice[in_idx + 2] as usize;

        output_slice[out_idx] = BASE64_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64_ENCODE_TABLE[((b0 & 0x03) << 4) | (b1 >> 4)];
        output_slice[out_idx + 2] = BASE64_ENCODE_TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)];
        output_slice[out_idx + 3] = BASE64_ENCODE_TABLE[b2 & 0x3f];

        in_idx += 3;
        out_idx += 4;
    }

    // Handle remaining bytes
    let remaining = input_len - in_idx;
    if remaining == 1 {
        let b0 = input_slice[in_idx] as usize;
        output_slice[out_idx] = BASE64_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64_ENCODE_TABLE[(b0 & 0x03) << 4];
        output_slice[out_idx + 2] = b'=';
        output_slice[out_idx + 3] = b'=';
        out_idx += 4;
    } else if remaining == 2 {
        let b0 = input_slice[in_idx] as usize;
        let b1 = input_slice[in_idx + 1] as usize;
        output_slice[out_idx] = BASE64_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64_ENCODE_TABLE[((b0 & 0x03) << 4) | (b1 >> 4)];
        output_slice[out_idx + 2] = BASE64_ENCODE_TABLE[(b1 & 0x0f) << 2];
        output_slice[out_idx + 3] = b'=';
        out_idx += 4;
    }

    out_idx as i64
}

/// Decode base64 data
///
/// # Arguments
/// * `input` - Pointer to base64 encoded data
/// * `input_len` - Length of input data
/// * `output` - Pointer to output buffer (must be at least arth_rt_base64_decode_len bytes)
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if output buffer is too small
/// * -2 if input or output is null
/// * -3 if input contains invalid base64 characters
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64_decode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    if input_len == 0 {
        return 0;
    }

    // Input must be a multiple of 4
    if !input_len.is_multiple_of(4) {
        return -3;
    }

    let max_output_len = arth_rt_base64_decode_len(input_len);
    if output_len < max_output_len {
        return -1;
    }

    let input_slice = unsafe { std::slice::from_raw_parts(input, input_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, max_output_len) };

    let mut out_idx = 0;
    let mut in_idx = 0;

    while in_idx < input_len {
        let c0 = BASE64_DECODE_TABLE[input_slice[in_idx] as usize];
        let c1 = BASE64_DECODE_TABLE[input_slice[in_idx + 1] as usize];
        let c2 = BASE64_DECODE_TABLE[input_slice[in_idx + 2] as usize];
        let c3 = BASE64_DECODE_TABLE[input_slice[in_idx + 3] as usize];

        // Check for invalid characters (255)
        if c0 == 255 || c1 == 255 || (c2 == 255 && c2 != 64) || (c3 == 255 && c3 != 64) {
            return -3;
        }

        // First byte is always produced
        output_slice[out_idx] = (c0 << 2) | (c1 >> 4);
        out_idx += 1;

        // Second byte if not padding
        if c2 != 64 {
            output_slice[out_idx] = ((c1 & 0x0f) << 4) | (c2 >> 2);
            out_idx += 1;

            // Third byte if not padding
            if c3 != 64 {
                output_slice[out_idx] = ((c2 & 0x03) << 6) | c3;
                out_idx += 1;
            }
        }

        in_idx += 4;
    }

    out_idx as i64
}

// =============================================================================
// Hex Encoding/Decoding
// =============================================================================

/// Hex encoding alphabet (lowercase)
const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// Hex decoding table
const HEX_DECODE_TABLE: [i8; 256] = {
    let mut table = [-1i8; 256];
    table[b'0' as usize] = 0;
    table[b'1' as usize] = 1;
    table[b'2' as usize] = 2;
    table[b'3' as usize] = 3;
    table[b'4' as usize] = 4;
    table[b'5' as usize] = 5;
    table[b'6' as usize] = 6;
    table[b'7' as usize] = 7;
    table[b'8' as usize] = 8;
    table[b'9' as usize] = 9;
    table[b'a' as usize] = 10;
    table[b'b' as usize] = 11;
    table[b'c' as usize] = 12;
    table[b'd' as usize] = 13;
    table[b'e' as usize] = 14;
    table[b'f' as usize] = 15;
    table[b'A' as usize] = 10;
    table[b'B' as usize] = 11;
    table[b'C' as usize] = 12;
    table[b'D' as usize] = 13;
    table[b'E' as usize] = 14;
    table[b'F' as usize] = 15;
    table
};

/// Calculate hex encoded length
///
/// # C ABI
/// ```c
/// size_t arth_rt_hex_encode_len(size_t input_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hex_encode_len(input_len: usize) -> usize {
    input_len * 2
}

/// Calculate hex decoded length
///
/// # C ABI
/// ```c
/// size_t arth_rt_hex_decode_len(size_t input_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hex_decode_len(input_len: usize) -> usize {
    input_len / 2
}

/// Encode bytes to hex (lowercase)
///
/// Returns the number of bytes written, or negative on error:
/// * -1 if output buffer is too small
/// * -2 if input or output is null
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hex_encode(const uint8_t* input, size_t input_len,
///                            uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hex_encode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    let needed = input_len * 2;
    if output_len < needed {
        return -1;
    }

    for i in 0..input_len {
        unsafe {
            let b = *input.add(i);
            *output.add(i * 2) = HEX_LOWER[(b >> 4) as usize];
            *output.add(i * 2 + 1) = HEX_LOWER[(b & 0x0f) as usize];
        }
    }

    needed as i64
}

/// Decode hex to bytes
///
/// Returns the number of bytes written, or negative on error:
/// * -1 if output buffer is too small
/// * -2 if input or output is null
/// * -3 if input contains invalid hex characters
/// * -4 if input length is odd
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hex_decode(const uint8_t* input, size_t input_len,
///                            uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hex_decode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    // Must be even length
    if !input_len.is_multiple_of(2) {
        return -4;
    }

    let out_len = input_len / 2;
    if output_len < out_len {
        return -1;
    }

    for i in 0..out_len {
        unsafe {
            let hi = HEX_DECODE_TABLE[*input.add(i * 2) as usize];
            let lo = HEX_DECODE_TABLE[*input.add(i * 2 + 1) as usize];

            if hi == -1 || lo == -1 {
                return -3;
            }

            *output.add(i) = ((hi as u8) << 4) | (lo as u8);
        }
    }

    out_len as i64
}

// =============================================================================
// Base64URL Encoding/Decoding (URL-safe variant)
// =============================================================================

/// URL-safe base64 alphabet
const BASE64URL_ENCODE_TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64URL decoding table
const BASE64URL_DECODE_TABLE: [u8; 256] = {
    let mut table = [255u8; 256];
    let mut i = 0u8;
    while i < 64 {
        table[BASE64URL_ENCODE_TABLE[i as usize] as usize] = i;
        i += 1;
    }
    table[b'=' as usize] = 64; // padding marker
    table
};

/// Encode bytes to URL-safe base64 (without padding)
///
/// Returns the number of bytes written, or negative on error.
///
/// # C ABI
/// ```c
/// int64_t arth_rt_base64url_encode(const uint8_t* input, size_t input_len,
///                                   uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64url_encode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    if input_len == 0 {
        return 0;
    }

    // Without padding: ceil(input_len * 4 / 3)
    let required_len = (input_len * 4).div_ceil(3);
    if output_len < required_len {
        return -1;
    }

    let input_slice = unsafe { std::slice::from_raw_parts(input, input_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };

    let mut out_idx = 0;
    let mut in_idx = 0;

    // Process 3 bytes at a time
    while in_idx + 3 <= input_len {
        let b0 = input_slice[in_idx] as usize;
        let b1 = input_slice[in_idx + 1] as usize;
        let b2 = input_slice[in_idx + 2] as usize;

        output_slice[out_idx] = BASE64URL_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64URL_ENCODE_TABLE[((b0 & 0x03) << 4) | (b1 >> 4)];
        output_slice[out_idx + 2] = BASE64URL_ENCODE_TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)];
        output_slice[out_idx + 3] = BASE64URL_ENCODE_TABLE[b2 & 0x3f];

        in_idx += 3;
        out_idx += 4;
    }

    // Handle remaining bytes (no padding)
    let remaining = input_len - in_idx;
    if remaining == 1 {
        let b0 = input_slice[in_idx] as usize;
        output_slice[out_idx] = BASE64URL_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64URL_ENCODE_TABLE[(b0 & 0x03) << 4];
        out_idx += 2;
    } else if remaining == 2 {
        let b0 = input_slice[in_idx] as usize;
        let b1 = input_slice[in_idx + 1] as usize;
        output_slice[out_idx] = BASE64URL_ENCODE_TABLE[b0 >> 2];
        output_slice[out_idx + 1] = BASE64URL_ENCODE_TABLE[((b0 & 0x03) << 4) | (b1 >> 4)];
        output_slice[out_idx + 2] = BASE64URL_ENCODE_TABLE[(b1 & 0x0f) << 2];
        out_idx += 3;
    }

    out_idx as i64
}

/// Decode URL-safe base64 to bytes
///
/// Accepts both padded and unpadded input.
///
/// Returns the number of bytes written, or negative on error.
///
/// # C ABI
/// ```c
/// int64_t arth_rt_base64url_decode(const uint8_t* input, size_t input_len,
///                                   uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_base64url_decode(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if input.is_null() || output.is_null() {
        return -2;
    }

    if input_len == 0 {
        return 0;
    }

    // Strip trailing padding
    let mut actual_len = input_len;
    while actual_len > 0 {
        let c = unsafe { *input.add(actual_len - 1) };
        if c == b'=' {
            actual_len -= 1;
        } else {
            break;
        }
    }

    if actual_len == 0 {
        return 0;
    }

    // Calculate output size based on actual content length
    let out_size = (actual_len * 3) / 4;
    if output_len < out_size {
        return -1;
    }

    let input_slice = unsafe { std::slice::from_raw_parts(input, actual_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, out_size) };

    let mut out_idx = 0;
    let mut accum: u32 = 0;
    let mut accum_bits = 0;

    for &c in input_slice {
        let val = BASE64URL_DECODE_TABLE[c as usize];
        if val == 255 {
            return -3; // Invalid character
        }
        if val == 64 {
            break; // Padding
        }

        accum = (accum << 6) | (val as u32);
        accum_bits += 6;

        if accum_bits >= 8 {
            accum_bits -= 8;
            output_slice[out_idx] = ((accum >> accum_bits) & 0xff) as u8;
            out_idx += 1;
        }
    }

    out_idx as i64
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode_len() {
        assert_eq!(arth_rt_base64_encode_len(0), 0);
        assert_eq!(arth_rt_base64_encode_len(1), 4);
        assert_eq!(arth_rt_base64_encode_len(2), 4);
        assert_eq!(arth_rt_base64_encode_len(3), 4);
        assert_eq!(arth_rt_base64_encode_len(4), 8);
        assert_eq!(arth_rt_base64_encode_len(6), 8);
    }

    #[test]
    fn test_base64_encode() {
        let input = b"Hello, World!";
        let expected = b"SGVsbG8sIFdvcmxkIQ==";

        let mut output = vec![0u8; arth_rt_base64_encode_len(input.len())];
        let len = arth_rt_base64_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, expected.len() as i64);
        assert_eq!(&output[..len as usize], expected.as_slice());
    }

    #[test]
    fn test_base64_encode_empty() {
        let input = b"";
        let mut output = vec![0u8; 4];
        let len = arth_rt_base64_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );
        assert_eq!(len, 0);
    }

    #[test]
    fn test_base64_encode_1_byte() {
        let input = b"M";
        let expected = b"TQ==";

        let mut output = vec![0u8; 4];
        let len = arth_rt_base64_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, 4);
        assert_eq!(&output[..], expected.as_slice());
    }

    #[test]
    fn test_base64_encode_2_bytes() {
        let input = b"Ma";
        let expected = b"TWE=";

        let mut output = vec![0u8; 4];
        let len = arth_rt_base64_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, 4);
        assert_eq!(&output[..], expected.as_slice());
    }

    #[test]
    fn test_base64_decode() {
        let input = b"SGVsbG8sIFdvcmxkIQ==";
        let expected = b"Hello, World!";

        let mut output = vec![0u8; arth_rt_base64_decode_len(input.len())];
        let len = arth_rt_base64_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, expected.len() as i64);
        assert_eq!(&output[..len as usize], expected.as_slice());
    }

    #[test]
    fn test_base64_decode_no_padding() {
        let input = b"TWFu"; // "Man"
        let expected = b"Man";

        let mut output = vec![0u8; arth_rt_base64_decode_len(input.len())];
        let len = arth_rt_base64_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, expected.len() as i64);
        assert_eq!(&output[..len as usize], expected.as_slice());
    }

    #[test]
    fn test_base64_roundtrip() {
        let original = b"The quick brown fox jumps over the lazy dog.";

        let enc_len = arth_rt_base64_encode_len(original.len());
        let mut encoded = vec![0u8; enc_len];
        let elen = arth_rt_base64_encode(
            original.as_ptr(),
            original.len(),
            encoded.as_mut_ptr(),
            encoded.len(),
        );
        assert!(elen > 0);

        let dec_len = arth_rt_base64_decode_len(elen as usize);
        let mut decoded = vec![0u8; dec_len];
        let dlen = arth_rt_base64_decode(
            encoded.as_ptr(),
            elen as usize,
            decoded.as_mut_ptr(),
            decoded.len(),
        );

        assert_eq!(dlen, original.len() as i64);
        assert_eq!(&decoded[..dlen as usize], original.as_slice());
    }

    // =========================================================================
    // Hex Encoding Tests
    // =========================================================================

    #[test]
    fn test_hex_encode() {
        let input = b"\xde\xad\xbe\xef";
        let expected = b"deadbeef";

        let mut output = vec![0u8; arth_rt_hex_encode_len(input.len())];
        let len = arth_rt_hex_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, 8);
        assert_eq!(&output[..8], expected.as_slice());
    }

    #[test]
    fn test_hex_decode() {
        let input = b"deadbeef";
        let expected = b"\xde\xad\xbe\xef";

        let mut output = vec![0u8; arth_rt_hex_decode_len(input.len())];
        let len = arth_rt_hex_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, 4);
        assert_eq!(&output[..4], expected.as_slice());
    }

    #[test]
    fn test_hex_decode_uppercase() {
        let input = b"DEADBEEF";
        let expected = b"\xde\xad\xbe\xef";

        let mut output = vec![0u8; 4];
        let len = arth_rt_hex_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );

        assert_eq!(len, 4);
        assert_eq!(&output, expected);
    }

    #[test]
    fn test_hex_decode_invalid() {
        let input = b"deadbeeZ"; // Z is invalid
        let mut output = vec![0u8; 4];
        let len = arth_rt_hex_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );
        assert_eq!(len, -3); // Invalid character error
    }

    #[test]
    fn test_hex_decode_odd_length() {
        let input = b"deadbee"; // Odd length
        let mut output = vec![0u8; 4];
        let len = arth_rt_hex_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );
        assert_eq!(len, -4); // Odd length error
    }

    #[test]
    fn test_hex_roundtrip() {
        let original = b"\x00\x11\x22\x33\x44\x55\x66\x77\x88\x99\xaa\xbb\xcc\xdd\xee\xff";

        let enc_len = arth_rt_hex_encode_len(original.len());
        let mut encoded = vec![0u8; enc_len];
        let elen = arth_rt_hex_encode(
            original.as_ptr(),
            original.len(),
            encoded.as_mut_ptr(),
            encoded.len(),
        );
        assert_eq!(elen, 32);

        let dec_len = arth_rt_hex_decode_len(elen as usize);
        let mut decoded = vec![0u8; dec_len];
        let dlen = arth_rt_hex_decode(
            encoded.as_ptr(),
            elen as usize,
            decoded.as_mut_ptr(),
            decoded.len(),
        );

        assert_eq!(dlen, original.len() as i64);
        assert_eq!(&decoded[..dlen as usize], original.as_slice());
    }

    // =========================================================================
    // Base64URL Encoding Tests
    // =========================================================================

    #[test]
    fn test_base64url_encode() {
        // Test with data that would have + and / in standard base64
        let input = b"\xfb\xff\xfe";

        let mut output = vec![0u8; 8];
        let len = arth_rt_base64url_encode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );
        assert!(len > 0);

        // URL-safe should use - instead of + and _ instead of /
        let encoded_str = std::str::from_utf8(&output[..len as usize]).unwrap();
        assert!(!encoded_str.contains('+'));
        assert!(!encoded_str.contains('/'));
    }

    #[test]
    fn test_base64url_decode() {
        // Test data encoded with URL-safe alphabet
        let input = b"--__"; // Would be ++ // in standard base64
        let mut output = vec![0u8; 8];
        let len = arth_rt_base64url_decode(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        );
        assert!(len > 0);
    }

    #[test]
    fn test_base64url_roundtrip() {
        let original = b"Test data with special characters: <>?&";

        let enc_len = (original.len() * 4 + 2) / 3;
        let mut encoded = vec![0u8; enc_len];
        let elen = arth_rt_base64url_encode(
            original.as_ptr(),
            original.len(),
            encoded.as_mut_ptr(),
            encoded.len(),
        );
        assert!(elen > 0);

        let mut decoded = vec![0u8; original.len()];
        let dlen = arth_rt_base64url_decode(
            encoded.as_ptr(),
            elen as usize,
            decoded.as_mut_ptr(),
            decoded.len(),
        );

        assert_eq!(dlen, original.len() as i64);
        assert_eq!(&decoded[..dlen as usize], original.as_slice());
    }
}
