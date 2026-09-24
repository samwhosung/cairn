//! The native wire between cairn's client and server: frames, messages and their encoding.
//!
//! Every message travels as one frame: a little-endian `u32` counting the bytes after it, a kind
//! byte, then the message. A client sends a [`Hello`] and then [`Claim`]s, and says which tick it
//! has seen now and then; the server answers with a [`Welcome`] and then one [`Batch`] per tick.

mod appearance;
mod batch;
mod error;
mod frame;
mod message;
mod movement;
mod reader;

pub use appearance::Appearance;
pub use batch::{
    Batch, Record, begin_batch, write_appear, write_correct, write_move, write_vanish,
};
pub use error::Error;
pub use frame::{Frames, Kind, LEN_BYTES, MAX_FRAME, begin_frame, finish_frame};
pub use message::{Claim, ClientMessage, Hello, ServerMessage, Welcome};
pub use movement::{Jump, Movement, flags};

pub const VERSION: u16 = 0;
