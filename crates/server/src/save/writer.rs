use std::cell::Cell;
use std::ffi::c_int;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use rusqlite::hooks::Wal;

use super::file::{self, Opened};
use super::{Batch, Saving};
use crate::net::Held;

const CHECKPOINT_EVERY: u32 = 20;
const WAL_PAGE_BYTES: u32 = 4096;

thread_local! {
    static WAL_PAGES: Cell<c_int> = const { Cell::new(0) };
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "SQLite's hook is handed a result to return"
)]
fn on_wal(_: &Wal, pages: c_int) -> rusqlite::Result<()> {
    WAL_PAGES.set(pages);
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Commit {
    pub tick: u32,
    pub rows: u32,
    /// The values written, at eight bytes a number and a name's length.
    pub value_bytes: u32,
    /// The bytes the write-ahead log took: whole pages.
    pub wal_bytes: u32,
    /// From the tick handing its changes over to their being durable.
    pub durable_ns: u64,
    /// The transaction alone, from its first statement to the end of its commit.
    pub commit_ns: u64,
}

struct Handed {
    batch: Batch,
    at: Instant,
}

enum Job {
    Save(Handed),
    Release(u32, Vec<Held>),
}

#[derive(Default)]
struct State {
    released: Option<u32>,
    failed: Option<String>,
    commits: Vec<Commit>,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

fn state(shared: &Shared) -> MutexGuard<'_, State> {
    shared.0.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Writer {
    jobs: Option<mpsc::Sender<Job>>,
    shared: Shared,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Writer {
    pub fn start(opened: Opened, saving: Saving) -> std::io::Result<Self> {
        let shared: Shared = Arc::default();
        let (jobs, rx) = mpsc::channel();
        let path = opened.path.clone();
        let theirs = shared.clone();
        let thread = std::thread::Builder::new()
            .name("save".into())
            .spawn(move || write_on(opened, &rx, saving, &theirs))?;
        Ok(Self {
            jobs: Some(jobs),
            shared,
            thread: Some(thread),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self, batch: Batch) {
        if batch.rows() > 0 {
            let at = Instant::now();
            self.send(Job::Save(Handed { batch, at }));
        }
    }

    /// Hands over a tick's results, which leave once every change handed over before is durable.
    pub fn release(&self, tick: u32, held: Vec<Held>) {
        self.send(Job::Release(tick, held));
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    pub fn wait_released(&self, tick: u32) -> Result<Duration, String> {
        let started = Instant::now();
        let mut s = state(&self.shared);
        loop {
            if let Some(why) = &s.failed {
                return Err(why.clone());
            }
            if s.released.is_some_and(|r| r >= tick) {
                return Ok(started.elapsed());
            }
            s = self
                .shared
                .1
                .wait(s)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub fn failed(&self) -> Option<String> {
        state(&self.shared).failed.clone()
    }

    pub fn take_commits(&self) -> Vec<Commit> {
        std::mem::take(&mut state(&self.shared).commits)
    }

    /// Makes everything handed over durable, lets out every result, and closes the file.
    pub fn finish(mut self) -> Result<Vec<Commit>, String> {
        self.stop();
        let mut s = state(&self.shared);
        match s.failed.take() {
            Some(why) => Err(why),
            None => Ok(std::mem::take(&mut s.commits)),
        }
    }

    fn stop(&mut self) {
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            state(&self.shared).failed = Some("the writer thread panicked".into());
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fail(shared: &Shared, why: String) {
    let mut s = state(shared);
    s.failed.get_or_insert(why);
    shared.1.notify_all();
}

struct FailOnPanic<'a>(&'a Shared);

impl Drop for FailOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            fail(self.0, "the writer thread panicked".into());
        }
    }
}

fn write_on(mut opened: Opened, jobs: &mpsc::Receiver<Job>, saving: Saving, shared: &Shared) {
    let _fail_on_panic = FailOnPanic(shared);
    let path = opened.path.clone();
    let at = |e: rusqlite::Error| format!("{}: {e}", path.display());
    opened.conn.wal_hook(Some(on_wal));
    if let Err(e) = opened
        .conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
    {
        return fail(shared, at(e));
    }
    let (kick, kicked) = mpsc::channel::<()>();
    let checkpointer = {
        let (path, shared) = (path.clone(), shared.clone());
        std::thread::Builder::new()
            .name("checkpoint".into())
            .spawn(move || {
                if let Err(why) = checkpoint(&path, &kicked) {
                    fail(&shared, why);
                }
            })
    };
    let (mut out, mut next): (Option<Handed>, Option<Handed>) = (None, None);
    let (mut dropped, mut made, mut pages) = (false, 0u32, 0);
    let mut commit = |conn: &mut Connection, Handed { batch, at: handed }: Handed| {
        let skip = match saving {
            Saving::Drops(from)
                if !dropped && batch.tick >= from && !batch.game_rows.is_empty() =>
            {
                1
            }
            _ => 0,
        };
        dropped |= skip > 0;
        let started = Instant::now();
        let wrote = file::write(conn, opened.table.as_ref(), &batch, skip).map_err(at)?;
        let now = WAL_PAGES.get();
        let taken = if now >= pages { now - pages } else { now };
        pages = now;
        made += 1;
        if made.is_multiple_of(CHECKPOINT_EVERY) {
            let _ = kick.send(());
        }
        state(shared).commits.push(Commit {
            tick: batch.tick,
            rows: wrote.rows,
            value_bytes: wrote.value_bytes,
            wal_bytes: taken.unsigned_abs() * WAL_PAGE_BYTES,
            durable_ns: handed.elapsed().as_nanos() as u64,
            commit_ns: started.elapsed().as_nanos() as u64,
        });
        Ok::<(), String>(())
    };
    while let Ok(job) = jobs.recv() {
        let done = match job {
            Job::Save(handed) if saving == Saving::Early => {
                next = Some(handed);
                Ok(())
            }
            Job::Save(handed) => commit(&mut opened.conn, handed),
            Job::Release(tick, held) => {
                for frame in held {
                    frame.send();
                }
                state(shared).released = Some(tick);
                shared.1.notify_all();
                if next.as_ref().is_some_and(|h| h.batch.tick == tick) {
                    let was = std::mem::replace(&mut out, next.take());
                    was.map_or(Ok(()), |handed| commit(&mut opened.conn, handed))
                } else {
                    Ok(())
                }
            }
        };
        if let Err(why) = done {
            return fail(shared, why);
        }
    }
    for handed in [out.take(), next.take()].into_iter().flatten() {
        if let Err(why) = commit(&mut opened.conn, handed) {
            return fail(shared, why);
        }
    }
    drop(kick);
    match checkpointer.map(JoinHandle::join) {
        Ok(Ok(())) => {}
        _ => fail(shared, "the checkpoint thread failed".into()),
    }
    if let Err((_, e)) = opened.conn.close() {
        fail(shared, at(e));
    }
}

fn checkpoint(path: &Path, kicked: &mpsc::Receiver<()>) -> Result<(), String> {
    let at = |e: rusqlite::Error| format!("{}: {e}", path.display());
    let conn = Connection::open(path).map_err(at)?;
    conn.execute_batch(
        "PRAGMA synchronous = FULL; PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(at)?;
    while kicked.recv().is_ok() {
        while kicked.try_recv().is_ok() {}
        conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
            .map_err(at)?;
    }
    Ok(())
}
