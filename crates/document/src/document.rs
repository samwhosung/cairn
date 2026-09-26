use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::build::{Built, build};
use crate::command::{Command, New};
use crate::edit;
use crate::files::{self, ZoneFile};
use crate::history::{CommandRecord, History};
use crate::image::{Footprint, Image};
use crate::install::Install;
use crate::journal::{self, Entry, Head, Journal, Kind, line_text};
use crate::steps::{Step, Steps};
use crate::zone::{Zone, check_author};

/// A zone open for editing. While it is, another process that opens the zone is refused.
pub struct Document {
    dir: PathBuf,
    zone: Zone,
    journal: Journal,
    history: History,
    steps: Steps,
    dirty: BTreeSet<ZoneFile>,
    saved: usize,
    _lock: File,
}

#[derive(Debug)]
pub struct Done {
    pub reply: String,
    pub footprint: Footprint,
}

fn lock(dir: &Path) -> Result<File, String> {
    let path = dir.join(".lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "{} is open for editing in another process",
            dir.display()
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("{}: {e}", path.display())),
    }
}

fn parts_of(i: &Image) -> impl Iterator<Item = ZoneFile> {
    [
        (!i.heights.is_empty(), ZoneFile::Heights),
        (!i.paint.is_empty(), ZoneFile::Paint),
        (!i.effects.is_empty(), ZoneFile::Palette),
        (!i.things.is_empty(), ZoneFile::Things),
        (!i.water.is_empty(), ZoneFile::Water),
        (i.settings.is_some(), ZoneFile::Zone),
    ]
    .into_iter()
    .filter_map(|(yes, p)| yes.then_some(p))
}

