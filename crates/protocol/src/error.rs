use std::fmt;

/// Why bytes are not a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The frame ended before the message did.
    Truncated,
    /// Bytes were left after the message ended.
    Trailing(usize),
    /// A length prefix of zero or over [`crate::MAX_FRAME`].
    FrameLength(usize),
    UnknownKind(u8),
    UnknownRecord(u8),
    /// A name that is not UTF-8.
    Name,
    /// A kind the reading side never receives, such as a batch sent to the server.
    Unexpected(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the frame ends inside the message"),
            Self::Trailing(n) => write!(f, "{n} bytes follow the message"),
            Self::FrameLength(n) => write!(f, "a frame of {n} bytes"),
            Self::UnknownKind(k) => write!(f, "unknown message kind {k}"),
            Self::UnknownRecord(k) => write!(f, "unknown batch record {k}"),
            Self::Name => f.write_str("a name that is not UTF-8"),
            Self::Unexpected(k) => write!(f, "message kind {k} is not for this side"),
        }
    }
}

impl std::error::Error for Error {}
