// ============================================================================
// Host Operation Enums
// ============================================================================

/// Host IO operations for file, directory, path, and console access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostIoOp {
    // File operations
    FileOpen = 0,
    FileClose = 1,
    FileRead = 2,
    FileWrite = 3,
    FileWriteStr = 4,
    FileFlush = 5,
    FileSeek = 6,
    FileSize = 7,
    FileExists = 8,
    FileDelete = 9,
    FileCopy = 10,
    FileMove = 11,
    // Directory operations
    DirCreate = 20,
    DirCreateAll = 21,
    DirDelete = 22,
    DirList = 23,
    DirExists = 24,
    IsDir = 25,
    IsFile = 26,
    // Path operations
    PathAbsolute = 30,
    // Console operations
    ConsoleReadLine = 40,
    ConsoleWrite = 41,
    ConsoleWriteErr = 42,
}

impl HostIoOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::FileOpen),
            1 => Some(Self::FileClose),
            2 => Some(Self::FileRead),
            3 => Some(Self::FileWrite),
            4 => Some(Self::FileWriteStr),
            5 => Some(Self::FileFlush),
            6 => Some(Self::FileSeek),
            7 => Some(Self::FileSize),
            8 => Some(Self::FileExists),
            9 => Some(Self::FileDelete),
            10 => Some(Self::FileCopy),
            11 => Some(Self::FileMove),
            20 => Some(Self::DirCreate),
            21 => Some(Self::DirCreateAll),
            22 => Some(Self::DirDelete),
            23 => Some(Self::DirList),
            24 => Some(Self::DirExists),
            25 => Some(Self::IsDir),
            26 => Some(Self::IsFile),
            30 => Some(Self::PathAbsolute),
            40 => Some(Self::ConsoleReadLine),
            41 => Some(Self::ConsoleWrite),
            42 => Some(Self::ConsoleWriteErr),
            _ => None,
        }
    }
}

/// Host networking operations for HTTP, WebSocket, and SSE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostNetOp {
    // HTTP operations
    HttpFetch = 0,
    HttpServe = 1,
    HttpAccept = 2,
    HttpRespond = 3,
    // WebSocket operations
    WsServe = 10,
    WsAccept = 11,
    WsSendText = 12,
    WsSendBinary = 13,
    WsRecv = 14,
    WsClose = 15,
    WsIsOpen = 16,
    // SSE operations
    SseServe = 20,
    SseAccept = 21,
    SseSend = 22,
    SseClose = 23,
    SseIsOpen = 24,
}

impl HostNetOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::HttpFetch),
            1 => Some(Self::HttpServe),
            2 => Some(Self::HttpAccept),
            3 => Some(Self::HttpRespond),
            10 => Some(Self::WsServe),
            11 => Some(Self::WsAccept),
            12 => Some(Self::WsSendText),
            13 => Some(Self::WsSendBinary),
            14 => Some(Self::WsRecv),
            15 => Some(Self::WsClose),
            16 => Some(Self::WsIsOpen),
            20 => Some(Self::SseServe),
            21 => Some(Self::SseAccept),
            22 => Some(Self::SseSend),
            23 => Some(Self::SseClose),
            24 => Some(Self::SseIsOpen),
            _ => None,
        }
    }
}

/// Host time operations for wall-clock, monotonic clock, and timers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostTimeOp {
    // Wall-clock time
    DateTimeNow = 0,
    DateTimeParse = 1,
    DateTimeFormat = 2,
    // Monotonic clock
    InstantNow = 10,
    InstantElapsed = 11,
    // Timers
    Sleep = 20,
}

impl HostTimeOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::DateTimeNow),
            1 => Some(Self::DateTimeParse),
            2 => Some(Self::DateTimeFormat),
            10 => Some(Self::InstantNow),
            11 => Some(Self::InstantElapsed),
            20 => Some(Self::Sleep),
            _ => None,
        }
    }
}

/// Host database operations for SQLite and PostgreSQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostDbOp {
    // Connection operations (0-9)
    SqliteOpen = 0,
    SqliteClose = 1,

    // Statement operations (10-19)
    SqlitePrepare = 10,
    SqliteStep = 11,
    SqliteFinalize = 12,
    SqliteReset = 13,

    // Binding operations (20-29)
    SqliteBindInt = 20,
    SqliteBindInt64 = 21,
    SqliteBindDouble = 22,
    SqliteBindText = 23,
    SqliteBindBlob = 24,
    SqliteBindNull = 25,

    // Column access operations (30-39)
    SqliteColumnInt = 30,
    SqliteColumnInt64 = 31,
    SqliteColumnDouble = 32,
    SqliteColumnText = 33,
    SqliteColumnBlob = 34,
    SqliteColumnType = 35,
    SqliteColumnCount = 36,
    SqliteColumnName = 37,
    SqliteIsNull = 38,

    // Utility operations (40-49)
    SqliteChanges = 40,
    SqliteLastInsertRowid = 41,
    SqliteErrmsg = 42,

    // Transaction operations (50-59)
    SqliteBegin = 50,
    SqliteCommit = 51,
    SqliteRollback = 52,
    SqliteSavepoint = 53,
    SqliteReleaseSavepoint = 54,
    SqliteRollbackToSavepoint = 55,

    // High-level convenience operations (60-69)
    SqliteQuery = 60,
    SqliteExecute = 61,

    // =========================================================================
    // PostgreSQL Operations (100+)
    // =========================================================================

    // PostgreSQL connection operations (100-109)
    PgConnect = 100,
    PgDisconnect = 101,
    PgStatus = 102,

    // PostgreSQL query operations (110-119)
    PgQuery = 110,
    PgExecute = 111,
    PgPrepare = 112,
    PgExecutePrepared = 113,

    // PostgreSQL result operations (120-139)
    PgRowCount = 120,
    PgColumnCount = 121,
    PgColumnName = 122,
    PgColumnType = 123,
    PgGetValue = 124,
    PgGetInt = 125,
    PgGetInt64 = 126,
    PgGetDouble = 127,
    PgGetText = 128,
    PgGetBytes = 129,
    PgGetBool = 130,
    PgIsNull = 131,
    PgAffectedRows = 132,

    // PostgreSQL transaction operations (140-149)
    PgBegin = 140,
    PgCommit = 141,
    PgRollback = 142,
    PgSavepoint = 143,
    PgReleaseSavepoint = 144,
    PgRollbackToSavepoint = 145,

    // PostgreSQL utility operations (150-159)
    PgErrmsg = 150,
    PgEscape = 151,
    PgFreeResult = 152,

    // =========================================================================
    // Async PostgreSQL Operations (160+)
    // =========================================================================
    // These operations use tokio-postgres for non-blocking I/O.

    // Async connection operations (160-164)
    /// Connect to PostgreSQL asynchronously
    /// Returns a connection handle or error
    PgConnectAsync = 160,
    /// Disconnect async connection
    PgDisconnectAsync = 161,
    /// Check async connection status
    PgStatusAsync = 162,

    // Async query operations (165-174)
    /// Execute async query, returns a pending query handle
    PgQueryAsync = 165,
    /// Execute async statement (INSERT/UPDATE/DELETE)
    PgExecuteAsync = 166,
    /// Prepare statement asynchronously
    PgPrepareAsync = 167,
    /// Execute prepared statement asynchronously
    PgExecutePreparedAsync = 168,

    // Async result polling (175-179)
    /// Check if async query is ready
    PgIsReady = 175,
    /// Get result from completed async query
    PgGetAsyncResult = 176,
    /// Cancel an in-progress async query
    PgCancelAsync = 177,

    // Async transaction operations (180-184)
    PgBeginAsync = 180,
    PgCommitAsync = 181,
    PgRollbackAsync = 182,

    // =========================================================================
    // Connection Pool Operations (190+)
    // =========================================================================

    // SQLite pool operations (190-199)
    /// Create a new SQLite connection pool
    SqlitePoolCreate = 190,
    /// Close a SQLite connection pool
    SqlitePoolClose = 191,
    /// Acquire a connection from SQLite pool
    SqlitePoolAcquire = 192,
    /// Release a connection back to SQLite pool
    SqlitePoolRelease = 193,
    /// Get pool statistics (available, in-use, total)
    SqlitePoolStats = 194,

    // PostgreSQL pool operations (200-209)
    /// Create a new PostgreSQL connection pool
    PgPoolCreate = 200,
    /// Close a PostgreSQL connection pool
    PgPoolClose = 201,
    /// Acquire a connection from PostgreSQL pool
    PgPoolAcquire = 202,
    /// Release a connection back to PostgreSQL pool
    PgPoolRelease = 203,
    /// Get pool statistics (available, in-use, total)
    PgPoolStats = 204,

    // SQLite transaction helper operations (210-219)
    /// Begin a managed transaction scope. Returns scope ID.
    /// Automatically uses savepoints for nested transactions.
    SqliteTxScopeBegin = 210,
    /// End a managed transaction scope. Takes (conn, scope_id, success).
    /// Commits on success, rolls back on failure.
    SqliteTxScopeEnd = 211,
    /// Get current transaction depth for a connection.
    SqliteTxDepth = 212,
    /// Check if connection is in a transaction.
    SqliteTxActive = 213,

    // PostgreSQL transaction helper operations (220-229)
    /// Begin a managed transaction scope. Returns scope ID.
    /// Automatically uses savepoints for nested transactions.
    PgTxScopeBegin = 220,
    /// End a managed transaction scope. Takes (conn, scope_id, success).
    /// Commits on success, rolls back on failure.
    PgTxScopeEnd = 221,
    /// Get current transaction depth for a connection.
    PgTxDepth = 222,
    /// Check if connection is in a transaction.
    PgTxActive = 223,
}

