use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, SystemTime};

const LOOK_EVERY: Duration = Duration::from_millis(200);
const EXTENSION: &str = "txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub lines: Vec<Line>,
    pub changed: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// An install path.
    pub path: String,
    /// What to say of it, or nothing.
    pub said: String,
}

pub struct Lists {
    pub all: BTreeMap<String, Named>,
    /// For each list, how long after its file changed it was taken in.
    pub late: BTreeMap<String, Duration>,
    news: Mutex<Receiver<News>>,
}

enum News {
    Changed(Named),
    Gone(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    changed: SystemTime,
    len: u64,
}

impl Lists {
    /// Starts looking at `dir`, which need not exist yet.
    pub fn watch(dir: Option<PathBuf>) -> Self {
        let (send, news) = channel();
        if let Some(dir) = dir {
            std::thread::spawn(move || look(&dir, &send));
        }
        Self {
            all: BTreeMap::new(),
            late: BTreeMap::new(),
            news: Mutex::new(news),
        }
    }

    /// Takes in what changed since the last call; says whether anything did.
    pub fn take_news(&mut self) -> bool {
        let Ok(news) = self.news.lock() else {
            return false;
        };
        let mut any = false;
        loop {
            match news.try_recv() {
                Ok(News::Changed(list)) => {
                    let late = SystemTime::now()
                        .duration_since(list.changed)
                        .unwrap_or_default();
                    self.late.insert(list.name.clone(), late);
                    self.all.insert(list.name.clone(), list);
                }
                Ok(News::Gone(name)) => {
                    self.all.remove(&name);
                    self.late.remove(&name);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return any,
            }
            any = true;
        }
    }
}

fn look(dir: &Path, send: &Sender<News>) {
    let mut seen: BTreeMap<String, Stamp> = BTreeMap::new();
    loop {
        let now = files(dir);
        for (name, (path, stamp)) in &now {
            if seen.get(name) == Some(stamp) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            seen.insert(name.clone(), *stamp);
            let list = Named {
                name: name.clone(),
                lines: parse(&text),
                changed: stamp.changed,
            };
            if send.send(News::Changed(list)).is_err() {
                return;
            }
        }
        let gone: Vec<String> = seen
            .keys()
            .filter(|name| !now.contains_key(*name))
            .cloned()
            .collect();
        for name in gone {
            seen.remove(&name);
            if send.send(News::Gone(name)).is_err() {
                return;
            }
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

fn files(dir: &Path) -> BTreeMap<String, (PathBuf, Stamp)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_str()?.to_owned();
            let listed = path.extension().is_some_and(|e| e == EXTENSION);
            if !listed || name.starts_with('.') {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let stamp = Stamp {
                changed: meta.modified().ok()?,
                len: meta.len(),
            };
            Some((name, (path, stamp)))
        })
        .collect()
}

/// A line a thing, `#` opening a comment: an install path, then after a tab what to say of it.
pub fn parse(text: &str) -> Vec<Line> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| {
            let (path, said) = line.split_once('\t').unwrap_or((line, ""));
            Line {
                path: path.trim().to_owned(),
                said: said.trim().to_owned(),
            }
        })
        .collect()
}
