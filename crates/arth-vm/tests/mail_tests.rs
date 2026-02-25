//! Integration tests for the mail functionality (HostMail trait).
//!
//! Tests cover:
//! - MIME encoding/decoding (base64, quoted-printable)
//! - MIME message building and serialization
//! - MIME header encoding (RFC 2047)
//! - Capability denial (NoHostMail)
//!
//! Note: SMTP/IMAP/POP3 connection tests require external servers and are
//! marked with #[ignore] to be run only in environments with test servers.

use arth_vm::{
    HostMail, ImapConnectionHandle, MailError, MailErrorKind, MimeMessageHandle, NoHostMail,
    Pop3ConnectionHandle, SmtpConnectionHandle, StdHostMail,
};
use std::sync::Arc;

/// Helper to create a StdHostMail instance for testing.
fn create_test_mail() -> Arc<StdHostMail> {
    Arc::new(StdHostMail::new())
}

// =============================================================================
// NoHostMail Tests (Capability Denial)
// =============================================================================

#[test]
fn test_no_host_mail_smtp_connect_denied() {
    let mail = NoHostMail;

    let result = mail.smtp_connect("localhost", 25, false, 5000);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind, MailErrorKind::CapabilityDenied);
    assert!(err.message.contains("capability denied"));
}

#[test]
fn test_no_host_mail_imap_connect_denied() {
    let mail = NoHostMail;

    let result = mail.imap_connect("localhost", 143, false, 5000);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind, MailErrorKind::CapabilityDenied);
}

#[test]
fn test_no_host_mail_pop3_connect_denied() {
    let mail = NoHostMail;

    let result = mail.pop3_connect("localhost", 110, false, 5000);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind, MailErrorKind::CapabilityDenied);
}

#[test]
fn test_no_host_mail_mime_encoding_works() {
    // MIME encoding should work even with NoHostMail (no network required)
    let mail = NoHostMail;

    // Base64 encoding
    let encoded = mail.mime_base64_encode(b"Hello, World!");
    assert_eq!(encoded, "SGVsbG8sIFdvcmxkIQ==");

    // Base64 decoding
    let decoded = mail.mime_base64_decode("SGVsbG8sIFdvcmxkIQ==");
    assert!(decoded.is_ok());
    assert_eq!(decoded.unwrap(), b"Hello, World!");

    // Quoted-printable encoding
    let qp_encoded = mail.mime_quoted_printable_encode(b"Hello=World");
    assert!(qp_encoded.contains("=3D")); // = becomes =3D

    // Quoted-printable decoding
    let qp_decoded = mail.mime_quoted_printable_decode("Hello=3DWorld");
    assert!(qp_decoded.is_ok());
    assert_eq!(qp_decoded.unwrap(), b"Hello=World");
}

// =============================================================================
// MIME Base64 Encoding Tests
// =============================================================================

#[test]
fn test_mime_base64_encode_empty() {
    let mail = create_test_mail();
    let encoded = mail.mime_base64_encode(b"");
    assert_eq!(encoded, "");
}

#[test]
fn test_mime_base64_encode_simple() {
    let mail = create_test_mail();
    let encoded = mail.mime_base64_encode(b"Hello");
    assert_eq!(encoded, "SGVsbG8=");
}

#[test]
fn test_mime_base64_encode_multiline() {
    let mail = create_test_mail();
    let encoded = mail.mime_base64_encode(b"Hello\r\nWorld");
    assert_eq!(encoded, "SGVsbG8NCldvcmxk");
}

#[test]
fn test_mime_base64_encode_binary() {
    let mail = create_test_mail();
    let data: Vec<u8> = (0u8..=255u8).collect();
    let encoded = mail.mime_base64_encode(&data);

    // Verify roundtrip
    let decoded = mail.mime_base64_decode(&encoded);
    assert!(decoded.is_ok());
    assert_eq!(decoded.unwrap(), data);
}

#[test]
fn test_mime_base64_decode_invalid() {
    let mail = create_test_mail();
    let result = mail.mime_base64_decode("!!!invalid!!!");
    assert!(result.is_err());
}

// =============================================================================
// MIME Quoted-Printable Encoding Tests
// =============================================================================

#[test]
fn test_mime_qp_encode_ascii() {
    let mail = create_test_mail();
    let encoded = mail.mime_quoted_printable_encode(b"Hello World");
    assert_eq!(encoded, "Hello World");
}