impl HostDbOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::SqliteOpen),
            1 => Some(Self::SqliteClose),
            10 => Some(Self::SqlitePrepare),
            11 => Some(Self::SqliteStep),
            12 => Some(Self::SqliteFinalize),
            13 => Some(Self::SqliteReset),
            20 => Some(Self::SqliteBindInt),
            21 => Some(Self::SqliteBindInt64),
            22 => Some(Self::SqliteBindDouble),
            23 => Some(Self::SqliteBindText),
            24 => Some(Self::SqliteBindBlob),
            25 => Some(Self::SqliteBindNull),
            30 => Some(Self::SqliteColumnInt),
            31 => Some(Self::SqliteColumnInt64),
            32 => Some(Self::SqliteColumnDouble),
            33 => Some(Self::SqliteColumnText),
            34 => Some(Self::SqliteColumnBlob),
            35 => Some(Self::SqliteColumnType),
            36 => Some(Self::SqliteColumnCount),
            37 => Some(Self::SqliteColumnName),
            38 => Some(Self::SqliteIsNull),
            40 => Some(Self::SqliteChanges),
            41 => Some(Self::SqliteLastInsertRowid),
            42 => Some(Self::SqliteErrmsg),
            50 => Some(Self::SqliteBegin),
            51 => Some(Self::SqliteCommit),
            52 => Some(Self::SqliteRollback),
            53 => Some(Self::SqliteSavepoint),
            54 => Some(Self::SqliteReleaseSavepoint),
            55 => Some(Self::SqliteRollbackToSavepoint),
            60 => Some(Self::SqliteQuery),
            61 => Some(Self::SqliteExecute),
            // PostgreSQL
            100 => Some(Self::PgConnect),
            101 => Some(Self::PgDisconnect),
            102 => Some(Self::PgStatus),
            110 => Some(Self::PgQuery),
            111 => Some(Self::PgExecute),
            112 => Some(Self::PgPrepare),
            113 => Some(Self::PgExecutePrepared),
            120 => Some(Self::PgRowCount),
            121 => Some(Self::PgColumnCount),
            122 => Some(Self::PgColumnName),
            123 => Some(Self::PgColumnType),
            124 => Some(Self::PgGetValue),
            125 => Some(Self::PgGetInt),
            126 => Some(Self::PgGetInt64),
            127 => Some(Self::PgGetDouble),
            128 => Some(Self::PgGetText),
            129 => Some(Self::PgGetBytes),
            130 => Some(Self::PgGetBool),
            131 => Some(Self::PgIsNull),
            132 => Some(Self::PgAffectedRows),
            140 => Some(Self::PgBegin),
            141 => Some(Self::PgCommit),
            142 => Some(Self::PgRollback),
            143 => Some(Self::PgSavepoint),
            144 => Some(Self::PgReleaseSavepoint),
            145 => Some(Self::PgRollbackToSavepoint),
            150 => Some(Self::PgErrmsg),
            151 => Some(Self::PgEscape),
            152 => Some(Self::PgFreeResult),
            // Async PostgreSQL
            160 => Some(Self::PgConnectAsync),
            161 => Some(Self::PgDisconnectAsync),
            162 => Some(Self::PgStatusAsync),
            165 => Some(Self::PgQueryAsync),
            166 => Some(Self::PgExecuteAsync),
            167 => Some(Self::PgPrepareAsync),
            168 => Some(Self::PgExecutePreparedAsync),
            175 => Some(Self::PgIsReady),
            176 => Some(Self::PgGetAsyncResult),
            177 => Some(Self::PgCancelAsync),
            180 => Some(Self::PgBeginAsync),
            181 => Some(Self::PgCommitAsync),
            182 => Some(Self::PgRollbackAsync),
            // Connection pools
            190 => Some(Self::SqlitePoolCreate),
            191 => Some(Self::SqlitePoolClose),
            192 => Some(Self::SqlitePoolAcquire),
            193 => Some(Self::SqlitePoolRelease),
            194 => Some(Self::SqlitePoolStats),
            200 => Some(Self::PgPoolCreate),
            201 => Some(Self::PgPoolClose),
            202 => Some(Self::PgPoolAcquire),
            203 => Some(Self::PgPoolRelease),
            204 => Some(Self::PgPoolStats),
            // Transaction helpers
            210 => Some(Self::SqliteTxScopeBegin),
            211 => Some(Self::SqliteTxScopeEnd),
            212 => Some(Self::SqliteTxDepth),
            213 => Some(Self::SqliteTxActive),
            220 => Some(Self::PgTxScopeBegin),
            221 => Some(Self::PgTxScopeEnd),
            222 => Some(Self::PgTxDepth),
            223 => Some(Self::PgTxActive),
            _ => None,
        }
    }
}

/// Host mail operations for SMTP, IMAP, and POP3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostMailOp {
    // =========================================================================
    // SMTP Operations (0-49)
    // =========================================================================

    // SMTP connection operations (0-9)
    /// Connect to an SMTP server (TCP)
    SmtpConnect = 0,
    /// Upgrade connection to TLS (STARTTLS)
    SmtpStartTls = 1,
    /// Authenticate with SMTP server (PLAIN, LOGIN, CRAM-MD5)
    SmtpAuth = 2,
    /// Send EHLO/HELO command
    SmtpEhlo = 3,
    /// Send QUIT command and disconnect
    SmtpQuit = 4,
    /// Check if connection is alive (NOOP)
    SmtpNoop = 5,
    /// Reset session state (RSET)
    SmtpReset = 6,

    // SMTP envelope operations (10-19)
    /// Set sender (MAIL FROM)
    SmtpMailFrom = 10,
    /// Add recipient (RCPT TO)
    SmtpRcptTo = 11,
    /// Start data transfer (DATA command)
    SmtpData = 12,
    /// Send message content
    SmtpSendData = 13,
    /// End data transfer (CRLF.CRLF)
    SmtpEndData = 14,

    // SMTP response operations (20-29)
    /// Read response from server
    SmtpReadResponse = 20,
    /// Get last response code
    SmtpGetResponseCode = 21,
    /// Get last response message
    SmtpGetResponseMessage = 22,
    /// Get server capabilities from EHLO response
    SmtpGetCapabilities = 23,

    // SMTP high-level operations (30-39)
    /// Send a complete message (convenience wrapper)
    SmtpSendMessage = 30,
    /// Verify address (VRFY command)
    SmtpVerify = 31,
    /// Expand mailing list (EXPN command)
    SmtpExpand = 32,

    // =========================================================================
    // IMAP Operations (50-99)
    // =========================================================================

    // IMAP connection operations (50-59)
    /// Connect to an IMAP server (TCP)
    ImapConnect = 50,
    /// Upgrade connection to TLS (STARTTLS)
    ImapStartTls = 51,
    /// Authenticate with IMAP server
    ImapAuth = 52,
    /// Authenticate with OAuth2
    ImapAuthOAuth = 53,
    /// Logout and disconnect
    ImapLogout = 54,
    /// Send NOOP command
    ImapNoop = 55,
    /// Get server capabilities
    ImapCapability = 56,

    // IMAP mailbox operations (60-69)
    /// Select a mailbox (read-write)
    ImapSelect = 60,
    /// Examine a mailbox (read-only)
    ImapExamine = 61,
    /// Create a mailbox
    ImapCreate = 62,
    /// Delete a mailbox
    ImapDelete = 63,
    /// Rename a mailbox
    ImapRename = 64,
    /// Subscribe to a mailbox
    ImapSubscribe = 65,
    /// Unsubscribe from a mailbox
    ImapUnsubscribe = 66,
    /// List mailboxes
    ImapList = 67,
    /// List subscribed mailboxes
    ImapLsub = 68,
    /// Get mailbox status
    ImapStatus = 69,

    // IMAP message operations (70-79)
    /// Fetch message data
    ImapFetch = 70,
    /// Store message flags
    ImapStore = 71,
    /// Copy messages to another mailbox
    ImapCopy = 72,
    /// Move messages to another mailbox
    ImapMove = 73,
    /// Expunge deleted messages
    ImapExpunge = 74,
    /// Search for messages
    ImapSearch = 75,
    /// Append message to mailbox
    ImapAppend = 76,

    // IMAP IDLE operations (80-84)
    /// Enter IDLE mode
    ImapIdle = 80,
    /// Exit IDLE mode
    ImapIdleDone = 81,
    /// Check for IDLE events
    ImapIdlePoll = 82,

    // =========================================================================
    // POP3 Operations (100-149)
    // =========================================================================

    // POP3 connection operations (100-109)
    /// Connect to a POP3 server
    Pop3Connect = 100,
    /// Upgrade connection to TLS
    Pop3StartTls = 101,
    /// Authenticate with USER/PASS
    Pop3Auth = 102,
    /// Authenticate with APOP
    Pop3AuthApop = 103,
    /// Quit and disconnect
    Pop3Quit = 104,
    /// NOOP command
    Pop3Noop = 105,

    // POP3 mailbox operations (110-119)
    /// Get mailbox statistics (STAT)
    Pop3Stat = 110,
    /// List messages (LIST)
    Pop3List = 111,
    /// Get message UIDs (UIDL)
    Pop3Uidl = 112,
    /// Retrieve message (RETR)
    Pop3Retr = 113,
    /// Delete message (DELE)
    Pop3Dele = 114,
    /// Reset deletion marks (RSET)
    Pop3Reset = 115,
    /// Get message headers only (TOP)
    Pop3Top = 116,

    // =========================================================================
    // MIME Operations (150-179)
    // =========================================================================

    // MIME encoding (150-159)
    /// Base64 encode
    MimeBase64Encode = 150,
    /// Base64 decode
    MimeBase64Decode = 151,
    /// Quoted-Printable encode
    MimeQuotedPrintableEncode = 152,
    /// Quoted-Printable decode
    MimeQuotedPrintableDecode = 153,
    /// RFC 2047 encode header (Q-encoding)
    MimeEncodeHeader = 154,
    /// RFC 2047 decode header
    MimeDecodeHeader = 155,

    // MIME message building (160-169)
    /// Create a new MIME message
    MimeMessageNew = 160,
    /// Set message header
    MimeMessageSetHeader = 161,
    /// Set message body (text)
    MimeMessageSetBody = 162,
    /// Add attachment
    MimeMessageAddAttachment = 163,
    /// Add inline part
    MimeMessageAddInline = 164,
    /// Build multipart message
    MimeMessageBuildMultipart = 165,
    /// Serialize message to RFC 5322 format
    MimeMessageSerialize = 166,

    // MIME message parsing (170-179)
    /// Parse RFC 5322 message
    MimeMessageParse = 170,
    /// Get header from parsed message
    MimeMessageGetHeader = 171,
    /// Get body from parsed message
    MimeMessageGetBody = 172,
    /// Get attachments from parsed message
    MimeMessageGetAttachments = 173,
    /// Get all headers as list
    MimeMessageGetAllHeaders = 174,

    // =========================================================================
    // TLS/SSL Operations (180-189)
    // =========================================================================
    /// Create TLS context
    TlsContextNew = 180,
    /// Upgrade socket to TLS
    TlsUpgrade = 181,
    /// TLS handshake
    TlsHandshake = 182,
    /// TLS read
    TlsRead = 183,
    /// TLS write
    TlsWrite = 184,
    /// TLS close
    TlsClose = 185,
    /// Set TLS certificate
    TlsSetCert = 186,
    /// Set TLS verification mode
    TlsSetVerify = 187,
}

