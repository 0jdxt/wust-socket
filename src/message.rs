/// Assembled messages recieved from an endpoint.
#[derive(Debug)]
pub enum Message {
    /// Valid UTF-8 message.
    Text(String),
    /// Binary message bytes.
    Binary(Vec<u8>),
}

impl Message {
    /// If the type is `Message::Text`, returns a reference to the internal `String`, otherwise
    /// `None`.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Message::Binary(..) => None,
            Message::Text(s) => Some(s),
        }
    }

    /// Returns a reference to the data as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Message::Binary(b) => b,
            Message::Text(s) => s.as_bytes(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Message::Binary(b) => b.len(),
            Message::Text(s) => s.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

#[derive(Debug)]
pub(crate) enum PartialMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
}

impl PartialMessage {
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        match self {
            Self::Text(buf) | Self::Binary(buf) => buf.extend_from_slice(bytes),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Text(buf) | Self::Binary(buf) => buf.len(),
        }
    }
}
