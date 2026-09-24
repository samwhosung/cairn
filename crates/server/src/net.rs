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

pub struct Outbox {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued_bytes: Arc<AtomicUsize>,
    /// One more than the latest tick the client has seen; 0 until it says.
    seen: Arc<AtomicU32>,
}

impl Outbox {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(AtomicU32::new(0));
        let outbox = Self {
            tx,
            queued_bytes,
            seen,
        };
        (outbox, rx)
    }

    #[cfg(test)]
    pub fn seen_by(&self) -> impl Fn(u32) + use<> {
        let seen = self.seen.clone();
        move |tick| {
            seen.fetch_max(tick + 1, Ordering::Relaxed);
        }
    }

    /// How many ticks behind `tick` the client says it is, once it has said.
    pub fn behind(&self, tick: u32) -> Option<u32> {
        match self.seen.load(Ordering::Relaxed) {
            0 => None,
            seen => Some((tick + 1).saturating_sub(seen)),
        }
    }

    pub fn send(&self, bytes: Vec<u8>) {
        self.queued_bytes.fetch_add(bytes.len(), Ordering::Relaxed);
        if let Err(e) = self.tx.send(bytes) {
            self.queued_bytes.fetch_sub(e.0.len(), Ordering::Relaxed);
        }
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes.load(Ordering::Relaxed)
    }
}

pub struct Shared {
    inbox: Mutex<Vec<Stamped>>,
    unadmitted: Mutex<HashMap<u32, Outbox>>,
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
            unadmitted: Mutex::new(HashMap::new()),
            next_conn: AtomicU32::new(0),
            bytes_in: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    pub fn ms_since_start(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    /// Everything that arrived since the last call, in each connection's own order.
    pub fn take_inputs(&self) -> Vec<Stamped> {
        let mut inputs =
            std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        inputs.sort_unstable_by_key(|s| (s.conn, s.seq));
        inputs
    }

    pub fn hold_outbox(&self, conn: u32, outbox: Outbox) {
        self.unadmitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(conn, outbox);
    }

    pub fn take_outbox(&self, conn: u32) -> Option<Outbox> {
        self.unadmitted
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
        let (queued, seen) = (outbox.queued_bytes.clone(), outbox.seen.clone());
        shared.hold_outbox(conn, outbox);
        tokio::spawn(write(writer, rx, queued));
        tokio::spawn(read(conn, reader, seen, shared.clone()));
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

async fn read(conn: u32, mut r: OwnedReadHalf, seen: Arc<AtomicU32>, shared: Arc<Shared>) {
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
        let received_ms = shared.ms_since_start();
        loop {
            let input = match frames.next_frame().map(|f| f.map(ClientMessage::read)) {
                Ok(None) => break,
                Ok(Some(Ok(ClientMessage::Hello(h)))) if !joined && h.version == VERSION => {
                    joined = true;
                    Input::Join(h)
                }
                Ok(Some(Ok(ClientMessage::Claim(c)))) if joined => Input::Claim(c),
                Ok(Some(Ok(ClientMessage::Seen(tick)))) if joined => {
                    seen.fetch_max(tick.saturating_add(1), Ordering::Relaxed);
                    continue;
                }
                _ => {
                    shared.push(&mut batch);
                    break 'conn;
                }
            };
            batch.push(Stamped {
                conn,
                seq,
                received_ms,
                input,
            });
            seq += 1;
        }
        shared.push(&mut batch);
    }
    if joined {
        let received_ms = shared.ms_since_start();
        batch.push(Stamped {
            conn,
            seq,
            received_ms,
            input: Input::Leave,
        });
        shared.push(&mut batch);
    }
    shared
        .unadmitted
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&conn);
}