impl HostMailOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            // SMTP connection
            0 => Some(Self::SmtpConnect),
            1 => Some(Self::SmtpStartTls),
            2 => Some(Self::SmtpAuth),
            3 => Some(Self::SmtpEhlo),
            4 => Some(Self::SmtpQuit),
            5 => Some(Self::SmtpNoop),
            6 => Some(Self::SmtpReset),
            // SMTP envelope
            10 => Some(Self::SmtpMailFrom),
            11 => Some(Self::SmtpRcptTo),
            12 => Some(Self::SmtpData),
            13 => Some(Self::SmtpSendData),
            14 => Some(Self::SmtpEndData),
            // SMTP response
            20 => Some(Self::SmtpReadResponse),
            21 => Some(Self::SmtpGetResponseCode),
            22 => Some(Self::SmtpGetResponseMessage),
            23 => Some(Self::SmtpGetCapabilities),
            // SMTP high-level
            30 => Some(Self::SmtpSendMessage),
            31 => Some(Self::SmtpVerify),
            32 => Some(Self::SmtpExpand),
            // IMAP connection
            50 => Some(Self::ImapConnect),
            51 => Some(Self::ImapStartTls),
            52 => Some(Self::ImapAuth),
            53 => Some(Self::ImapAuthOAuth),
            54 => Some(Self::ImapLogout),
            55 => Some(Self::ImapNoop),
            56 => Some(Self::ImapCapability),
            // IMAP mailbox
            60 => Some(Self::ImapSelect),
            61 => Some(Self::ImapExamine),
            62 => Some(Self::ImapCreate),
            63 => Some(Self::ImapDelete),
            64 => Some(Self::ImapRename),
            65 => Some(Self::ImapSubscribe),
            66 => Some(Self::ImapUnsubscribe),
            67 => Some(Self::ImapList),
            68 => Some(Self::ImapLsub),
            69 => Some(Self::ImapStatus),
            // IMAP message
            70 => Some(Self::ImapFetch),
            71 => Some(Self::ImapStore),
            72 => Some(Self::ImapCopy),
            73 => Some(Self::ImapMove),
            74 => Some(Self::ImapExpunge),
            75 => Some(Self::ImapSearch),
            76 => Some(Self::ImapAppend),
            // IMAP IDLE
            80 => Some(Self::ImapIdle),
            81 => Some(Self::ImapIdleDone),
            82 => Some(Self::ImapIdlePoll),
            // POP3 connection
            100 => Some(Self::Pop3Connect),
            101 => Some(Self::Pop3StartTls),
            102 => Some(Self::Pop3Auth),
            103 => Some(Self::Pop3AuthApop),
            104 => Some(Self::Pop3Quit),
            105 => Some(Self::Pop3Noop),
            // POP3 mailbox
            110 => Some(Self::Pop3Stat),
            111 => Some(Self::Pop3List),
            112 => Some(Self::Pop3Uidl),
            113 => Some(Self::Pop3Retr),
            114 => Some(Self::Pop3Dele),
            115 => Some(Self::Pop3Reset),
            116 => Some(Self::Pop3Top),
            // MIME encoding
            150 => Some(Self::MimeBase64Encode),
            151 => Some(Self::MimeBase64Decode),
            152 => Some(Self::MimeQuotedPrintableEncode),
            153 => Some(Self::MimeQuotedPrintableDecode),
            154 => Some(Self::MimeEncodeHeader),
            155 => Some(Self::MimeDecodeHeader),
            // MIME message building
            160 => Some(Self::MimeMessageNew),
            161 => Some(Self::MimeMessageSetHeader),
            162 => Some(Self::MimeMessageSetBody),
            163 => Some(Self::MimeMessageAddAttachment),
            164 => Some(Self::MimeMessageAddInline),
            165 => Some(Self::MimeMessageBuildMultipart),
            166 => Some(Self::MimeMessageSerialize),
            // MIME message parsing
            170 => Some(Self::MimeMessageParse),
            171 => Some(Self::MimeMessageGetHeader),
            172 => Some(Self::MimeMessageGetBody),
            173 => Some(Self::MimeMessageGetAttachments),
            174 => Some(Self::MimeMessageGetAllHeaders),
            // TLS operations
            180 => Some(Self::TlsContextNew),
            181 => Some(Self::TlsUpgrade),
            182 => Some(Self::TlsHandshake),
            183 => Some(Self::TlsRead),
            184 => Some(Self::TlsWrite),
            185 => Some(Self::TlsClose),
            186 => Some(Self::TlsSetCert),
            187 => Some(Self::TlsSetVerify),
            _ => None,
        }
    }
}