impl Document {
    pub fn zone(&self) -> &Zone {
        &self.zone
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Journal lines so far, the first the one that made the zone.
    pub fn lines(&self) -> &[String] {
        &self.journal.lines
    }

    /// Make a zone in `dir`, which must hold none.
    pub fn create(
        dir: &Path,
        author: &str,
        new: &New,
        install: &mut dyn Install,
    ) -> Result<(Document, String), String> {
        Self::create_at(dir, &journal::now(), author, new, install)
    }

    fn create_at(
        dir: &Path,
        time: &str,
        author: &str,
        new: &New,
        install: &mut dyn Install,
    ) -> Result<(Document, String), String> {
        check_author(author)?;
        if dir.join(journal::FILE).exists() {
            return Err(format!("{} already holds a zone", dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let lock = lock(dir)?;
        let (zone, reply) = edit::create(new, install)?;
        let mut journal = Journal::create(dir)?;
        let history = History::create(dir)?;
        let entry = Entry::Command {
            join: false,
            command: Command::New(new.clone()),
        };
        journal.append(&[line_text(time, author, &entry)])?;
        files::save(dir, &zone, &ZoneFile::ALL, 1)?;
        let doc = Document {
            dir: dir.to_path_buf(),
            zone,
            journal,
            history,
            steps: Steps::default(),
            dirty: BTreeSet::new(),
            saved: 1,
            _lock: lock,
        };
        Ok((doc, reply))
    }

    /// Open the zone in `dir` as of its journal's last line.
    pub fn open(dir: &Path, install: &mut dyn Install) -> Result<Document, String> {
        let lock = lock(dir)?;
        files::finish(dir)?;
        let journal = Journal::open(dir)?;
        let first = journal
            .lines
            .first()
            .ok_or_else(|| format!("{}: an empty journal", dir.display()))?;
        let mut history = History::open(dir, journal.lines.len())?;
        let mut steps = Steps::default();
        let (mut zone, from, saved) = match files::saved_line(dir) {
            Ok(n) if (1..=journal.lines.len()).contains(&n) => (files::load(dir)?, n, n),
            Ok(n) => {
                return Err(format!(
                    "{}: its files hold line {n} of a journal of {}",
                    dir.display(),
                    journal.lines.len()
                ));
            }
            Err(_) => match journal::parse(first)?.entry {
                Entry::Command {
                    command: Command::New(n),
                    ..
                } => (edit::create(&n, install)?.0, 1, 0),
                _ => {
                    return Err(format!(
                        "{}: its journal starts with no `new`",
                        dir.display()
                    ));
                }
            },
        };
        for (k, text) in journal.lines.iter().enumerate().skip(1) {
            let line = k + 1;
            let at = |e: String| format!("{} line {line}: {e}", journal::FILE);
            let head = journal::head(text).map_err(at)?;
            let step = steps.follow(line, &head).map_err(at)?;
            if line > from {
                roll_forward(&mut zone, &mut history, text, &head, line, &step, install)
                    .map_err(at)?;
            }
        }
        let behind = journal.lines.len() > saved;
        Ok(Document {
            dir: dir.to_path_buf(),
            zone,
            journal,
            history,
            steps,
            dirty: if behind {
                ZoneFile::ALL.into()
            } else {
                BTreeSet::new()
            },
            saved,
            _lock: lock,
        })
    }

    /// Apply a command for `author`, as a step of its own or, with `join`, as part of the
    /// author's last step when their last line was a command.
    pub fn apply(
        &mut self,
        author: &str,
        cmd: &Command,
        join: bool,
        install: &mut dyn Install,
    ) -> Result<Done, String> {
        self.batch_at(
            &journal::now(),
            author,
            std::slice::from_ref(cmd),
            join,
            install,
        )
        .map(|mut r| r.remove(0))
    }

    /// Apply commands as one step, all of them or none.
    pub fn batch(
        &mut self,
        author: &str,
        cmds: &[Command],
        install: &mut dyn Install,
    ) -> Result<Vec<Done>, String> {
        self.batch_at(&journal::now(), author, cmds, false, install)
    }

    fn batch_at(
        &mut self,
        time: &str,
        author: &str,
        cmds: &[Command],
        join: bool,
        install: &mut dyn Install,
    ) -> Result<Vec<Done>, String> {
        check_author(author)?;
        if cmds.is_empty() {
            return Err("no command".into());
        }
        let (authors, palette) = (self.zone.authors.clone(), self.zone.palette.len());
        let mut done: Vec<(String, CommandRecord)> = Vec::with_capacity(cmds.len());
        let mut written: Vec<Command> = Vec::with_capacity(cmds.len());
        let mut failed = None;
        for c in cmds {
            match edit::apply(&mut self.zone, author, c, install) {
                Ok(a) => {
                    let after_digest = a.before.capture(&self.zone).digest();
                    written.push(a.journaled);
                    done.push((
                        a.reply,
                        CommandRecord {
                            before: a.before,
                            after_digest,
                        },
                    ));
                }
                Err(e) => {
                    failed = Some(if cmds.len() > 1 {
                        format!("{c}: {e}; nothing of the batch was applied")
                    } else {
                        e
                    });
                    break;
                }
            }
        }
        let join = join && self.steps.joinable(author);
        let first = self.journal.lines.len() + 1;
        let lines = match failed {
            Some(e) => Err(e),
            None => self.write_lines(time, author, &written, join, first, &done),
        };
        if let Err(e) = lines {
            for (_, f) in done.iter().rev() {
                f.before.restore(&mut self.zone);
            }
            self.zone.authors = authors;
            self.zone.palette.truncate(palette);
            return Err(e);
        }
        if self.zone.authors != authors {
            self.dirty.insert(ZoneFile::Zone);
        }
        if self.zone.palette.len() != palette {
            self.dirty.insert(ZoneFile::Palette);
        }
        for (i, (_, record)) in done.iter().enumerate() {
            let kind = if i > 0 || join {
                Kind::Join
            } else {
                Kind::Step
            };
            let head = Head {
                author: author.to_owned(),
                kind,
            };
            self.steps.follow(first + i, &head)?;
            self.dirty.extend(parts_of(&record.before));
        }
        Ok(done
            .into_iter()
            .map(|(reply, f)| Done {
                reply,
                footprint: f.before.footprint(&self.zone),
            })
            .collect())
    }

    fn write_lines(
        &mut self,
        time: &str,
        author: &str,
        cmds: &[Command],
        join: bool,
        first: usize,
        done: &[(String, CommandRecord)],
    ) -> Result<(), String> {
        let mut texts = Vec::with_capacity(cmds.len());
        for (i, (c, (_, f))) in cmds.iter().zip(done).enumerate() {
            self.history.write_forward(first + i, f)?;
            let entry = Entry::Command {
                join: i > 0 || join,
                command: c.clone(),
            };
            texts.push(line_text(time, author, &entry));
        }
        self.journal.append(&texts)
    }

    /// Take back `author`'s last step, unless someone else has changed its footprint since.
    pub fn undo(&mut self, author: &str) -> Result<Done, String> {
        self.undo_at(&journal::now(), author)
    }

    fn undo_at(&mut self, time: &str, author: &str) -> Result<Done, String> {
        let step = self
            .steps
            .next_undo(author)
            .cloned()
            .ok_or_else(|| format!("{author} has nothing to undo"))?;
        let mut taken: Vec<Image> = Vec::with_capacity(step.lines.len());
        let mut problem = None;
        for &l in step.lines.iter().rev() {
            let f = match self.history.forward(l) {
                Ok(f) => f,
                Err(e) => {
                    problem = Some(e);
                    break;
                }
            };
            let now = f
                .before
                .places_exist_in(&self.zone)
                .map(|()| f.before.capture(&self.zone));
            match now {
                Ok(now) if now.digest() == f.after_digest => {
                    f.before.restore(&mut self.zone);
                    taken.push(now);
                }
                Ok(_) => {
                    problem = Some(self.refusal(author, &step, &f.before, "undo"));
                    break;
                }
                Err(e) => {
                    problem = Some(e);
                    break;
                }
            }
        }
        let line = self.journal.lines.len() + 1;
        if problem.is_none() {
            taken.reverse();
            let written = self.history.write_taken_back(line, &taken).and_then(|()| {
                self.journal
                    .append(&[line_text(time, author, &Entry::Undo)])
            });
            taken.reverse();
            problem = written.err();
        }
        if let Some(e) = problem {
            for i in taken.iter().rev() {
                i.restore(&mut self.zone);
            }
            return Err(e);
        }
        let head = Head {
            author: author.to_owned(),
            kind: Kind::Undo,
        };
        self.steps.follow(line, &head)?;
        let mut footprint = Footprint::default();
        for i in &taken {
            self.dirty.extend(parts_of(i));
            footprint.add(i.footprint(&self.zone));
        }
        Ok(Done {
            reply: format!("took back {}", self.step_text(&step)),
            footprint,
        })
    }

    /// Put back `author`'s last step taken back, unless someone else has changed its footprint
    /// since.
    pub fn redo(&mut self, author: &str) -> Result<Done, String> {
        self.redo_at(&journal::now(), author)
    }

    fn redo_at(&mut self, time: &str, author: &str) -> Result<Done, String> {
        let step = self
            .steps
            .next_redo(author)
            .cloned()
            .ok_or_else(|| format!("{author} has nothing to redo"))?;
        let undone = step.undone_at.ok_or("a step taken back names no undo")?;
        let afters = self.history.taken_back(undone)?;
        if afters.len() != step.lines.len() {
            return Err(format!(
                "{}: line {undone}'s record does not fit its step",
                crate::history::FILE
            ));
        }
        let mut put: Vec<Image> = Vec::with_capacity(step.lines.len());
        let mut problem = None;
        for (&l, after) in step.lines.iter().zip(&afters) {
            let before = match self.history.forward(l).and_then(|f| {
                after.places_exist_in(&self.zone)?;
                Ok(f.before)
            }) {
                Ok(b) => b,
                Err(e) => {
                    problem = Some(e);
                    break;
                }
            };
            let (mut now, mut want) = (Vec::new(), Vec::new());
            before.capture(&self.zone).encode(&mut now);
            before.encode(&mut want);
            if now != want {
                problem = Some(self.refusal(author, &step, &before, "redo"));
                break;
            }
            after.restore(&mut self.zone);
            put.push(before);
        }
        let line = self.journal.lines.len() + 1;
        if problem.is_none() {
            problem = self
                .journal
                .append(&[line_text(time, author, &Entry::Redo)])
                .err();
        }
        if let Some(e) = problem {
            for b in put.iter().rev() {
                b.restore(&mut self.zone);
            }
            return Err(e);
        }
        let head = Head {
            author: author.to_owned(),
            kind: Kind::Redo,
        };
        self.steps.follow(line, &head)?;
        let mut footprint = Footprint::default();
        for i in &afters {
            self.dirty.extend(parts_of(i));
            footprint.add(i.footprint(&self.zone));
        }
        Ok(Done {
            reply: format!("put back {}", self.step_text(&step)),
            footprint,
        })
    }

    fn step_text(&self, step: &Step) -> String {
        let first = step
            .lines
            .first()
            .and_then(|&l| self.journal.lines.get(l - 1));
        let command = first
            .and_then(|t| journal::parse(t).ok())
            .map_or_else(String::new, |l| match l.entry {
                Entry::Command { command, .. } => command.to_string(),
                _ => String::new(),
            });
        match step.lines.len() {
            1 => format!("`{command}`"),
            n => format!("`{command}` and {} more of its step", n - 1),
        }
    }

    fn refusal(&mut self, author: &str, step: &Step, what: &Image, verb: &str) -> String {
        let mine = what.footprint(&self.zone);
        let from = step.undone_at.or(step.lines.last().copied()).unwrap_or(0);
        for l in (from + 1..=self.journal.lines.len()).rev() {
            let text = self.journal.lines[l - 1].clone();
            let Ok(head) = journal::head(&text) else {
                continue;
            };
            if head.author == author {
                continue;
            }
            let theirs = match head.kind {
                Kind::Step | Kind::Join => self.history.forward(l).map(|f| vec![f.before]),
                Kind::Undo => self.history.taken_back(l),
                Kind::Redo => continue,
            };
            let Ok(theirs) = theirs else { continue };
            if let Some(f) = theirs
                .iter()
                .map(|i| i.footprint(&self.zone))
                .find(|f| f.meets(&mine))
            {
                let shared = crate::image::Footprint {
                    chunks: f.chunks.intersection(&mine.chunks).copied().collect(),
                    things: f.things.intersection(&mine.things).cloned().collect(),
                    water: f.water.intersection(&mine.water).cloned().collect(),
                    effects: f.effects.intersection(&mine.effects).copied().collect(),
                    settings: f.settings && mine.settings,
                };
                return format!(
                    "{author} can't {verb} {}: {} changed {} since (journal line {l}: {text})",
                    self.step_text(step),
                    head.author,
                    shared.describe()
                );
            }
        }
        format!(
            "{author} can't {verb} {}: {} has changed since",
            self.step_text(step),
            mine.describe()
        )
    }

    /// Keep journal lines in memory until [`Document::release_journal`], so a crash before then
    /// loses them: for testing that a zone that writes as it goes loses nothing.
    pub fn hold_journal(&mut self) {
        self.journal.hold();
    }

    pub fn release_journal(&mut self) -> Result<(), String> {
        self.journal.release()
    }

    /// Write the zone's files as of the journal's last line.
    pub fn save(&mut self) -> Result<(), String> {
        let lines = self.journal.lines.len();
        if self.dirty.is_empty() && self.saved == lines {
            return Ok(());
        }
        let parts: Vec<ZoneFile> = self.dirty.iter().copied().collect();
        files::save(&self.dir, &self.zone, &parts, lines)?;
        self.dirty.clear();
        self.saved = lines;
        Ok(())
    }

    pub fn build(&self, install: &mut dyn Install) -> Result<Built, String> {
        build(&self.zone, install)
    }
}

fn roll_forward(
    zone: &mut Zone,
    history: &mut History,
    text: &str,
    head: &Head,
    line: usize,
    step: &Step,
    install: &mut dyn Install,
) -> Result<(), String> {
    match head.kind {
        Kind::Step | Kind::Join => {
            let l = journal::parse(text)?;
            let Entry::Command { command, .. } = l.entry else {
                return Err("not a command".into());
            };
            let applied = edit::apply(zone, &l.author, &command, install)?;
            if applied.before.capture(zone).digest() != history.forward(line)?.after_digest {
                return Err("applies differently from when it was written".into());
            }
        }
        Kind::Undo => {
            for &l in step.lines.iter().rev() {
                let f = history.forward(l)?;
                f.before.places_exist_in(zone)?;
                f.before.restore(zone);
            }
        }
        Kind::Redo => {
            let undone = step.undone_at.ok_or("a redo of a step never taken back")?;
            for i in history.taken_back(undone)? {
                i.places_exist_in(zone)?;
                i.restore(zone);
            }
        }
    }
    Ok(())
}

impl Document {
    /// Make a zone in `dir` from a journal's lines, each with the time and author it was written
    /// with.
    pub fn replay(
        lines: &[&str],
        dir: &Path,
        install: &mut dyn Install,
    ) -> Result<Document, String> {
        let (first, rest) = lines.split_first().ok_or("an empty journal")?;
        let l = journal::parse(first).map_err(|e| format!("line 1: {e}"))?;
        let Entry::Command {
            command: Command::New(new),
            ..
        } = l.entry
        else {
            return Err("line 1: a journal starts with `new`".into());
        };
        let (mut doc, _) = Self::create_at(dir, &l.time, &l.author, &new, install)?;
        for (k, text) in rest.iter().enumerate() {
            doc.replay_line(text, install)
                .map_err(|e| format!("line {}: {e}", k + 2))?;
        }
        Ok(doc)
    }

    pub(crate) fn replay_line(
        &mut self,
        text: &str,
        install: &mut dyn Install,
    ) -> Result<(), String> {
        let l = journal::parse(text)?;
        match l.entry {
            Entry::Command { join, command } => self
                .batch_at(
                    &l.time,
                    &l.author,
                    std::slice::from_ref(&command),
                    join,
                    install,
                )
                .map(drop),
            Entry::Undo => self.undo_at(&l.time, &l.author).map(drop),
            Entry::Redo => self.redo_at(&l.time, &l.author).map(drop),
        }
    }
}
