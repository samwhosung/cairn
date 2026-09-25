//! The connection to a server, over a socket on threads of its own or inside the server's own
//! process: a frame hands it what to send and takes what has arrived, and never waits on either.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use protocol::{ClientMessage, Frames, Hello};
use server::InProcess;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_BUF: usize = 64 << 10;

pub enum Arrival {
    Frame { bytes: Vec<u8>, arrived: Instant },
    Gone { reason: String },
}

pub struct Link(Way);

enum Way {
    Socket {
        out: Sender<Vec<u8>>,
        arrivals: Mutex<Receiver<Arrival>>,
        stream: Arc<Mutex<Option<TcpStream>>>,
    },
    Here(Mutex<Here>),
}

struct Here {
    server: InProcess,
    frames: Frames,
    gone: bool,
}

impl Link {
    /// Returns at once and connects on a thread of its own, `hello` first; what arrives, a failed
    /// connect included, waits for [`Link::arrivals`].
    pub fn open(addr: SocketAddr, hello: Hello) -> Self {
        let (out, to_send) = mpsc::channel();
        let (to_window, arrivals) = mpsc::channel();
        let stream = Arc::new(Mutex::new(None));
        let (held, gone) = (stream.clone(), to_window.clone());
        let spawned = thread::Builder::new().name("net".into()).spawn(move || {
            let reason = run(addr, &hello, &held, to_send, &to_window);
            let _ = to_window.send(Arrival::Gone { reason });
        });
        if let Err(e) = spawned {
            let reason = format!("no thread for the connection: {e}");
            let _ = gone.send(Arrival::Gone { reason });
        }
        Self(Way::Socket {
            out,
            arrivals: Mutex::new(arrivals),
            stream,
        })
    }

    pub fn in_process(server: InProcess, hello: Hello) -> Self {
        let link = Self(Way::Here(Mutex::new(Here {
            server,
            frames: Frames::default(),
            gone: false,
        })));
        link.send(&ClientMessage::Hello(hello));
        link
    }

    pub fn send(&self, message: &ClientMessage) {
        let mut bytes = Vec::new();
        message.write(&mut bytes);
        match &self.0 {
            Way::Socket { out, .. } => {
                let _ = out.send(bytes);
            }
            Way::Here(here) => {
                let _ = lock(here).server.send(bytes);
            }
        }
    }

    /// Everything that has arrived since the last call, in order.
    pub fn arrivals(&self) -> Vec<Arrival> {
        match &self.0 {
            Way::Socket { arrivals, .. } => lock(arrivals).try_iter().collect(),
            Way::Here(here) => lock(here).arrivals(),
        }
    }
}

impl Here {
    fn arrivals(&mut self) -> Vec<Arrival> {
        let mut got = Vec::new();
        while !self.gone {
            let (bytes, arrived) = match self.server.try_recv() {
                Ok(written) => written,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.gone = true;
                    let reason = "the server in this window stopped".into();
                    got.push(Arrival::Gone { reason });
                    break;
                }
            };
            self.frames.extend(&bytes);
            loop {
                match self.frames.next_frame() {
                    Ok(Some(frame)) => got.push(Arrival::Frame {
                        bytes: frame.to_vec(),
                        arrived,
                    }),
                    Ok(None) => break,
                    Err(e) => {
                        self.gone = true;
                        let reason = format!("the server in this window sent a broken frame: {e}");
                        got.push(Arrival::Gone { reason });
                        break;
                    }
                }
            }
        }
        got
    }
}