// ============================================================================
// VM Opcodes
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Print(u32),
    // Print a string followed by the top-of-stack numeric/bool on one line
    PrintStrVal(u32),
    // Raw prints (no newline)
    PrintRaw(u32),
    PrintRawStrVal(u32),
    PrintLn,
    Halt,
    // Stack-based expression ops
    PushI64(i64),
    PushF64(f64),
    PushBool(u8),
    PushStr(u32),
    AddI64,
    SubI64,
    MulI64,
    DivI64,
    ModI64,
    LtI64,
    EqI64,
    EqStr,     // String equality: str1, str2 -> 1 if equal, 0 otherwise
    ConcatStr, // String concatenation: str1, str2 -> new_string
    ShlI64,
    ShrI64,
    AndI64,
    OrI64,
    XorI64,
    Jump(u32),
    JumpIfFalse(u32),
    // Function calls
    Call(u32),
    /// Call a function by symbolic name (for cross-library calls).
    ///
    /// The symbol name is looked up in the program string table at `sym`.
    /// At runtime, the symbol is resolved via the linked symbol table to
    /// get the actual bytecode offset, then the function is called.
    ///
    /// This is used for calls to functions in external packages where the
    /// offset isn't known at compile time.
    CallSymbol(u32),
    /// Call an external (host) symbol using the platform C ABI.
    ///
    /// The symbol name is looked up in the program string table at `sym`.
    /// Arguments are popped from the stack and the return value is pushed.
    ///
    /// Encoding:
    /// - `argc`: number of arguments to pop.
    /// - `float_mask`: bit i is 1 if arg i is `f64`, else integer-like (`i64`).
    /// - `ret_kind`: 0 = integer-like (`i64`), 1 = `f64`, 2 = `void` (pushes `0`).
    ExternCall {
        sym: u32,
        argc: u8,
        float_mask: u32,
        ret_kind: u8,
    },
    Ret,
    Pop,
    PrintTop,
    LocalGet(u32),
    LocalSet(u32),
    // Numeric conversions
    ToF64,
    ToI64,
    ToI64OrEnumTag,
    ToBool,
    ToChar,
    ToI8,
    ToI16,
    ToI32,
    ToU8,
    ToU16,
    ToU32,
    ToU64,
    ToF32,
    // Float math (stack-based; operate on top values)
    SqrtF64,  // a -> sqrt(a)
    PowF64,   // a, b -> a^b
    SinF64,   // a -> sin(a)
    CosF64,   // a -> cos(a)
    TanF64,   // a -> tan(a)
    FloorF64, // a -> floor(a)
    CeilF64,  // a -> ceil(a)
    RoundF64, // a -> round(a)
    // Removed: RoundF64N, MinF64, MaxF64, ClampF64, AbsF64, MinI64, MaxI64, ClampI64, AbsI64
    // These are now pure Arth in stdlib/src/math/Math.arth
    // Collections (MVP): list and map core primitives using i64 handles/values
    // List operations (7 core intrinsics)
    // Removed: ListIndexOf, ListContains, ListInsert, ListClear, ListReverse, ListConcat, ListSlice, ListUnique
    // These are now pure Arth code in stdlib/src/arth/array.arth
    ListNew,    // -> handle
    ListPush,   // list, value -> new_len
    ListGet,    // list, index -> value (panics if OOB)
    ListSet,    // list, index, value -> 0 (sets in-place, panics if OOB)
    ListLen,    // list -> len
    ListRemove, // list, index -> removed_value (panics if OOB)
    ListSort,   // list -> 0 (sorts in-place)

    // Map operations (7 core intrinsics)
    // Removed: MapContainsValue, MapClear, MapIsEmpty, MapGetOrDefault, MapValues
    // These are now pure Arth code in stdlib/src/arth/map.arth
    MapNew,         // -> handle
    MapPut,         // map, key, value -> 0 (status)
    MapGet,         // map, key -> value (0 if missing)
    MapLen,         // map -> len
    MapContainsKey, // map, key -> 1 if found, 0 otherwise
    MapRemove,      // map, key -> removed_value (0 if missing)
    MapKeys,        // map -> list_handle of keys
    MapMerge,       // dest_map, src_map -> dest_map (copies all entries from src to dest)

    // String operations (18 core intrinsics)
    // These implement the Strings module functions from stdlib.
    // Strings are stored as indices into the program string pool.
    StrLen,         // str -> len (length in characters)
    StrSubstring,   // str, start, end -> new_str (substring from start to end)
    StrIndexOf,     // str, search -> index (-1 if not found)
    StrLastIndexOf, // str, search -> index (-1 if not found)
    StrStartsWith,  // str, prefix -> 1 if starts with prefix, 0 otherwise
    StrEndsWith,    // str, suffix -> 1 if ends with suffix, 0 otherwise
    StrSplit,       // str, delimiter -> list_handle of strings
    StrTrim,        // str -> new_str (whitespace trimmed)
    StrToLower,     // str -> new_str (lowercase)
    StrToUpper,     // str -> new_str (uppercase)
    StrReplace,     // str, old, new -> new_str (all occurrences replaced)
    StrCharAt,      // str, index -> char_code (panics if out of bounds)
    StrContains,    // str, search -> 1 if contains, 0 otherwise
    StrRepeat,      // str, count -> new_str (repeated count times)
    // Note: ConcatStr already exists for string concatenation
    StrParseInt,   // str -> int (0 on parse failure, sets error flag)
    StrParseFloat, // str -> float (0.0 on parse failure, sets error flag)
    StrFromInt,    // int -> str (decimal representation)
    StrFromFloat,  // float -> str (decimal representation)

    // Optional operations (5 core intrinsics)
    // Optional<T> represents an optional value - either Some(value) or None
    OptSome,   // value -> optional_handle (creates Some(value))
    OptNone,   // -> optional_handle (creates None)
    OptIsSome, // optional -> 1 if Some, 0 if None
    OptUnwrap, // optional -> value (returns 0 if None)
    OptOrElse, // optional, default -> value (returns default if None)

    // Native struct operations (replacing map-based representation)
    // Structs are stored as typed, indexed arrays with field name metadata
    StructNew,        // type_name_idx, field_count -> struct_handle
    StructSet,        // struct_handle, field_idx, value, field_name_idx -> 0
    StructGet,        // struct_handle, field_idx -> value
    StructGetNamed,   // struct_handle, field_name_idx -> value (field lookup by name)
    StructSetNamed,   // struct_handle, field_name_idx, value -> 0 (field set by name)
    StructCopy,       // dest_handle, src_handle -> 0 (copy fields from src to dest)
    StructTypeName,   // struct_handle -> type_name_idx
    StructFieldCount, // struct_handle -> field_count

    // Native enum operations (replacing list-based representation)
    // Enums are stored with type info, variant tag, and payload array
    EnumNew,        // enum_name_idx, variant_name_idx, tag, payload_count -> enum_handle
    EnumSetPayload, // enum_handle, payload_idx, value -> 0
    EnumGetPayload, // enum_handle, payload_idx -> value
    EnumGetTag,     // enum_handle -> tag (i64)
    EnumGetVariant, // enum_handle -> variant_name_idx
    EnumTypeName,   // enum_handle -> enum_name_idx

    // HTTP client/server operations have been migrated to HostCallNet opcodes.
    // See HostNetOp enum for HttpFetch, HttpServe, HttpAccept, HttpRespond.
    // JSON serialization operations (encoding.json package)
    // Stringify: converts a value (primitive, list handle, or map handle) to JSON string
    JsonStringify, // value -> json_string (pushes string to stack)
    // Parse: parses a JSON string into a JsonValue handle
    JsonParse, // json_string -> json_value_handle (-1 on error)
    // Struct JSON serialization (for @derive(JsonCodec))
    // Serializes a struct (list handle) to JSON using field names
    StructToJson, // struct_handle, field_names_str -> json_string
    // Deserializes JSON to a struct (list handle) using field names
    JsonToStruct, // json_string, field_names_str -> struct_handle (-1 on error)

    // JSON value accessor operations (for accessing parsed JSON)
    // These work on handles returned by JsonParse
    JsonGetField, // json_handle, key_str -> value_handle (-1 if not found or not object)
    JsonGetIndex, // json_handle, index -> value_handle (-1 if out of bounds or not array)
    JsonGetString, // json_handle -> string (empty if not a string)
    JsonGetNumber, // json_handle -> f64 (0.0 if not a number)
    JsonGetBool,  // json_handle -> i64 (0 or 1, 0 if not a bool)
    JsonIsNull,   // json_handle -> i64 (1 if null, 0 otherwise)
    JsonIsObject, // json_handle -> i64 (1 if object, 0 otherwise)
    JsonIsArray,  // json_handle -> i64 (1 if array, 0 otherwise)
    JsonArrayLen, // json_handle -> i64 (length if array, -1 otherwise)
    JsonKeys,     // json_handle -> list_handle (list of key strings, empty if not object)

    // HTML parsing operations (markup package)
    // Parse HTML string into DOM handle
    HtmlParse,           // html_string -> document_handle (-1 on error)
    HtmlParseFragment,   // html_string -> fragment_handle (-1 on error)
    HtmlStringify,       // handle -> html_string
    HtmlStringifyPretty, // handle, indent -> html_string
    HtmlFree,            // handle -> 0 (frees DOM memory)
    // Node properties
    HtmlNodeType,    // handle -> node_type (1=element, 3=text, 8=comment, 9=document)
    HtmlTagName,     // handle -> tag_string (empty for non-elements)
    HtmlTextContent, // handle -> text_string
    HtmlInnerHtml,   // handle -> inner_html_string
    HtmlOuterHtml,   // handle -> outer_html_string
    // Attributes
    HtmlGetAttr,   // handle, attr_name -> attr_value (empty if not found)
    HtmlHasAttr,   // handle, attr_name -> 1 if exists, 0 otherwise
    HtmlAttrNames, // handle -> list_handle of attribute names
    // Tree navigation
    HtmlParent,          // handle -> parent_handle (0 if root)
    HtmlChildren,        // handle -> list_handle of child handles (all nodes)
    HtmlElementChildren, // handle -> list_handle of element child handles only
    HtmlFirstChild,      // handle -> first_child_handle (0 if none)
    HtmlLastChild,       // handle -> last_child_handle (0 if none)
    HtmlNextSibling,     // handle -> next_sibling_handle (0 if none)
    HtmlPrevSibling,     // handle -> prev_sibling_handle (0 if none)
    // Query methods
    HtmlQuerySelector,    // handle, selector -> first_match_handle (0 if not found)
    HtmlQuerySelectorAll, // handle, selector -> list_handle of matches
    HtmlGetById,          // handle, id -> element_handle (0 if not found)
    HtmlGetByTag,         // handle, tag -> list_handle of elements
    HtmlGetByClass,       // handle, class -> list_handle of elements
    HtmlHasClass,         // handle, class -> 1 if has class, 0 otherwise

    // Template engine operations (template package)
    // Compile HTML string with data-* directives into template handle
    TemplateCompile,           // html_string -> template_handle (-1 on error)
    TemplateCompileFile,       // file_path -> template_handle (-1 on error)
    TemplateRender,            // template_handle, context_map_handle -> html_string
    TemplateRegisterPartial,   // name_string, template_handle -> 0
    TemplateGetPartial,        // name_string -> template_handle (0 if not found)
    TemplateUnregisterPartial, // name_string -> 0
    TemplateFree,              // template_handle -> 0 (frees memory)
    TemplateEscapeHtml,        // string -> escaped_string
    TemplateUnescapeHtml,      // string -> unescaped_string

    // Shared memory cells (simple global handle->value store)
    SharedNew,            // -> handle
    SharedStore,          // handle, value -> 0
    SharedLoad,           // handle -> value (0 default)
    SharedGetByName(u32), // name index -> handle
    // Closure operations (first-class functions)
    // Creates a closure object: function_id, num_captures -> closure_handle
    ClosureNew(u32, u32),
    // Adds a captured value to the most recently created closure: closure_handle, value -> 0
    ClosureCapture,
    // Calls a closure indirectly: closure_handle -> result (args on stack)
    ClosureCall(u32), // num_args

    // Reference counting operations
    // Allocate a new RC cell: value -> handle
    // Takes value from stack, wraps in RC cell with count=1, pushes handle
    RcAlloc,
    // Increment reference count: handle -> handle
    // Increments RC, pushes same handle back
    RcInc,
    // Decrement reference count: handle -> 0
    // Decrements RC, deallocates if 0, pushes 0
    RcDec,
    // Decrement with deinit call: handle, deinit_func_index -> 0
    // Like RcDec but calls deinit function when count reaches 0
    RcDecWithDeinit(u32),
    // Load value from RC cell: handle -> value
    RcLoad,
    // Store value to RC cell: handle, value -> 0
    RcStore,
    // Get current reference count: handle -> count
    RcGetCount,

    // Region-based allocation operations for loop-local values
    // Enter a region: region_id -> 0
    // Creates a new allocation region for bulk deallocation
    RegionEnter(u32),
    // Exit a region: region_id -> 0
    // Bulk deallocates all values allocated in this region
    RegionExit(u32),

    // Panic and unwinding operations
    // Panic: trigger an unrecoverable error that unwinds within task boundary
    // Takes a string message index from the string pool
    // Panics run drops along the unwind path and propagate to join() as TaskPanicked
    Panic(u32),
    // SetUnwindHandler: register a handler for panic/throw unwinding
    // Handler is an instruction pointer (u32) to jump to on unwind
    SetUnwindHandler(u32),
    // ClearUnwindHandler: remove the current unwind handler
    ClearUnwindHandler,
    // GetPanicMessage: push the current panic message onto the stack (for task failure inspection)
    GetPanicMessage,

    // Exception handling operations (catchable exceptions, unlike panic)
    // Throw: throw an exception value, transfer control to nearest handler
    // Pops exception value from stack, stores it, jumps to handler
    // If no handler is registered, terminates with error
    Throw,
    // GetException: push the current exception value onto the stack
    // Used in catch blocks to access the caught exception
    GetException,

    // I/O, Directory, Path, Console, DateTime, Instant operations have been migrated
    // to HostCallIo and HostCallTime opcodes. See HostIoOp and HostTimeOp enums.
    // Legacy opcodes removed in favor of capability-based host dispatch.

    // BigDecimal operations (numeric package)
    // BigDecimal stored as string internally for arbitrary precision
    BigDecimalNew,       // str_idx -> bigdecimal_handle
    BigDecimalFromInt,   // i64 -> bigdecimal_handle
    BigDecimalFromFloat, // f64 -> bigdecimal_handle
    BigDecimalAdd,       // bd1, bd2 -> bd_result
    BigDecimalSub,       // bd1, bd2 -> bd_result
    BigDecimalMul,       // bd1, bd2 -> bd_result
    BigDecimalDiv,       // bd1, bd2, scale -> bd_result (scale = decimal places)
    BigDecimalRem,       // bd1, bd2 -> bd_result
    BigDecimalPow,       // bd, exponent -> bd_result
    // Removed: BigDecimalAbs - now pure Arth in stdlib/src/numeric/BigDecimal.arth
    BigDecimalNegate,   // bd -> bd_result
    BigDecimalCompare,  // bd1, bd2 -> -1/0/1
    BigDecimalToString, // bd -> string_handle
    BigDecimalToInt,    // bd -> i64 (truncated)
    BigDecimalToFloat,  // bd -> f64 (may lose precision)
    BigDecimalScale,    // bd -> scale (number of decimal places)
    BigDecimalSetScale, // bd, scale, rounding_mode -> bd_result
    BigDecimalRound,    // bd, scale, rounding_mode -> bd_result

    // BigInt operations (numeric package)
    // BigInt stored as string internally for arbitrary precision
    BigIntNew,     // str_idx -> bigint_handle
    BigIntFromInt, // i64 -> bigint_handle
    BigIntAdd,     // bi1, bi2 -> bi_result
    BigIntSub,     // bi1, bi2 -> bi_result
    BigIntMul,     // bi1, bi2 -> bi_result
    BigIntDiv,     // bi1, bi2 -> bi_result
    BigIntRem,     // bi1, bi2 -> bi_result
    BigIntPow,     // bi, exponent -> bi_result
    // Removed: BigIntAbs, BigIntGcd, BigIntModPow - now pure Arth in stdlib/src/numeric/BigInt.arth
    BigIntNegate,   // bi -> bi_result
    BigIntCompare,  // bi1, bi2 -> -1/0/1
    BigIntToString, // bi -> string_handle
    BigIntToInt,    // bi -> i64 (truncated/clamped)

    // WebSocket and SSE operations have been migrated to HostCallNet opcodes.
    // See HostNetOp enum for WsServe, WsAccept, WsSendText, WsSendBinary, WsRecv,
    // WsClose, WsIsOpen, SseServe, SseAccept, SseSend, SseClose, SseIsOpen.

    // =========================================================================
    // Host Call Operations (new capability-based dispatch)
    // =========================================================================
    // These opcodes route to pluggable host implementations via HostContext.
    // They will eventually replace the dedicated IO/Net/Time opcodes above.
    /// Host IO call: dispatches to HostIo trait implementation
    /// Stack effects depend on the specific HostIoOp
    HostCallIo(HostIoOp),

    /// Host networking call: dispatches to HostNet trait implementation
    /// Stack effects depend on the specific HostNetOp
    HostCallNet(HostNetOp),

    /// Host time call: dispatches to HostTime trait implementation
    /// Stack effects depend on the specific HostTimeOp
    HostCallTime(HostTimeOp),

    /// Host database call: dispatches to HostDb trait implementation
    /// Stack effects depend on the specific HostDbOp
    HostCallDb(HostDbOp),

    /// Host mail call: dispatches to HostMail trait implementation
    /// Stack effects depend on the specific HostMailOp
    HostCallMail(HostMailOp),

    /// Host crypto call: dispatches to HostCrypto trait implementation
    /// Stack effects depend on the specific HostCryptoOp
    HostCallCrypto(HostCryptoOp),

    /// Generic host call: dispatches based on JSON payload
    /// Stack: [json_payload_string] -> [result_string]
    /// The JSON payload should have format: { "fn": "function_name", "args": { ... } }
    /// Supported functions:
    /// - log_info, log_debug, log_warn, log_error (args: { message: string })
    /// - get_local_state (args: { key: string }) -> { has_value: bool, value_json: string }
    /// - set_local_state (args: { key: string, value: string })
    /// - delete_local_state (args: { key: string })
    /// - get_time_ms () -> timestamp_ms
    /// - send_to_server (args: { event: string }) -> response_json
    HostCallGeneric,

    // =========================================================================
    // Async/Task Operations
    // =========================================================================
    // These opcodes support async function execution via the task runtime.
    /// TaskSpawn: spawn a new task for an async function body
    /// Stack: fn_id (function hash), argc (arg count) -> task_handle
    /// Creates a pending task that will execute the async body function
    TaskSpawn,

    /// TaskPushArg: push an argument to a pending task
    /// Stack: task_handle, arg_value -> 0
    /// Arguments are stored and passed to the body function when executed
    TaskPushArg,

    /// TaskAwait: await a task and get its result, executing the body if needed
    /// Stack: task_handle -> result_value
    /// For a pending task, this executes the body function synchronously
    /// Returns the result value or triggers an exception if the task failed
    TaskAwait,

    /// TaskRunBody: dispatch to run an async body function by its hash
    /// Stack: fn_id, argc, args... -> result
    /// This is used internally by TaskAwait to execute the task's body
    TaskRunBody(u32), // u32 = func offset in bytecode

    // =========================================================================
    // Concurrent Runtime Operations
    // =========================================================================
    // Thread pool and work-stealing executor operations for true parallel execution.
    /// ExecutorInit: initialize the global thread pool
    /// Stack: num_threads -> success (1 if initialized, 0 if already initialized)
    /// Call once at program startup. If not called, pool is lazily initialized
    /// with CPU count threads on first use.
    ExecutorInit,

    /// ExecutorThreadCount: get number of worker threads in the pool
    /// Stack: -> thread_count
    ExecutorThreadCount,

    /// ExecutorActiveWorkers: get number of currently active worker threads
    /// Stack: -> active_count
    /// Active workers are threads currently executing tasks (not parked)
    ExecutorActiveWorkers,

    /// ExecutorSpawn: spawn a task on the thread pool
    /// Stack: fn_id -> task_id
    /// Creates a pending task and submits it to the global queue.
    /// The task will be executed by a worker thread when available.
    ExecutorSpawn,

    /// ExecutorJoin: wait for a task to complete and get its result
    /// Stack: task_id -> result (or -1 if task not found/cancelled)
    /// Blocks the current thread until the task completes.
    ExecutorJoin,

    /// ExecutorSpawnWithArg: spawn a task with a single argument
    /// Stack: fn_id, arg -> task_id
    /// Creates a pending task with fn_id as work type and arg as parameter.
    /// Work types:
    ///   1 = CPU iterations (arg = number of iterations)
    ///   2 = Fibonacci (arg = n)
    ExecutorSpawnWithArg,

    /// ExecutorActiveExecutorCount: get number of workers that executed at least one task
    /// Stack: -> count
    /// This is the key metric for verifying work-stealing is working.
    ExecutorActiveExecutorCount,

    /// ExecutorWorkerTaskCount: get task count for a specific worker
    /// Stack: worker_idx -> count
    /// Returns the number of tasks executed by the specified worker thread.
    ExecutorWorkerTaskCount,

    /// ExecutorResetStats: reset all worker task counters
    /// Stack: -> 0
    /// Resets per-worker task counts to zero for testing purposes.
    ExecutorResetStats,

    /// ExecutorSpawnAwait: spawn a task and immediately wait for it (C04 suspension demo)
    /// Stack: sub_fn_id, sub_arg, local_accumulator -> task_id
    /// Spawns a sub-task with (sub_fn_id, sub_arg) and creates a parent task that:
    /// 1. Spawns the sub-task
    /// 2. Suspends waiting for sub-task to complete
    /// 3. Resumes and returns local_accumulator + sub_task_result
    /// This demonstrates task suspension/wakeup with preserved locals.
    ExecutorSpawnAwait,

    // =========================================================================
    // MPMC Channel Operations (C06 - Lock-Free Multi-Producer Multi-Consumer)
    // =========================================================================
    // Thread-safe channels using crossbeam-channel for inter-thread communication.
    /// MpmcChanCreate: create a new MPMC channel
    /// Stack: capacity -> channel_handle
    /// If capacity <= 0, creates an unbounded channel.
    MpmcChanCreate,

    /// MpmcChanSend: non-blocking send to MPMC channel
    /// Stack: channel_handle, value -> status
    /// Returns: 0=success, 1=full, 2=closed, 3=not_found
    MpmcChanSend,

    /// MpmcChanSendBlocking: blocking send to MPMC channel
    /// Stack: channel_handle, value -> status
    /// Blocks until message is sent or channel is closed.
    /// Returns: 0=success, 2=closed, 3=not_found
    MpmcChanSendBlocking,

    /// MpmcChanRecv: non-blocking receive from MPMC channel
    /// Stack: channel_handle -> value
    /// Returns: value on success, -2=empty, -3=closed
    MpmcChanRecv,

    /// MpmcChanRecvBlocking: blocking receive from MPMC channel
    /// Stack: channel_handle -> value
    /// Blocks until a message is received or channel is closed.
    /// Returns: value on success, -3=closed
    MpmcChanRecvBlocking,

    /// MpmcChanClose: close an MPMC channel
    /// Stack: channel_handle -> status
    /// Returns: 0=success, 1=not_found
    MpmcChanClose,

    /// MpmcChanLen: get current length of MPMC channel
    /// Stack: channel_handle -> length
    /// Returns: number of messages, -1=not_found
    MpmcChanLen,

    /// MpmcChanIsEmpty: check if MPMC channel is empty
    /// Stack: channel_handle -> is_empty
    /// Returns: 1=empty, 0=not_empty, -1=not_found
    MpmcChanIsEmpty,

    /// MpmcChanIsFull: check if MPMC channel is full
    /// Stack: channel_handle -> is_full
    /// Returns: 1=full, 0=not_full, -1=not_found
    MpmcChanIsFull,

    /// MpmcChanIsClosed: check if MPMC channel is closed
    /// Stack: channel_handle -> is_closed
    /// Returns: 1=closed, 0=open, -1=not_found
    MpmcChanIsClosed,

    /// MpmcChanCapacity: get capacity of MPMC channel
    /// Stack: channel_handle -> capacity
    /// Returns: capacity (0=unbounded), -1=not_found
    MpmcChanCapacity,

    // =========================================================================
    // C07: Executor-Integrated MPMC Channel Operations
    // =========================================================================
    // These operations integrate MPMC channels with the executor's task
    // suspension mechanism for true async send/receive without blocking workers.
    /// MpmcChanSendWithTask: try send with task suspension support
    /// Stack: channel_handle, value, task_id -> status
    /// Returns: 0=success, 1=full (task registered as waiting), 2=closed, 3=not_found
    /// If channel is full, registers task_id as waiting sender and returns 1.
    /// The caller should suspend the task and wake it when space is available.
    MpmcChanSendWithTask,

    /// MpmcChanRecvWithTask: try recv with task suspension support
    /// Stack: channel_handle, task_id -> value
    /// Returns: value on success, -2=empty (task registered), -3=closed
    /// If channel is empty, registers task_id as waiting receiver and returns -2.
    MpmcChanRecvWithTask,

    /// MpmcChanRecvAndWake: receive and wake a waiting sender
    /// Stack: channel_handle -> value
    /// After receiving, pops a waiting sender and completes their send.
    /// The woken sender's task ID can be retrieved via MpmcChanGetWokenSender.
    MpmcChanRecvAndWake,

    /// MpmcChanPopWaitingSender: pop a waiting sender from the channel
    /// Stack: channel_handle -> task_id
    /// Returns: task_id (0 if no waiting senders)
    /// The sender's value can be retrieved via MpmcChanGetWaitingSenderValue.
    MpmcChanPopWaitingSender,

    /// MpmcChanGetWaitingSenderValue: get the value from the last popped sender
    /// Stack: -> value
    MpmcChanGetWaitingSenderValue,

    /// MpmcChanPopWaitingReceiver: pop a waiting receiver from the channel
    /// Stack: channel_handle -> task_id
    /// Returns: task_id (0 if no waiting receivers)
    MpmcChanPopWaitingReceiver,

    /// MpmcChanWaitingSenderCount: count waiting senders
    /// Stack: channel_handle -> count
    /// Returns: count (-1 if not found)
    MpmcChanWaitingSenderCount,

    /// MpmcChanWaitingReceiverCount: count waiting receivers
    /// Stack: channel_handle -> count
    /// Returns: count (-1 if not found)
    MpmcChanWaitingReceiverCount,

    /// MpmcChanGetWokenSender: get the task ID of the sender woken by last recv_and_wake
    /// Stack: -> task_id
    /// Returns: task_id (0 if no sender was woken)
    MpmcChanGetWokenSender,

    // =========================================================================
    // C08: Blocking Receive - Send and Wake Operations
    // =========================================================================
    /// MpmcChanSendAndWake: send a value and wake any waiting receivers
    /// Stack: channel_handle, value -> status
    /// Returns: 0=success, 1=full, 2=closed, 3=not_found
    /// Side effect: Wakes a waiting receiver if one exists.
    MpmcChanSendAndWake,

    /// MpmcChanGetWokenReceiver: get the task ID of the receiver woken by last send_and_wake
    /// Stack: -> task_id
    /// Returns: task_id (0 if no receiver was woken)
    MpmcChanGetWokenReceiver,

    // =========================================================================
    // C09: Channel Select Operations
    // =========================================================================
    /// MpmcChanSelectClear: clear the select channel set for a new select operation
    /// Stack: -> (no result pushed)
    MpmcChanSelectClear,

    /// MpmcChanSelectAdd: add a channel to the select set
    /// Stack: channel_handle -> index
    /// Returns: index of the channel in the select set (0-based)
    MpmcChanSelectAdd,

    /// MpmcChanSelectCount: get the number of channels in the select set
    /// Stack: -> count
    MpmcChanSelectCount,

    /// MpmcChanTrySelectRecv: non-blocking try to receive from any channel
    /// Stack: -> status
    /// Returns: 0=received, 1=none ready, 2=all closed, 3=no channels
    MpmcChanTrySelectRecv,

    /// MpmcChanSelectRecvBlocking: blocking receive from any channel
    /// Stack: -> status
    /// Returns: 0=received, 2=all closed, 3=no channels
    MpmcChanSelectRecvBlocking,

    /// MpmcChanSelectRecvWithTask: try select with task registration
    /// Stack: task_id -> status
    /// Returns: 0=received, 1=suspended, 2=all closed, 3=no channels
    MpmcChanSelectRecvWithTask,

    /// MpmcChanSelectGetReadyIndex: get the index of the ready channel
    /// Stack: -> index
    /// Returns: index (or -1 if none)
    MpmcChanSelectGetReadyIndex,

    /// MpmcChanSelectGetValue: get the value from the last select
    /// Stack: -> value
    MpmcChanSelectGetValue,

    /// MpmcChanSelectDeregister: deregister task from all channels except one
    /// Stack: task_id, except_index -> (no result)
    MpmcChanSelectDeregister,

    /// MpmcChanSelectGetHandle: get a channel handle from the select set by index
    /// Stack: index -> handle
    /// Returns: handle (or -1 if out of bounds)
    MpmcChanSelectGetHandle,

    // =========================================================================
    // C11: Actor Operations (Actor = Task + Channel)
    // =========================================================================
    // Actors are tasks with mailboxes (MPMC channels) for message passing.
    // Each actor runs on the thread pool and processes messages from its mailbox.
    /// ActorCreate: create a new actor with a mailbox
    /// Stack: capacity -> actor_handle
    /// Creates an actor with a mailbox of the specified capacity.
    /// The actor is in Running state but not yet associated with a task.
    ActorCreate,

    /// ActorSpawn: create an actor associated with a task
    /// Stack: capacity, task_handle -> actor_handle
    /// Creates an actor and associates it with the specified task handle.
    ActorSpawn,

    /// ActorSend: non-blocking send to an actor's mailbox
    /// Stack: actor_handle, message -> status
    /// Returns: 0=success, 1=full, 2=stopped/closed, 3=not_found
    ActorSend,

    /// ActorSendBlocking: blocking send to an actor's mailbox
    /// Stack: actor_handle, message -> status
    /// Returns: 0=success, 2=stopped/closed, 3=not_found
    ActorSendBlocking,

    /// ActorRecv: non-blocking receive from an actor's mailbox
    /// Stack: actor_handle -> message
    /// Returns: message on success, -2=empty, -3=stopped/closed
    ActorRecv,

    /// ActorRecvBlocking: blocking receive from an actor's mailbox
    /// Stack: actor_handle -> message
    /// Returns: message on success, -3=stopped/closed
    ActorRecvBlocking,

    /// ActorClose: close an actor's mailbox (request stop)
    /// Stack: actor_handle -> status
    /// Returns: 0=success, 1=not_found
    ActorClose,

    /// ActorStop: request graceful stop of an actor
    /// Stack: actor_handle -> status
    /// Returns: 0=success, 1=not_found
    ActorStop,

    /// ActorGetTask: get the task handle of an actor
    /// Stack: actor_handle -> task_handle
    /// Returns: task_handle (0 if not spawned, -1 if not found)
    ActorGetTask,

    /// ActorGetMailbox: get the mailbox channel handle of an actor
    /// Stack: actor_handle -> mailbox_handle
    /// Returns: mailbox_handle (-1 if not found)
    ActorGetMailbox,

    /// ActorIsRunning: check if an actor is running
    /// Stack: actor_handle -> is_running
    /// Returns: 1=running, 0=not running, -1=not found
    ActorIsRunning,

    /// ActorGetState: get the state of an actor
    /// Stack: actor_handle -> state
    /// Returns: 0=running, 1=stopping, 2=stopped, 3=failed, -1=not found
    ActorGetState,

    /// ActorMessageCount: get the message count of an actor
    /// Stack: actor_handle -> count
    /// Returns: message count (-1 if not found)
    ActorMessageCount,

    /// ActorMailboxEmpty: check if an actor's mailbox is empty
    /// Stack: actor_handle -> is_empty
    /// Returns: 1=empty, 0=not empty, -1=not found
    ActorMailboxEmpty,

    /// ActorMailboxLen: get the length of an actor's mailbox
    /// Stack: actor_handle -> length
    /// Returns: mailbox length (-1 if not found)
    ActorMailboxLen,

    /// ActorSetTask: set the task handle for an actor
    /// Stack: actor_handle, task_handle -> status
    /// Returns: 0=success, 1=not found
    ActorSetTask,

    /// ActorMarkStopped: mark an actor as stopped
    /// Stack: actor_handle -> status
    /// Returns: 0=success, 1=not found
    ActorMarkStopped,

    /// ActorMarkFailed: mark an actor as failed (crashed)
    /// Stack: actor_handle -> status
    /// Returns: 0=success, 1=not found
    ActorMarkFailed,

    /// ActorIsFailed: check if an actor has failed
    /// Stack: actor_handle -> result
    /// Returns: 1=failed, 0=not failed, -1=not found
    ActorIsFailed,

    // =========================================================================
    // Phase 4: Atomic<T> Operations (C19)
    // Thread-safe atomic integer operations with memory ordering support.
    // Atomic<T> is both Sendable AND Shareable.
    // =========================================================================
    /// AtomicCreate: create a new atomic integer with an initial value
    /// Stack: initial_value -> handle
    /// Returns: positive handle on success
    AtomicCreate,

    /// AtomicLoad: atomically load the current value
    /// Stack: handle -> value
    /// Returns: current value
    AtomicLoad,

    /// AtomicStore: atomically store a new value
    /// Stack: handle, value -> status
    /// Returns: 0=success, -1=not found
    AtomicStore,

    /// AtomicCas: compare-and-swap operation
    /// Stack: handle, expected, new_value -> result
    /// Returns: 1=success (swapped), 0=failure (not swapped), -1=not found
    AtomicCas,

    /// AtomicFetchAdd: atomically add to the value and return the old value
    /// Stack: handle, delta -> old_value
    /// Returns: old value before addition
    AtomicFetchAdd,

    /// AtomicFetchSub: atomically subtract from the value and return the old value
    /// Stack: handle, delta -> old_value
    /// Returns: old value before subtraction
    AtomicFetchSub,

    /// AtomicSwap: atomically swap the value and return the old value
    /// Stack: handle, new_value -> old_value
    /// Returns: old value before swap
    AtomicSwap,

    /// AtomicGet: alias for AtomicLoad (for ergonomics)
    /// Stack: handle -> value
    AtomicGet,

    /// AtomicSet: alias for AtomicStore (for ergonomics)
    /// Stack: handle, value -> status
    AtomicSet,

    /// AtomicInc: atomically increment by 1 and return the old value
    /// Stack: handle -> old_value
    AtomicInc,

    /// AtomicDec: atomically decrement by 1 and return the old value
    /// Stack: handle -> old_value
    AtomicDec,

    // =========================================================================
    // Phase 5: Event Loop Operations (C21)
    // Event-driven I/O using OS primitives (pipes, timers) with polling.
    // Provides the foundation for async I/O integration.
    // =========================================================================
    /// EventLoopCreate: create a new event loop instance
    /// Stack: -> loop_handle
    /// Returns: positive handle on success, -1 on error
    EventLoopCreate,

    /// EventLoopRegisterTimer: register a one-shot timer with the event loop
    /// Stack: loop_handle, timeout_ms -> token
    /// Returns: token (>= 0) on success, -1 on error
    /// The timer fires once after timeout_ms milliseconds.
    EventLoopRegisterTimer,

    /// EventLoopRegisterFd: register a file descriptor for monitoring
    /// Stack: loop_handle, fd, interest -> token
    /// Interest: 1=READ, 2=WRITE, 3=READ|WRITE
    /// Returns: token (>= 0) on success, -1 on error
    EventLoopRegisterFd,

    /// EventLoopDeregister: deregister a token from the event loop
    /// Stack: loop_handle, token -> status
    /// Returns: 0=success, -1=error
    EventLoopDeregister,

    /// EventLoopPoll: poll for events with timeout
    /// Stack: loop_handle, timeout_ms -> num_events
    /// Returns: number of events ready (0 = timeout, -1 = error)
    /// Events are stored internally and can be retrieved with GetEvent/GetEventType.
    EventLoopPoll,

    /// EventLoopGetEvent: get the token from an event at index
    /// Stack: index -> token
    /// Returns: token for the event, -1 if index out of bounds
    EventLoopGetEvent,

    /// EventLoopGetEventType: get the type of event at index
    /// Stack: index -> event_type
    /// Returns: 1=TIMER, 2=READ, 3=WRITE, 4=ERROR, -1=invalid index
    EventLoopGetEventType,

    /// EventLoopClose: close and destroy an event loop
    /// Stack: loop_handle -> status
    /// Returns: 0=success, -1=error
    EventLoopClose,

    /// EventLoopPipeCreate: create a pipe pair for inter-thread communication
    /// Stack: -> read_fd
    /// The write_fd is stored in thread-local storage and can be retrieved
    /// with EventLoopPipeGetWriteFd. Both fds are non-blocking.
    EventLoopPipeCreate,

    /// EventLoopPipeGetWriteFd: get the write fd from the last pipe creation
    /// Stack: -> write_fd
    /// Returns: write file descriptor, -1 if no pipe was created
    EventLoopPipeGetWriteFd,

    /// EventLoopPipeWrite: write a single i64 value to a pipe
    /// Stack: write_fd, value -> bytes_written
    /// Returns: 8 on success (bytes written), -1 on error, 0 if would block
    EventLoopPipeWrite,

    /// EventLoopPipeRead: read a single i64 value from a pipe
    /// Stack: read_fd -> value
    /// Returns: the value read, or -1 on error, -2 if would block
    EventLoopPipeRead,

    /// EventLoopPipeClose: close a pipe file descriptor
    /// Stack: fd -> status
    /// Returns: 0=success, -1=error
    EventLoopPipeClose,

    // =========================================================================
    // Phase 5: Async Timer Operations (C22)
    // Timer-based async operations with task suspension and wakeup.
    // =========================================================================
    /// TimerSleep: blocking sleep for specified milliseconds
    /// Stack: ms -> ()
    /// Blocks the current thread for ms milliseconds.
    TimerSleep,

    /// TimerSleepAsync: register an async timer that will wake a task
    /// Stack: ms, task_id -> timer_id
    /// Registers a timer that fires after ms milliseconds. When fired,
    /// the task with task_id will be marked ready for wakeup.
    /// Returns: timer_id (>= 0) on success, -1 on error
    TimerSleepAsync,

    /// TimerCheckExpired: check if a timer has expired
    /// Stack: timer_id -> status
    /// Returns: 1=expired, 0=pending, -1=not found/cancelled
    TimerCheckExpired,

    /// TimerGetWaitingTask: get the task ID waiting on a timer
    /// Stack: timer_id -> task_id
    /// Returns: task_id (>= 0), or -1 if not found
    TimerGetWaitingTask,

    /// TimerCancel: cancel a pending timer
    /// Stack: timer_id -> status
    /// Returns: 0=success, -1=not found/already fired
    TimerCancel,

    /// TimerPollExpired: poll for the next expired timer
    /// Stack: -> timer_id
    /// Returns: timer_id of an expired timer, or -1 if none expired
    /// This is used by the executor to check for timers to wake.
    TimerPollExpired,

    /// TimerNow: get current time in milliseconds since epoch
    /// Stack: -> ms
    /// Returns: current time in milliseconds
    TimerNow,

    /// TimerElapsed: get elapsed time since a previous timestamp
    /// Stack: start_ms -> elapsed_ms
    /// Returns: elapsed milliseconds since start_ms
    TimerElapsed,

    /// TimerRemove: remove a timer from the registry
    /// Stack: timer_id -> status
    /// Returns: 0 on success, -1 if not found
    TimerRemove,

    /// TimerRemaining: get remaining time until timer fires
    /// Stack: timer_id -> remaining_ms
    /// Returns: milliseconds remaining (>= 0), 0 if expired, -1 if not found
    TimerRemaining,

    // Phase 5: Async TCP Socket Operations (C23)
    // TCP listener and stream operations for network I/O.
    /// TcpListenerBind: bind a TCP listener to a port
    /// Stack: port -> listener_handle
    /// Returns: listener handle (>= 80000), -1 on error
    TcpListenerBind,

    /// TcpListenerAccept: accept a connection (blocking)
    /// Stack: listener_handle -> stream_handle
    /// Returns: stream handle (>= 90000), -1 on error
    TcpListenerAccept,

    /// TcpListenerAcceptAsync: start async accept
    /// Stack: listener_handle, task_id -> request_id
    /// Returns: request ID for checking completion
    TcpListenerAcceptAsync,

    /// TcpListenerClose: close a TCP listener
    /// Stack: listener_handle -> status
    /// Returns: 0 on success, -1 on error
    TcpListenerClose,

    /// TcpListenerLocalPort: get the local port of listener
    /// Stack: listener_handle -> port
    /// Returns: port number, -1 on error
    TcpListenerLocalPort,

    /// TcpStreamConnect: connect to a TCP server (blocking)
    /// Stack: host_str_idx, port -> stream_handle
    /// Returns: stream handle (>= 90000), -1 on error
    TcpStreamConnect,

    /// TcpStreamConnectAsync: start async connect
    /// Stack: host_str_idx, port, task_id -> request_id
    /// Returns: request ID for checking completion
    TcpStreamConnectAsync,

    /// TcpStreamRead: read from stream (blocking)
    /// Stack: stream_handle, max_bytes -> bytes_read
    /// Returns: number of bytes read, 0 on EOF, -1 on error
    /// Data available via TcpStreamGetLastRead
    TcpStreamRead,

    /// TcpStreamReadAsync: start async read
    /// Stack: stream_handle, max_bytes, task_id -> request_id
    /// Returns: request ID for checking completion
    TcpStreamReadAsync,

    /// TcpStreamWrite: write to stream (blocking)
    /// Stack: stream_handle, data_str_idx -> bytes_written
    /// Returns: number of bytes written, -1 on error
    TcpStreamWrite,

    /// TcpStreamWriteAsync: start async write
    /// Stack: stream_handle, data_str_idx, task_id -> request_id
    /// Returns: request ID for checking completion
    TcpStreamWriteAsync,

    /// TcpStreamClose: close a TCP stream
    /// Stack: stream_handle -> status
    /// Returns: 0 on success, -1 on error
    TcpStreamClose,

    /// TcpStreamGetLastRead: get data from last read operation
    /// Stack: -> str_idx
    /// Returns: string index of read data
    TcpStreamGetLastRead,

    /// TcpStreamSetTimeout: set read/write timeout
    /// Stack: stream_handle, timeout_ms -> status
    /// Returns: 0 on success, -1 on error
    TcpStreamSetTimeout,

    /// TcpCheckReady: check if async operation is ready
    /// Stack: request_id -> status
    /// Returns: 0=pending, 1=ready, -1=error/not found
    TcpCheckReady,

    /// TcpGetResult: get result of completed async operation
    /// Stack: request_id -> result
    /// Returns: operation result, -1 on error
    TcpGetResult,

    /// TcpPollReady: poll for next ready async operation
    /// Stack: -> request_id
    /// Returns: request ID of ready operation, -1 if none
    TcpPollReady,

    /// TcpRemoveRequest: remove completed request from registry
    /// Stack: request_id -> status
    /// Returns: 0 on success, -1 on error
    TcpRemoveRequest,

    // Phase 5: HTTP Client Operations (C24)
    // HTTP request/response operations building on TCP sockets.
    /// HttpGet: perform a blocking HTTP GET request
    /// Stack: url_str_idx, timeout_ms -> response_handle
    /// Returns: response handle (>= 110000), -1 on error
    HttpGet,

    /// HttpPost: perform a blocking HTTP POST request
    /// Stack: url_str_idx, body_str_idx, timeout_ms -> response_handle
    /// Returns: response handle (>= 110000), -1 on error
    HttpPost,

    /// HttpGetAsync: start async HTTP GET request
    /// Stack: url_str_idx, timeout_ms, task_id -> request_id
    /// Returns: request ID for checking completion
    HttpGetAsync,

    /// HttpPostAsync: start async HTTP POST request
    /// Stack: url_str_idx, body_str_idx, timeout_ms, task_id -> request_id
    /// Returns: request ID for checking completion
    HttpPostAsync,

    /// HttpResponseStatus: get HTTP response status code
    /// Stack: response_handle -> status_code
    /// Returns: HTTP status code (200, 404, etc), -1 on error
    HttpResponseStatus,

    /// HttpResponseHeader: get HTTP response header value
    /// Stack: response_handle, header_key_str_idx -> value_str_idx
    /// Returns: string index of header value, -1 if not found
    HttpResponseHeader,

    /// HttpResponseBody: get HTTP response body
    /// Stack: response_handle -> body_str_idx
    /// Returns: string index of body content, -1 on error
    HttpResponseBody,

    /// HttpResponseClose: close HTTP response and release resources
    /// Stack: response_handle -> status
    /// Returns: 0 on success, -1 on error
    HttpResponseClose,

    /// HttpCheckReady: check if async HTTP request is ready
    /// Stack: request_id -> status
    /// Returns: 0=pending, 1=ready, -1=error/not found
    HttpCheckReady,

    /// HttpGetResult: get result of completed async HTTP request
    /// Stack: request_id -> response_handle
    /// Returns: response handle, -1 on error
    HttpGetResult,

    /// HttpPollReady: poll for next ready async HTTP request
    /// Stack: -> request_id
    /// Returns: request ID of ready request, -1 if none
    HttpPollReady,

    /// HttpRemoveRequest: remove completed HTTP request from registry
    /// Stack: request_id -> status
    /// Returns: 0 on success, -1 on error
    HttpRemoveRequest,

    /// HttpGetBodyLength: get length of HTTP response body
    /// Stack: response_handle -> length
    /// Returns: body length in bytes, -1 on error
    HttpGetBodyLength,

    /// HttpGetHeaderCount: get number of headers in response
    /// Stack: response_handle -> count
    /// Returns: header count, -1 on error
    HttpGetHeaderCount,

    // Phase 5: HTTP Server Operations (C25)
    // HTTP server operations building on TCP sockets.
    /// HttpServerCreate: create an HTTP server bound to a port
    /// Stack: port -> server_handle
    /// Returns: server handle (>= 130000), -1 on error
    HttpServerCreate,

    /// HttpServerClose: close an HTTP server
    /// Stack: server_handle -> status
    /// Returns: 0 on success, -1 on error
    HttpServerClose,

    /// HttpServerGetPort: get the bound port of the server
    /// Stack: server_handle -> port
    /// Returns: port number, -1 on error
    HttpServerGetPort,

    /// HttpServerAccept: accept an incoming HTTP request (blocking)
    /// Stack: server_handle -> conn_handle
    /// Returns: connection handle (>= 140000), -1 on error
    HttpServerAccept,

    /// HttpServerAcceptAsync: start async accept for incoming request
    /// Stack: server_handle, task_id -> request_id
    /// Returns: request ID for checking completion
    HttpServerAcceptAsync,

    /// HttpRequestMethod: get the HTTP method of a request
    /// Stack: conn_handle -> method_str_idx
    /// Returns: string index ("GET", "POST", etc), -1 on error
    HttpRequestMethod,

    /// HttpRequestPath: get the request path
    /// Stack: conn_handle -> path_str_idx
    /// Returns: string index of path, -1 on error
    HttpRequestPath,

    /// HttpRequestHeader: get a request header value
    /// Stack: conn_handle, header_name_str_idx -> value_str_idx
    /// Returns: string index of header value, -1 if not found
    HttpRequestHeader,

    /// HttpRequestBody: get the request body
    /// Stack: conn_handle -> body_str_idx
    /// Returns: string index of body, -1 on error
    HttpRequestBody,

    /// HttpRequestHeaderCount: get number of headers in request
    /// Stack: conn_handle -> count
    /// Returns: header count, -1 on error
    HttpRequestHeaderCount,

    /// HttpRequestBodyLength: get length of request body
    /// Stack: conn_handle -> length
    /// Returns: body length in bytes, -1 on error
    HttpRequestBodyLength,

    /// HttpWriterStatus: set the response status code
    /// Stack: conn_handle, status_code -> status
    /// Returns: 0 on success, -1 on error
    HttpWriterStatus,

    /// HttpWriterHeader: add a response header
    /// Stack: conn_handle, name_str_idx, value_str_idx -> status
    /// Returns: 0 on success, -1 on error
    HttpWriterHeader,

    /// HttpWriterBody: set the response body
    /// Stack: conn_handle, body_str_idx -> status
    /// Returns: 0 on success, -1 on error
    HttpWriterBody,

    /// HttpWriterSend: send the HTTP response (blocking)
    /// Stack: conn_handle -> status
    /// Returns: 0 on success, -1 on error
    HttpWriterSend,

    /// HttpWriterSendAsync: send the HTTP response asynchronously
    /// Stack: conn_handle, task_id -> request_id
    /// Returns: request ID for checking completion
    HttpWriterSendAsync,

    /// HttpServerCheckReady: check if async server operation is ready
    /// Stack: request_id -> status
    /// Returns: 0=pending, 1=ready, -1=error/not found
    HttpServerCheckReady,

    /// HttpServerGetResult: get result of completed async server operation
    /// Stack: request_id -> result
    /// Returns: operation result, -1 on error
    HttpServerGetResult,

    /// HttpServerPollReady: poll for next ready async server operation
    /// Stack: -> request_id
    /// Returns: request ID of ready operation, -1 if none
    HttpServerPollReady,

    /// HttpServerRemoveRequest: remove completed server request from registry
    /// Stack: request_id -> status
    /// Returns: 0 on success, -1 on error
    HttpServerRemoveRequest,
}

