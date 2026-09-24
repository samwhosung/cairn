//! The connection to a server on threads of its own: a frame hands it what to send and takes what
//! has arrived, and never waits on the socket.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use protocol::{ClientMessage, Frames, Hello};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_BUF: usize = 64 << 10;

pub enum Arrival {
    /// A frame, and when its bytes came off the socket.
    Frame {
        bytes: Vec<u8>,
        at: Instant,
    },
    Gone {
        reason: String,
    },
}

pub struct Link {
    out: Sender<Vec<u8>>,
    arrivals: Mutex<Receiver<Arrival>>,
    stream: Arc<Mutex<Option<TcpStream>>>,
}

impl Link {
    /// Returns at once and connects on a thread of its own, `hello` first; what arrives, a failed
    /// connect included, waits for [`Link::arrivals`].
    pub fn open(addr: SocketAddr, hello: Hello) -> Self {
        let (out, to_send) = mpsc::channel();
        let (arrived, arrivals) = mpsc::channel();
        let stream = Arc::new(Mutex::new(None));
        let (held, gone) = (stream.clone(), arrived.clone());
        let spawned = thread::Builder::new().name("net".into()).spawn(move || {
            let reason = run(addr, &hello, &held, to_send, &arrived);
            let _ = arrived.send(Arrival::Gone { reason });
        });
        if let Err(e) = spawned {
            let reason = format!("no thread for the connection: {e}");
            let _ = gone.send(Arrival::Gone { reason });
        }
        Self {
            out,
            arrivals: Mutex::new(arrivals),
            stream,
        }
    }

    pub fn send(&self, message: &ClientMessage) {
        let mut bytes = Vec::new();
        message.write(&mut bytes);
        let _ = self.out.send(bytes);
    }

    /// Everything that has arrived since the last call, in order.
    pub fn arrivals(&self) -> Vec<Arrival> {
        self.arrivals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let held = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = held.as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn run(
    addr: SocketAddr,
    hello: &Hello,
    held: &Mutex<Option<TcpStream>>,
    to_send: Receiver<Vec<u8>>,
    arrived: &Sender<Arrival>,
) -> String {
    let mut stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
        Ok(s) => s,
        Err(e) => return format!("could not reach {addr}: {e}"),
    };
    let _ = stream.set_nodelay(true);
    let writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => return format!("the connection to {addr}: {e}"),
    };
    *held.lock().unwrap_or_else(PoisonError::into_inner) = stream.try_clone().ok();
    let mut first = Vec::new();
    ClientMessage::Hello(hello.clone()).write(&mut first);
    let spawned = thread::Builder::new()
        .name("net-out".into())
        .spawn(move || write(writer, first, &to_send));
    if let Err(e) = spawned {
        return format!("no thread to write to {addr}: {e}");
    }
    let mut frames = Frames::default();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => return format!("{addr} closed the connection"),
            Ok(n) => n,
            Err(e) => return format!("the connection to {addr}: {e}"),
        };
        let at = Instant::now();
        frames.extend(&buf[..n]);
        loop {
            match frames.next_frame() {
                Ok(Some(frame)) => {
                    let bytes = frame.to_vec();
                    if arrived.send(Arrival::Frame { bytes, at }).is_err() {
                        return "the client stopped listening".into();
                    }
                }
                Ok(None) => break,
                Err(e) => return format!("{addr} sent a broken frame: {e}"),
            }
        }
    }
}

fn write(mut stream: TcpStream, first: Vec<u8>, to_send: &Receiver<Vec<u8>>) {
    let mut next = Some(first);
    while let Some(bytes) = next.take().or_else(|| to_send.recv().ok()) {
        if stream.write_all(&bytes).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}