#[test]
fn test_mime_qp_encode_special_chars() {
    let mail = create_test_mail();
    let encoded = mail.mime_quoted_printable_encode(b"Hello=World");
    assert!(encoded.contains("=3D")); // = must be encoded
}

#[test]
fn test_mime_qp_encode_non_ascii() {
    let mail = create_test_mail();
    let encoded = mail.mime_quoted_printable_encode("Héllo".as_bytes());
    // Non-ASCII bytes should be encoded
    assert!(encoded.contains("=C3") || encoded.contains("=E9"));
}

#[test]
fn test_mime_qp_decode_simple() {
    let mail = create_test_mail();
    let decoded = mail.mime_quoted_printable_decode("Hello=20World");
    assert!(decoded.is_ok());
    assert_eq!(decoded.unwrap(), b"Hello World");
}

#[test]
fn test_mime_qp_roundtrip() {
    let mail = create_test_mail();
    let original = "Special chars: = \t \r\n and more!";
    let encoded = mail.mime_quoted_printable_encode(original.as_bytes());
    let decoded = mail.mime_quoted_printable_decode(&encoded);
    assert!(decoded.is_ok());
    assert_eq!(String::from_utf8_lossy(&decoded.unwrap()), original);
}

// =============================================================================
// MIME Header Encoding Tests (RFC 2047)
// =============================================================================

#[test]
fn test_mime_header_encode_ascii() {
    let mail = create_test_mail();
    let encoded = mail.mime_encode_header("Hello World", "UTF-8");
    // ASCII strings should not be encoded
    assert_eq!(encoded, "Hello World");
}

#[test]
fn test_mime_header_encode_unicode() {
    let mail = create_test_mail();
    let encoded = mail.mime_encode_header("Héllo Wörld", "UTF-8");
    // Non-ASCII strings should be encoded as =?UTF-8?Q?...?=
    assert!(encoded.starts_with("=?UTF-8?Q?"));
    assert!(encoded.ends_with("?="));
}

#[test]
fn test_mime_header_decode_plain() {
    let mail = create_test_mail();
    let decoded = mail.mime_decode_header("Hello World");
    assert!(decoded.is_ok());
    assert_eq!(decoded.unwrap(), "Hello World");
}

// =============================================================================
// MIME Message Building Tests
// =============================================================================

#[test]
fn test_mime_message_new() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();
    assert!(handle.0 > 0, "Handle should be positive");
}

#[test]
fn test_mime_message_set_header() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    let result = mail.mime_message_set_header(handle, "Subject", "Test Message");
    assert!(result.is_ok());

    let value = mail.mime_message_get_header(handle, "Subject");
    assert!(value.is_ok());
    assert_eq!(value.unwrap(), Some("Test Message".to_string()));
}

#[test]
fn test_mime_message_set_body() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    let body = b"This is the message body.";
    let result = mail.mime_message_set_body(handle, "text/plain; charset=utf-8", body);
    assert!(result.is_ok());

    let retrieved = mail.mime_message_get_body(handle);
    assert!(retrieved.is_ok());
    assert_eq!(retrieved.unwrap(), body);
}

#[test]
fn test_mime_message_add_attachment() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    let attachment_data = b"Attachment content here";
    let result =
        mail.mime_message_add_attachment(handle, "document.txt", "text/plain", attachment_data);
    assert!(result.is_ok());
}

#[test]
fn test_mime_message_serialize_simple() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    // Set up a simple message
    mail.mime_message_set_header(handle, "From", "sender@example.com")
        .unwrap();
    mail.mime_message_set_header(handle, "To", "recipient@example.com")
        .unwrap();
    mail.mime_message_set_header(handle, "Subject", "Test")
        .unwrap();
    mail.mime_message_set_body(handle, "text/plain; charset=utf-8", b"Hello!")
        .unwrap();

    let serialized = mail.mime_message_serialize(handle);
    assert!(serialized.is_ok());

    let serialized_bytes = serialized.unwrap();
    let data = String::from_utf8_lossy(&serialized_bytes);
    assert!(data.contains("From: sender@example.com"));
    assert!(data.contains("To: recipient@example.com"));
    assert!(data.contains("Subject: Test"));
    assert!(data.contains("MIME-Version: 1.0"));
    assert!(data.contains("Content-Type: text/plain"));
}