// ============================================================================
// Crypto Operations
// ============================================================================

/// Host cryptographic operations.
///
/// These operations provide access to cryptographic primitives including:
/// - Secure memory allocation
/// - Hash functions (SHA-256, SHA-512, SHA3, BLAKE3)
/// - HMAC
/// - AEAD encryption (AES-GCM, ChaCha20-Poly1305)
/// - Digital signatures (Ed25519, ECDSA, RSA-PSS)
/// - Key exchange (X25519, ECDH)
/// - Key derivation (HKDF, PBKDF2, Argon2)
/// - Password hashing (bcrypt, Argon2)
/// - Secure random number generation
/// - Base64/hex encoding
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HostCryptoOp {
    // -------------------------------------------------------------------------
    // Secure Memory Operations
    // -------------------------------------------------------------------------
    /// Allocate secure memory region (mlock'd, zero-on-free)
    /// Stack: size -> handle
    SecureAlloc = 0,
    /// Free secure memory region
    /// Stack: handle -> status
    SecureFree = 1,
    /// Get pointer to secure memory
    /// Stack: handle -> ptr
    SecurePtr = 2,
    /// Get length of secure memory
    /// Stack: handle -> len
    SecureLen = 3,
    /// Write to secure memory
    /// Stack: handle, offset, data_ptr, data_len -> bytes_written
    SecureWrite = 4,
    /// Read from secure memory
    /// Stack: handle, offset, out_ptr, out_len -> bytes_read
    SecureRead = 5,
    /// Zero secure memory
    /// Stack: handle -> status
    SecureZero = 6,
    /// Constant-time compare
    /// Stack: a_ptr, a_len, b_ptr, b_len -> 0 if equal, non-zero otherwise
    SecureCompare = 7,

    // -------------------------------------------------------------------------
    // Hash Operations
    // -------------------------------------------------------------------------
    /// One-shot hash
    /// Stack: algorithm, data_ptr, data_len, out_ptr, out_len -> hash_len or -1
    Hash = 10,
    /// Create new hasher
    /// Stack: algorithm -> handle
    HasherNew = 11,
    /// Update hasher with data
    /// Stack: handle, data_ptr, data_len -> status
    HasherUpdate = 12,
    /// Finalize hasher
    /// Stack: handle, out_ptr, out_len -> hash_len or -1
    HasherFinalize = 13,

    // -------------------------------------------------------------------------
    // HMAC Operations
    // -------------------------------------------------------------------------
    /// Compute HMAC
    /// Stack: algorithm, key_ptr, key_len, data_ptr, data_len, out_ptr, out_len -> mac_len or -1
    Hmac = 20,
    /// Create HMAC context
    /// Stack: algorithm, key_ptr, key_len -> handle
    HmacNew = 21,
    /// Update HMAC with data
    /// Stack: handle, data_ptr, data_len -> status
    HmacUpdate = 22,
    /// Finalize HMAC
    /// Stack: handle, out_ptr, out_len -> mac_len or -1
    HmacFinalize = 23,
    /// Verify HMAC
    /// Stack: algorithm, key_ptr, key_len, data_ptr, data_len, mac_ptr, mac_len -> 0 if valid, -1 otherwise
    HmacVerify = 24,

    // -------------------------------------------------------------------------
    // AEAD Operations
    // -------------------------------------------------------------------------
    /// Generate random nonce
    /// Stack: algorithm, out_ptr, out_len -> nonce_len or -1
    AeadGenerateNonce = 30,
    /// Encrypt with AEAD
    /// Stack: algorithm, key_ptr, key_len, nonce_ptr, nonce_len, plaintext_ptr, plaintext_len, aad_ptr, aad_len, out_ptr, out_len -> ciphertext_len or -1
    AeadEncrypt = 31,
    /// Decrypt with AEAD
    /// Stack: algorithm, key_ptr, key_len, nonce_ptr, nonce_len, ciphertext_ptr, ciphertext_len, aad_ptr, aad_len, out_ptr, out_len -> plaintext_len or -1
    AeadDecrypt = 32,

    // -------------------------------------------------------------------------
    // Signature Operations
    // -------------------------------------------------------------------------
    /// Generate keypair
    /// Stack: algorithm, priv_out_ptr, priv_out_len, pub_out_ptr, pub_out_len -> status
    SignatureGenerateKeypair = 40,
    /// Derive public key from private key
    /// Stack: algorithm, priv_ptr, priv_len, pub_out_ptr, pub_out_len -> pub_len or -1
    SignatureDerivePublicKey = 41,
    /// Sign message
    /// Stack: algorithm, priv_ptr, priv_len, msg_ptr, msg_len, sig_out_ptr, sig_out_len -> sig_len or -1
    SignatureSign = 42,
    /// Verify signature
    /// Stack: algorithm, pub_ptr, pub_len, msg_ptr, msg_len, sig_ptr, sig_len -> 0 if valid, -1 otherwise
    SignatureVerify = 43,
    /// Sign pre-hashed message
    /// Stack: algorithm, priv_ptr, priv_len, hash_ptr, hash_len, sig_out_ptr, sig_out_len -> sig_len or -1
    SignatureSignHash = 44,
    /// Verify signature on pre-hashed message
    /// Stack: algorithm, pub_ptr, pub_len, hash_ptr, hash_len, sig_ptr, sig_len -> 0 if valid, -1 otherwise
    SignatureVerifyHash = 45,

    // -------------------------------------------------------------------------
    // Key Exchange Operations
    // -------------------------------------------------------------------------
    /// Generate key exchange keypair
    /// Stack: algorithm, priv_out_ptr, priv_out_len, pub_out_ptr, pub_out_len -> status
    KexGenerateKeypair = 50,
    /// Perform key agreement
    /// Stack: algorithm, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len, shared_out_ptr, shared_out_len -> shared_len or -1
    KexAgree = 51,
    /// Perform key agreement with KDF
    /// Stack: algorithm, hash_algo, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len, salt_ptr, salt_len, info_ptr, info_len, out_ptr, out_len -> out_len or -1
    KexAgreeWithKdf = 52,

    // -------------------------------------------------------------------------
    // Key Derivation Operations
    // -------------------------------------------------------------------------
    /// HKDF extract and expand
    /// Stack: algorithm, ikm_ptr, ikm_len, salt_ptr, salt_len, info_ptr, info_len, out_ptr, out_len -> out_len or -1
    KdfHkdf = 60,
    /// PBKDF2 key derivation
    /// Stack: algorithm, password_ptr, password_len, salt_ptr, salt_len, iterations, out_ptr, out_len -> out_len or -1
    KdfPbkdf2 = 61,
    /// Argon2 key derivation
    /// Stack: variant, password_ptr, password_len, salt_ptr, salt_len, memory_kib, iterations, parallelism, out_ptr, out_len -> out_len or -1
    KdfArgon2 = 62,

    // -------------------------------------------------------------------------
    // Password Hashing Operations
    // -------------------------------------------------------------------------
    /// Hash password with bcrypt
    /// Stack: password_ptr, password_len, cost, out_ptr, out_len -> hash_len or -1
    PasswordHashBcrypt = 70,
    /// Hash password with Argon2
    /// Stack: variant, password_ptr, password_len, memory_kib, iterations, parallelism, out_ptr, out_len -> hash_len or -1
    PasswordHashArgon2 = 71,
    /// Verify password against hash (auto-detects algorithm)
    /// Stack: password_ptr, password_len, hash_ptr, hash_len -> 0 if valid, -1 otherwise
    PasswordVerify = 72,

    // -------------------------------------------------------------------------
    // Random Operations
    // -------------------------------------------------------------------------
    /// Generate cryptographically secure random bytes
    /// Stack: out_ptr, out_len -> bytes_generated or -1
    RandomBytes = 80,
    /// Generate random salt
    /// Stack: out_ptr, out_len -> salt_len or -1
    RandomSalt = 81,

    // -------------------------------------------------------------------------
    // Encoding Operations
    // -------------------------------------------------------------------------
    /// Encode to hex
    /// Stack: data_ptr, data_len, out_ptr, out_len -> encoded_len or -1
    EncodingToHex = 90,
    /// Decode from hex
    /// Stack: hex_ptr, hex_len, out_ptr, out_len -> decoded_len or -1
    EncodingFromHex = 91,
    /// Encode to Base64
    /// Stack: data_ptr, data_len, out_ptr, out_len -> encoded_len or -1
    EncodingToBase64 = 92,
    /// Decode from Base64
    /// Stack: base64_ptr, base64_len, out_ptr, out_len -> decoded_len or -1
    EncodingFromBase64 = 93,
    /// Encode to Base64 URL-safe
    /// Stack: data_ptr, data_len, out_ptr, out_len -> encoded_len or -1
    EncodingToBase64Url = 94,
    /// Decode from Base64 URL-safe
    /// Stack: base64_ptr, base64_len, out_ptr, out_len -> decoded_len or -1
    EncodingFromBase64Url = 95,

    // -------------------------------------------------------------------------
    // Nonce Tracking Operations
    // -------------------------------------------------------------------------
    /// Enable/disable nonce tracking
    /// Stack: enable -> previous_state
    NonceTrackingEnable = 100,
    /// Check if nonce was used
    /// Stack: key_ptr, key_len, nonce_ptr, nonce_len -> 0=not used, 1=used, -1=error
    NonceCheck = 101,
    /// Mark nonce as used
    /// Stack: key_ptr, key_len, nonce_ptr, nonce_len -> 0=new, 1=reuse, -1=error
    NonceMarkUsed = 102,
    /// Check and mark nonce atomically
    /// Stack: key_ptr, key_len, nonce_ptr, nonce_len -> 0=new, 1=reuse, -1=error
    NonceCheckAndMark = 103,
    /// Clear all tracked nonces
    /// Stack: -> count_cleared or -1
    NonceClear = 104,
}

