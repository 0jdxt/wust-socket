/// Close reason codes as specified in
/// [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html#section-7.4)
#[repr(u16)]
#[derive(Debug)]
pub(crate) enum CloseReason {
    /// Normal close
    Normal = 1000,
    /// Going away
    GoingAway = 1001,
    /// Websocket protocol violation
    ProtoError = 1002,
    /// Unsupported data type
    DataType = 1003,
    /// Reserved code, should never be seen
    Rsv = 1004,
    /// No reason code provided
    NoneGiven = 1005,
    /// Abnormal closure
    Abnormal = 1006,
    /// Invalid UTF-8 in Text message
    DataError = 1007,
    /// Generic policy violation
    Policy = 1008,
    /// Messages are too big
    TooBig = 1009,
    /// Unsupported extensions
    Extension = 1010,
    /// An unexpected condition that prevented the request from being fulfilled
    Unexpected = 1011,
    /// TLS error
    Tls = 1015,
    /// Other valid codes with unknown meanings
    Unknown = 4000, // private use code
}

/// Converts a reason code to bytes of the appropriate endianness.
impl From<CloseReason> for [u8; 2] {
    fn from(value: CloseReason) -> Self { (value as u16).to_be_bytes() }
}

impl From<[u8; 2]> for CloseReason {
    fn from(bytes: [u8; 2]) -> Self {
        match u16::from_be_bytes(bytes) {
            1000 => CloseReason::Normal,
            1001 => CloseReason::GoingAway,
            1002 => CloseReason::ProtoError,
            1003 => CloseReason::DataType,
            1004 => CloseReason::Rsv,
            1005 => CloseReason::NoneGiven,
            1006 => CloseReason::Abnormal,
            1007 => CloseReason::DataError,
            1008 => CloseReason::Policy,
            1009 => CloseReason::TooBig,
            1010 => CloseReason::Extension,
            1011 => CloseReason::Unexpected,
            1015 => CloseReason::Tls,
            _ => CloseReason::Unknown,
        }
    }
}

/// Errors that can occur when upgrading a TCP stream to a WebSocket.
#[derive(Debug)]
pub enum UpgradeError {
    /// Tried to connect to invalid url.
    InvalidUrl,
    /// Failed to bind TCP listener.
    Bind,
    /// Failed to read from the TCP stream.
    Read,
    /// Failed to write to the TCP steam.
    Write,
    /// Server returned an unexpected HTTP status line.
    StatusLine(String),
    /// Missing header from upgrade request.
    MissingHeader(&'static str),
    /// A handshake header did not match expectations.
    Header {
        /// The name of the header field.
        field: &'static str,
        /// The expected value.
        expected: String,
        /// The actual value.
        got: String,
    },
    /// Failed to obtain the socket's peer address.
    ///
    /// This can happen if the socket is not connected, has been closed,
    /// or the underlying OS socket is invalid.
    Addr,
    /// Failed to establish TCP connection.
    Connect,
    /// Attempt to connect timed out.
    Timeout,
    /// Protocol mismatch
    Protocol,
}