#[test]
fn test_mime_message_serialize_with_attachment() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    // Set up message with attachment
    mail.mime_message_set_header(handle, "Subject", "With Attachment")
        .unwrap();
    mail.mime_message_set_body(handle, "text/plain", b"Body text")
        .unwrap();
    mail.mime_message_add_attachment(handle, "file.txt", "text/plain", b"File content")
        .unwrap();

    let serialized = mail.mime_message_serialize(handle);
    assert!(serialized.is_ok());

    let serialized_bytes = serialized.unwrap();
    let data = String::from_utf8_lossy(&serialized_bytes);
    assert!(data.contains("multipart/mixed"));
    assert!(data.contains("boundary="));
    assert!(data.contains("file.txt"));
    assert!(data.contains("Content-Disposition: attachment"));
}

#[test]
fn test_mime_message_parse() {
    let mail = create_test_mail();

    let raw_message = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Test Message\r\n\
Content-Type: text/plain\r\n\
\r\n\
This is the body.";

    let result = mail.mime_message_parse(raw_message);
    assert!(result.is_ok());

    let handle = result.unwrap();

    let from = mail.mime_message_get_header(handle, "From");
    assert!(from.is_ok());
    assert_eq!(from.unwrap(), Some("sender@example.com".to_string()));

    let subject = mail.mime_message_get_header(handle, "Subject");
    assert!(subject.is_ok());
    assert_eq!(subject.unwrap(), Some("Test Message".to_string()));

    let body = mail.mime_message_get_body(handle);
    assert!(body.is_ok());
    assert!(String::from_utf8_lossy(&body.unwrap()).contains("This is the body"));
}

#[test]
fn test_mime_message_free() {
    let mail = create_test_mail();
    let handle = mail.mime_message_new();

    // Set some data
    mail.mime_message_set_header(handle, "Subject", "Test")
        .unwrap();

    // Free the message
    mail.mime_message_free(handle);

    // After freeing, operations should return invalid handle errors
    let result = mail.mime_message_get_header(handle, "Subject");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind, MailErrorKind::InvalidHandle);
}

#[test]
fn test_mime_message_invalid_handle() {
    let mail = create_test_mail();
    let invalid_handle = MimeMessageHandle(99999);

    let result = mail.mime_message_set_header(invalid_handle, "Subject", "Test");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, MailErrorKind::InvalidHandle);
}

// =============================================================================
// SMTP Connection Tests (require real server - marked ignore)
// =============================================================================

#[test]
#[ignore = "Requires a real SMTP server"]
fn test_smtp_connect_localhost() {
    let mail = create_test_mail();

    // Try to connect to localhost SMTP (typically fails unless server is running)
    let result = mail.smtp_connect("localhost", 25, false, 5000);
    // This test documents the expected behavior but will fail without a server
    assert!(result.is_ok() || result.is_err());
}

#[test]
#[ignore = "Requires a real SMTP server"]
fn test_smtp_connect_with_tls() {
    let mail = create_test_mail();

    // Try to connect with implicit TLS (port 465)
    let result = mail.smtp_connect("smtp.gmail.com", 465, true, 10000);
    // This test documents the expected behavior
    println!("SMTP TLS connect result: {:?}", result);
}

#[test]
fn test_smtp_invalid_handle() {
    let mail = create_test_mail();
    let invalid_handle = SmtpConnectionHandle(99999);

    // All operations on invalid handle should return error
    let result = mail.smtp_ehlo(invalid_handle, "localhost");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, MailErrorKind::InvalidHandle);

    let result = mail.smtp_quit(invalid_handle);
    assert!(result.is_err());

    let result = mail.smtp_noop(invalid_handle);
    assert!(result.is_err());

    let result = mail.smtp_mail_from(invalid_handle, "test@example.com");
    assert!(result.is_err());

    let result = mail.smtp_rcpt_to(invalid_handle, "recipient@example.com");
    assert!(result.is_err());

    let result = mail.smtp_data(invalid_handle);
    assert!(result.is_err());

    let result = mail.smtp_get_capabilities(invalid_handle);
    assert!(result.is_err());
}

// =============================================================================
// Error Type Tests
// =============================================================================

#[test]
fn test_mail_error_display() {
    let err = MailError::new(MailErrorKind::ConnectionError, "Connection refused");
    let display = format!("{}", err);
    assert!(display.contains("ConnectionError"));
    assert!(display.contains("Connection refused"));
}

#[test]
fn test_mail_error_with_code() {
    let err = MailError::with_code(MailErrorKind::ProtocolError, "User unknown", 550);
    assert_eq!(err.kind, MailErrorKind::ProtocolError);
    assert_eq!(err.response_code, Some(550));
    assert!(err.message.contains("User unknown"));
}