impl HostCryptoOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            // Secure Memory
            0 => Some(Self::SecureAlloc),
            1 => Some(Self::SecureFree),
            2 => Some(Self::SecurePtr),
            3 => Some(Self::SecureLen),
            4 => Some(Self::SecureWrite),
            5 => Some(Self::SecureRead),
            6 => Some(Self::SecureZero),
            7 => Some(Self::SecureCompare),
            // Hash
            10 => Some(Self::Hash),
            11 => Some(Self::HasherNew),
            12 => Some(Self::HasherUpdate),
            13 => Some(Self::HasherFinalize),
            // HMAC
            20 => Some(Self::Hmac),
            21 => Some(Self::HmacNew),
            22 => Some(Self::HmacUpdate),
            23 => Some(Self::HmacFinalize),
            24 => Some(Self::HmacVerify),
            // AEAD
            30 => Some(Self::AeadGenerateNonce),
            31 => Some(Self::AeadEncrypt),
            32 => Some(Self::AeadDecrypt),
            // Signature
            40 => Some(Self::SignatureGenerateKeypair),
            41 => Some(Self::SignatureDerivePublicKey),
            42 => Some(Self::SignatureSign),
            43 => Some(Self::SignatureVerify),
            44 => Some(Self::SignatureSignHash),
            45 => Some(Self::SignatureVerifyHash),
            // Key Exchange
            50 => Some(Self::KexGenerateKeypair),
            51 => Some(Self::KexAgree),
            52 => Some(Self::KexAgreeWithKdf),
            // KDF
            60 => Some(Self::KdfHkdf),
            61 => Some(Self::KdfPbkdf2),
            62 => Some(Self::KdfArgon2),
            // Password
            70 => Some(Self::PasswordHashBcrypt),
            71 => Some(Self::PasswordHashArgon2),
            72 => Some(Self::PasswordVerify),
            // Random
            80 => Some(Self::RandomBytes),
            81 => Some(Self::RandomSalt),
            // Encoding
            90 => Some(Self::EncodingToHex),
            91 => Some(Self::EncodingFromHex),
            92 => Some(Self::EncodingToBase64),
            93 => Some(Self::EncodingFromBase64),
            94 => Some(Self::EncodingToBase64Url),
            95 => Some(Self::EncodingFromBase64Url),
            // Nonce Tracking
            100 => Some(Self::NonceTrackingEnable),
            101 => Some(Self::NonceCheck),
            102 => Some(Self::NonceMarkUsed),
            103 => Some(Self::NonceCheckAndMark),
            104 => Some(Self::NonceClear),
            _ => None,
        }
    }
}
