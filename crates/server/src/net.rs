use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use protocol::{ClientMessage, Frames, VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::world::{Input, Stamped};

const READ_BUF: usize = 16 << 10;

/// Where one connection's batches queue for its writer.
pub struct Outbox {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
}

impl Outbox {
    /// An outbox and the end its batches arrive at, with no connection behind it.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (Self { tx, queued }, rx)
    }

    pub fn send(&self, bytes: Vec<u8>) {
        self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
        if let Err(e) = self.tx.send(bytes) {
            self.queued.fetch_sub(e.0.len(), Ordering::Relaxed);
        }
    }

    /// Bytes handed to the connection that it has not written yet.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
}

/// What the connections and the tick share: the inputs waiting for the next tick, and the
/// outboxes of connections the world has not admitted yet.
pub struct Shared {
    inbox: Mutex<Vec<Stamped>>,
    waiting: Mutex<HashMap<u32, Outbox>>,
    next_conn: AtomicU32,
    pub bytes_in: AtomicU64,
    pub stop: AtomicBool,
    started: Instant,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            inbox: Mutex::new(Vec::new()),
            waiting: Mutex::new(HashMap::new()),
            next_conn: AtomicU32::new(0),
            bytes_in: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    /// Milliseconds since the server started: the clock inputs are stamped with.
    pub fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// Everything that arrived since the last call, in each connection's own order.
    pub fn take_inputs(&self) -> Vec<Stamped> {
        let mut inputs =
            std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        inputs.sort_unstable_by_key(|s| (s.conn, s.seq));
        inputs
    }

    /// Holds a new connection's outbox until the world admits it.
    pub fn hold_outbox(&self, conn: u32, outbox: Outbox) {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(conn, outbox);
    }

    /// The outbox of a connection the world is admitting.
    pub fn claim_outbox(&self, conn: u32) -> Option<Outbox> {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&conn)
    }

    fn push(&self, batch: &mut Vec<Stamped>) {
        if !batch.is_empty() {
            let mut inbox = self.inbox.lock().unwrap_or_else(PoisonError::into_inner);
            inbox.append(batch);
        }
    }
}

/// Accepts connections until the runtime shuts down.
pub async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            tokio::task::yield_now().await;
            continue;
        };
        let _ = socket.set_nodelay(true);
        let conn = shared.next_conn.fetch_add(1, Ordering::Relaxed);
        let (reader, writer) = socket.into_split();
        let (outbox, rx) = Outbox::channel();
        let queued = outbox.queued.clone();
        shared.hold_outbox(conn, outbox);
        tokio::spawn(write(writer, rx, queued));
        tokio::spawn(read(conn, reader, shared.clone()));
    }
}

async fn write(
    mut w: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    queued: Arc<AtomicUsize>,
) {
    while let Some(bytes) = rx.recv().await {
        if w.write_all(&bytes).await.is_err() {
            break;
        }
        queued.fetch_sub(bytes.len(), Ordering::Relaxed);
    }
}

/// Reads a connection's frames into the inbox: a hello first, then claims. Anything else, or
/// the connection closing, ends it with a leave.
async fn read(conn: u32, mut r: OwnedReadHalf, shared: Arc<Shared>) {
    let mut frames = Frames::default();
    let mut buf = vec![0u8; READ_BUF];
    let (mut seq, mut joined, mut batch) = (0u32, false, Vec::new());
    'conn: loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        shared.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
        frames.extend(&buf[..n]);
        let at_ms = shared.now_ms();
        loop {
            let input = match frames.next_frame().map(|f| f.map(ClientMessage::read)) {
                Ok(None) => break,
                Ok(Some(Ok(ClientMessage::Hello(h)))) if !joined && h.version == VERSION => {
                    joined = true;
                    Input::Join(h)
                }
                Ok(Some(Ok(ClientMessage::Claim(c)))) if joined => Input::Claim(c),
                _ => {
                    shared.push(&mut batch);
                    break 'conn;
                }
            };
            batch.push(Stamped {
                conn,
                seq,
                at_ms,
                input,
            });
            seq += 1;
        }
        shared.push(&mut batch);
    }
    if joined {
        let at_ms = shared.now_ms();
        batch.push(Stamped {
            conn,
            seq,
            at_ms,
            input: Input::Leave,
        });
        shared.push(&mut batch);
    }
    shared
        .waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&conn);
}