fn lock<T>(held: &Mutex<T>) -> MutexGuard<'_, T> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for Link {
    fn drop(&mut self) {
        let Way::Socket { stream, .. } = &self.0 else {
            return;
        };
        if let Some(stream) = lock(stream).as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn run(
    addr: SocketAddr,
    hello: &Hello,
    held: &Mutex<Option<TcpStream>>,
    to_send: Receiver<Vec<u8>>,
    to_window: &Sender<Arrival>,
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
    *lock(held) = stream.try_clone().ok();
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
        let arrived = Instant::now();
        frames.extend(&buf[..n]);
        loop {
            match frames.next_frame() {
                Ok(Some(frame)) => {
                    let bytes = frame.to_vec();
                    if to_window.send(Arrival::Frame { bytes, arrived }).is_err() {
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

#[cfg(test)]
mod tests {
    use protocol::{Appearance, VERSION};

    use super::*;

    #[test]
    fn a_frame_from_the_servers_own_process_bears_the_time_it_was_written() {
        let running = server::start(server::Config {
            addr: None,
            tick_threads: 1,
            io_threads: 1,
            ..server::Config::default()
        })
        .expect("a server");
        let hello = Hello {
            version: VERSION,
            name: "Host".into(),
            appearance: Appearance::default(),
        };
        let link = Link::in_process(running.connect_host(), hello);
        thread::sleep(Duration::from_millis(600));
        let read = Instant::now();
        let stamps: Vec<Instant> = link
            .arrivals()
            .into_iter()
            .map(|a| match a {
                Arrival::Frame { arrived, .. } => arrived,
                Arrival::Gone { reason } => panic!("{reason}"),
            })
            .collect();
        let (first, last) = (stamps[0], stamps[stamps.len() - 1]);
        eprintln!(
            "{} frames over {:?}, the first {:?} before they were read",
            stamps.len(),
            last - first,
            read - first
        );
        assert!(stamps.len() >= 8, "{} frames", stamps.len());
        assert!(
            last - first >= Duration::from_millis(350),
            "{:?}",
            last - first
        );
        drop(link);
        running.stop().expect("the server stops");
    }

    enum Route {
        InProcess,
        Loopback,
    }

    fn malformed(ack: u32) -> ClientMessage {
        ClientMessage::Claim(protocol::Claim {
            ack,
            movement: protocol::Movement {
                pos: [f32::NAN; 3],
                ..protocol::Movement::default()
            },
        })
    }

    fn correction_seq(bytes: &[u8]) -> Option<u32> {
        let Ok(protocol::ServerMessage::Batch(batch)) = protocol::ServerMessage::read(bytes) else {
            return None;
        };
        batch.flatten().find_map(|r| match r {
            protocol::Record::Correct { seq, .. } => Some(seq),
            _ => None,
        })
    }

    fn percentile(sorted: &[Duration], q: f64) -> f64 {
        sorted[((sorted.len() - 1) as f64 * q) as usize].as_secs_f64() * 1e6
    }

    #[test]
    #[ignore = "a measurement, for a release build"]
    fn the_cost_of_a_link_in_process_and_over_loopback() {
        const PINGS: usize = 2000;
        const IDLE: Duration = Duration::from_secs(10);
        const FRAME: Duration = Duration::from_millis(16);
        eprintln!("{}", server::load_average());
        for tick_ms in [1u16, 50] {
            for way in [Route::InProcess, Route::Loopback] {
                let running = server::start(server::Config {
                    addr: Some(std::net::SocketAddr::from(([127, 0, 0, 1], 0))),
                    tick_threads: 1,
                    io_threads: 1,
                    tick_ms,
                    ..server::Config::default()
                })
                .expect("a server");
                let hello = Hello {
                    version: VERSION,
                    name: "Probe".into(),
                    appearance: Appearance::default(),
                };
                let (name, link) = match way {
                    Route::InProcess => (
                        "in-process",
                        Link::in_process(running.connect_host(), hello),
                    ),
                    Route::Loopback => (
                        "over loopback",
                        Link::open(running.addr().expect("a listener"), hello),
                    ),
                };
                while link.arrivals().is_empty() {
                    thread::sleep(Duration::from_millis(1));
                }
                let (cpu, began) = (server::process_cpu_ns(), Instant::now());
                let mut frames = 0usize;
                while began.elapsed() < IDLE {
                    thread::sleep(FRAME);
                    frames += link.arrivals().len();
                }
                let idle_cpu = (server::process_cpu_ns() - cpu) as f64 / 1e9 / IDLE.as_secs_f64();
                let mut trips = Vec::with_capacity(PINGS);
                let mut ack = 0;
                if tick_ms == 1 {
                    for _ in 0..PINGS {
                        let sent = Instant::now();
                        link.send(&malformed(ack));
                        'answer: loop {
                            for a in link.arrivals() {
                                if let Arrival::Frame { bytes, arrived } = a
                                    && let Some(seq) = correction_seq(&bytes)
                                {
                                    ack = seq;
                                    trips.push(arrived - sent);
                                    break 'answer;
                                }
                            }
                            thread::yield_now();
                        }
                    }
                    trips.sort();
                }
                let trip = if trips.is_empty() {
                    "—".to_owned()
                } else {
                    format!(
                        "{:.1} / {:.1} µs",
                        percentile(&trips, 0.5),
                        percentile(&trips, 0.99)
                    )
                };
                eprintln!(
                    "{name}, {tick_ms} ms ticks: {:.0} frames a second, the process {:.2} % of a core \
                     idle; a claim refused and answered in {trip} (p50 / p99)",
                    frames as f64 / IDLE.as_secs_f64(),
                    idle_cpu * 100.0
                );
                drop(link);
                running.stop().expect("the server stops");
            }
        }
        eprintln!("{}", server::load_average());
    }
}