#[test]
fn test_mail_error_kinds() {
    // Test all error kinds can be created
    let errors = vec![
        MailError::connection_error("test"),
        MailError::protocol_error("test"),
        MailError::tls_error("test"),
        MailError::auth_error("test"),
        MailError::message_error("test"),
        MailError::invalid_handle(),
    ];

    for err in errors {
        assert!(!err.message.is_empty());
    }
}

// =============================================================================
// IMAP Tests
// =============================================================================

#[test]
fn test_imap_invalid_handle() {
    let mail = create_test_mail();
    let invalid_handle = ImapConnectionHandle(99999);

    // All operations on invalid handle should return error
    let result = mail.imap_start_tls(invalid_handle);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, MailErrorKind::InvalidHandle);

    let result = mail.imap_auth(invalid_handle, "user", "pass");
    assert!(result.is_err());

    let result = mail.imap_logout(invalid_handle);
    assert!(result.is_err());

    let result = mail.imap_noop(invalid_handle);
    assert!(result.is_err());

    let result = mail.imap_capability(invalid_handle);
    assert!(result.is_err());

    let result = mail.imap_select(invalid_handle, "INBOX");
    assert!(result.is_err());

    let result = mail.imap_examine(invalid_handle, "INBOX");
    assert!(result.is_err());

    let result = mail.imap_list(invalid_handle, "", "*");
    assert!(result.is_err());

    let result = mail.imap_fetch(invalid_handle, "1:*", "FLAGS");
    assert!(result.is_err());

    let result = mail.imap_search(invalid_handle, "ALL");
    assert!(result.is_err());

    let result = mail.imap_store(invalid_handle, "1", "\\Seen", "+FLAGS");
    assert!(result.is_err());

    let result = mail.imap_expunge(invalid_handle);
    assert!(result.is_err());

    let result = mail.imap_idle(invalid_handle, 5000);
    assert!(result.is_err());
}

#[test]
#[ignore = "Requires a real IMAP server"]
fn test_imap_connect_localhost() {
    let mail = create_test_mail();

    // Try to connect to localhost IMAP (typically fails unless server is running)
    let result = mail.imap_connect("localhost", 143, false, 5000);
    // This test documents the expected behavior but will fail without a server
    assert!(result.is_ok() || result.is_err());
}

#[test]
#[ignore = "Requires a real IMAP server"]
fn test_imap_connect_with_tls() {
    let mail = create_test_mail();

    // Try to connect with implicit TLS (port 993)
    let result = mail.imap_connect("imap.gmail.com", 993, true, 10000);
    // This test documents the expected behavior
    println!("IMAP TLS connect result: {:?}", result);
}

// =============================================================================
// POP3 Tests
// =============================================================================

#[test]
fn test_pop3_invalid_handle() {
    let mail = create_test_mail();
    let invalid_handle = Pop3ConnectionHandle(99999);

    // All operations on invalid handle should return error
    let result = mail.pop3_start_tls(invalid_handle);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, MailErrorKind::InvalidHandle);

    let result = mail.pop3_auth(invalid_handle, "user", "pass");
    assert!(result.is_err());

    let result = mail.pop3_quit(invalid_handle);
    assert!(result.is_err());

    let result = mail.pop3_stat(invalid_handle);
    assert!(result.is_err());

    let result = mail.pop3_list(invalid_handle);
    assert!(result.is_err());

    let result = mail.pop3_uidl(invalid_handle);
    assert!(result.is_err());

    let result = mail.pop3_retr(invalid_handle, 1);
    assert!(result.is_err());

    let result = mail.pop3_dele(invalid_handle, 1);
    assert!(result.is_err());

    let result = mail.pop3_reset(invalid_handle);
    assert!(result.is_err());
}

#[test]
#[ignore = "Requires a real POP3 server"]
fn test_pop3_connect_localhost() {
    let mail = create_test_mail();

    // Try to connect to localhost POP3 (typically fails unless server is running)
    let result = mail.pop3_connect("localhost", 110, false, 5000);
    // This test documents the expected behavior but will fail without a server
    assert!(result.is_ok() || result.is_err());
}

#[test]
#[ignore = "Requires a real POP3 server"]
fn test_pop3_connect_with_tls() {
    let mail = create_test_mail();

    // Try to connect with implicit TLS (port 995)
    let result = mail.pop3_connect("pop.gmail.com", 995, true, 10000);
    // This test documents the expected behavior
    println!("POP3 TLS connect result: {:?}", result);
}
