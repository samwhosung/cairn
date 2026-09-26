use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, SystemTime};

/// How often the folder is looked at.
pub const EVERY: Duration = Duration::from_millis(200);
const EXTENSION: &str = "txt";

/// A list someone named and wrote into the folder, as its file says: a thing a line, an install
/// path, and after a tab what to say of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub name: String,
    pub lines: Vec<(String, String)>,
    /// When its file last changed.
    pub changed: SystemTime,
}

/// The lists in a folder, as they last were, kept up to date by a thread that looks at it.
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

/// Every [`EVERY`], the folder's lists that are new, changed or gone, until no one listens.
fn look(dir: &Path, send: &Sender<News>) {
    let mut seen: BTreeMap<String, (SystemTime, u64)> = BTreeMap::new();
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
                changed: stamp.0,
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
        std::thread::sleep(EVERY);
    }
}

type Stamped = BTreeMap<String, (PathBuf, (SystemTime, u64))>;

/// The folder's list files by name, each with when it last changed and its length.
fn files(dir: &Path) -> Stamped {
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
            Some((name, (path, (meta.modified().ok()?, meta.len()))))
        })
        .collect()
}

/// A line a thing, `#` opening a comment: an install path, then after a tab what to say of it.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| match line.split_once('\t') {
            Some((path, said)) => (path.trim().to_owned(), said.trim().to_owned()),
            None => (line.trim().to_owned(), String::new()),
        })
        .collect()
}
