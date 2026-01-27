/// Close reason codes as specified in [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html)
#[repr(u16)]
pub enum CloseReason {
    /// Normal close
    Normal = 1000,
    /// Going away
    GoingAway = 1001,
    /// Websocket protocol violation
    ProtoError = 1002,
    /// Unsupported data type
    DataType = 1003,
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
    TLS = 1015,
}

/// Converts a reason code to bytes of the appropriate endianness.
impl From<CloseReason> for [u8; 2] {
    fn from(value: CloseReason) -> Self { (value as u16).to_be_bytes() }
}
