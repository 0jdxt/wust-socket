/// Close reason codes as specified in
/// [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html#section-7.4)
#[repr(u16)]
#[derive(Debug)]
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

impl From<[u8; 2]> for CloseReason {
    fn from(bytes: [u8; 2]) -> Self {
        match u16::from_be_bytes(bytes) {
            1000 => CloseReason::Normal,
            1001 => CloseReason::GoingAway,
            1002 => CloseReason::ProtoError,
            1003 => CloseReason::DataType,
            1005 => CloseReason::NoneGiven,
            1006 => CloseReason::Abnormal,
            1007 => CloseReason::DataError,
            1008 => CloseReason::Policy,
            1009 => CloseReason::TooBig,
            1010 => CloseReason::Extension,
            1011 => CloseReason::Unexpected,
            1015 => CloseReason::TLS,
            _ => unreachable!(),
        }
    }
}
