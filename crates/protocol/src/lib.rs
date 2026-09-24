//! The native wire between cairn's client and server: frames, messages and their encoding.
//!
//! Every message travels as one frame: a little-endian `u32` counting the bytes after it, a kind
//! byte, then the message. A client sends a [`Hello`] and then [`Claim`]s, and says which tick it
//! has seen now and then; the server answers with a [`Welcome`] and then one [`Batch`] per tick.
//! A claim carries the client's own movement, clock and all; a batch relays other entities'
//! movement with each position [`Wrapped`] to 16 bits an axis and the angles in [`Angle`]s.

mod appearance;
mod batch;
mod cadence;
mod error;
mod frame;
mod message;
mod movement;
mod pos;
mod reader;
mod relay;

pub use appearance::Appearance;
pub use batch::{
    Batch, Record, SLOTS, begin_batch, write_appear, write_correct, write_move, write_state,
    write_turn, write_vanish,
};
pub use cadence::{Cadence, HEARTBEAT_MS};
pub use error::Error;
pub use frame::{Frames, Kind, LEN_BYTES, MAX_FRAME, begin_frame, finish_frame};
pub use message::{Claim, ClientMessage, Hello, ServerMessage, Welcome};
pub use movement::{Jump, Movement, flags};
pub use pos::{Angle, Pos, STEPS_PER_YD, Wrapped};
pub use relay::{Changed, Intro, Relay, State};

pub const VERSION: u16 = 1;
