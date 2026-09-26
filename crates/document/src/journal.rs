//! The journal: a line for every change to the zone, written as the change applies, with its time
//! and its author. `TIME AUTHOR COMMAND` is a command; `TIME AUTHOR + COMMAND` joins its author's
//! step before it, so one undo takes both back; `TIME AUTHOR undo` and `TIME AUTHOR redo` take
//! back and put back that author's own last step. The zone as of its last line is the zone.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::command::Command;
use crate::grammar::{join, split};
use crate::zone::check_author;

pub const FILE: &str = "journal.txt";

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Command { join: bool, command: Command },
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub author: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Step,
    Join,
    Undo,
    Redo,
}

pub fn line_text(time: &str, author: &str, entry: &Entry) -> String {
    match entry {
        Entry::Command { join: j, command } => {
            let words = command.words();
            format!(
                "{time} {author}{} {}",
                if *j { " +" } else { "" },
                join(&words)
            )
        }
        Entry::Undo => format!("{time} {author} undo"),
        Entry::Redo => format!("{time} {author} redo"),
    }
}

pub struct Line {
    pub time: String,
    pub author: String,
    pub entry: Entry,
}

struct Words {
    time: String,
    author: String,
    rest: Vec<String>,
}

fn words(line: &str) -> Result<Words, String> {
    let mut w = split(line)?;
    if w.len() < 3 {
        return Err("want `TIME AUTHOR COMMAND`".into());
    }
    let rest = w.split_off(2);
    let author = w.pop().unwrap_or_default();
    let time = w.pop().unwrap_or_default();
    check_author(&author)?;
    Ok(Words { time, author, rest })
}

pub fn head(line: &str) -> Result<Head, String> {
    let Words { author, rest, .. } = words(line)?;
    let kind = match rest.first().map(String::as_str) {
        Some("undo") if rest.len() == 1 => Kind::Undo,
        Some("redo") if rest.len() == 1 => Kind::Redo,
        Some("+") => Kind::Join,
        _ => Kind::Step,
    };
    Ok(Head { author, kind })
}

pub fn parse(line: &str) -> Result<Line, String> {
    let Words { time, author, rest } = words(line)?;
    let entry = match rest.first().map(String::as_str) {
        Some("undo") if rest.len() == 1 => Entry::Undo,
        Some("redo") if rest.len() == 1 => Entry::Redo,
        Some("+") => Entry::Command {
            join: true,
            command: Command::parse(&rest[1..], &author)?,
        },
        _ => Entry::Command {
            join: false,
            command: Command::parse(&rest, &author)?,
        },
    };
    Ok(Line {
        time,
        author,
        entry,
    })
}

pub struct Journal {
    path: PathBuf,
    file: File,
    pub lines: Vec<String>,
    held: Option<String>,
}

impl Journal {
    /// Open the journal in `dir`; a last line cut short by a crash is dropped from it.
    pub fn open(dir: &Path) -> Result<Journal, String> {
        let path = dir.join(FILE);
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        let bytes = std::fs::read(&path).map_err(io)?;
        let whole = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let text = std::str::from_utf8(&bytes[..whole])
            .map_err(|_| format!("{}: not text", path.display()))?;
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(io)?;
        if whole < bytes.len() {
            file.set_len(whole as u64).map_err(io)?;
        }
        let mut file = file;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::End(0)).map_err(io)?;
        Ok(Journal {
            path,
            file,
            lines,
            held: None,
        })
    }

    pub fn create(dir: &Path) -> Result<Journal, String> {
        let path = dir.join(FILE);
        let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Journal {
            path,
            file,
            lines: Vec::new(),
            held: None,
        })
    }

    /// Add lines, all in one write.
    pub fn append(&mut self, lines: &[String]) -> Result<(), String> {
        let mut text = String::new();
        for l in lines {
            text.push_str(l);
            text.push('\n');
        }
        match &mut self.held {
            Some(held) => held.push_str(&text),
            None => self
                .file
                .write_all(text.as_bytes())
                .map_err(|e| format!("{}: {e}", self.path.display()))?,
        }
        self.lines.extend(lines.iter().cloned());
        Ok(())
    }

    pub fn hold(&mut self) {
        self.held.get_or_insert_with(String::new);
    }

    pub fn release(&mut self) -> Result<(), String> {
        match self.held.take() {
            Some(text) => self
                .file
                .write_all(text.as_bytes())
                .map_err(|e| format!("{}: {e}", self.path.display())),
            None => Ok(()),
        }
    }
}

/// Now, in UTC to the millisecond: `2026-09-26T14:03:12.345Z`.
pub fn now() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (secs, ms) = (t.as_secs() as i64, t.subsec_millis());
    let (days, s) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_back() {
        let c = Command::parse(&split("move 3 1,2").expect("words"), "ai").expect("a move");
        let line = line_text(
            "2026-09-26T14:03:12.345Z",
            "ai",
            &Entry::Command {
                join: true,
                command: c.clone(),
            },
        );
        assert_eq!(line, "2026-09-26T14:03:12.345Z ai + move ai.3 1,2");
        let parsed = parse(&line).expect("a line");
        assert_eq!(parsed.author, "ai");
        assert_eq!(
            parsed.entry,
            Entry::Command {
                join: true,
                command: c
            }
        );
        assert_eq!(head("t sam undo").map(|h| h.kind), Ok(Kind::Undo));
        assert!(parse("t 2x undo").is_err());
        assert!(
            parse("t 2x undo")
                .err()
                .is_some_and(|e| e.contains("author"))
        );
        assert_eq!(now().len(), 24);
    }
}
