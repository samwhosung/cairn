use std::any::{Any, TypeId};
use std::collections::BTreeMap;

use rayon::prelude::*;

use crate::engine::{Clock, NEVER};
use crate::out::{Fate, Out, STEP};
use crate::record::Record;
use crate::{Bytes, Game, Id, Kind, Letter, Schema, Tick, World, canon};

const ROWS_PER_TASK: usize = 256;
const TOUCHED: u8 = 1;
const GONE: u8 = 2;

/// One kind's rows as last tick left them, by ascending number.
pub struct Table<K> {
    ns: Vec<u32>,
    rows: Vec<K>,
}

impl<K> Table<K> {
    pub fn get(&self, n: u32) -> Option<&K> {
        let i = if self.ns.get(n as usize) == Some(&n) {
            n as usize
        } else {
            self.ns.binary_search(&n).ok()?
        };
        Some(&self.rows[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &K)> {
        self.ns.iter().copied().zip(&self.rows)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

struct Live<K> {
    ns: Vec<u32>,
    rows: Vec<K>,
    due: Vec<Tick>,
    marks: Vec<u8>,
}

pub(crate) struct Pair<G: Game> {
    pub kind: TypeId,
    pub schema: Option<Schema>,
    pub prev: Box<dyn Any + Send + Sync>,
    pub live: Box<dyn Rows<G>>,
}

impl<G: Game> Pair<G> {
    pub fn of<K: Kind<G>>() -> Self {
        Self {
            kind: TypeId::of::<K>(),
            schema: Schema::of::<K::Saved>(),
            prev: Box::new(Table::<K> {
                ns: Vec::new(),
                rows: Vec::new(),
            }),
            live: Box::new(Live::<K> {
                ns: Vec::new(),
                rows: Vec::new(),
                due: Vec::new(),
                marks: Vec::new(),
            }),
        }
    }
}

pub(crate) struct Encoded {
    pub sent: Vec<u8>,
    pub saved: Vec<u8>,
}

pub(crate) trait Rows<G: Game>: Send + Sync {
    fn step(&mut self, kind: u16, w: &World<'_, G>, clock: &Clock) -> Vec<Out<G>>;

    fn apply(
        &mut self,
        kind: u16,
        phase: u8,
        targets: &[u32],
        letters: &[Letter<G::Msg>],
        w: &World<'_, G>,
        clock: &Clock,
    ) -> Vec<Out<G>>;

    fn push(&mut self, prev: &mut dyn Any, n: u32, row: Box<dyn Any>, due: Tick) -> Encoded;

    fn commit(&mut self, prev: &mut dyn Any, kind: u16, record: &mut Record, clock: &Clock);

    fn hash(&self, kind: u16) -> u64;

    fn saved(&self, kind: u16, into: &mut BTreeMap<Id, Vec<u8>>);
}

#[derive(Default)]
struct Changes {
    shown: Vec<(Id, Vec<u8>)>,
    saved: Vec<(Id, Option<Vec<u8>>)>,
    despawned: Vec<Id>,
}

impl<G: Game, K: Kind<G>> Rows<G> for Live<K> {
    fn step(&mut self, kind: u16, w: &World<'_, G>, clock: &Clock) -> Vec<Out<G>> {
        let now = w.tick();
        let ns = &self.ns[..];
        self.rows
            .par_chunks_mut(ROWS_PER_TASK)
            .zip(self.due.par_chunks_mut(ROWS_PER_TASK))
            .zip(self.marks.par_chunks_mut(ROWS_PER_TASK))
            .enumerate()
            .map(|(c, ((rows, due), marks))| {
                let mut out = Out::new(STEP);
                if due.iter().all(|&t| t > now) {
                    return out;
                }
                let ns = &ns[c * ROWS_PER_TASK..];
                clock.time(|| {
                    for j in 0..rows.len() {
                        if due[j] > now {
                            continue;
                        }
                        let id = Id { kind, n: ns[j] };
                        due[j] = NEVER;
                        marks[j] |= TOUCHED;
                        out.begin(id);
                        K::step(id, &mut rows[j], w, &mut out);
                        if out.end(&mut due[j]) == Fate::Leaves {
                            marks[j] |= GONE;
                        }
                    }
                });
                out
            })
            .collect()
    }

    fn apply(
        &mut self,
        kind: u16,
        phase: u8,
        targets: &[u32],
        letters: &[Letter<G::Msg>],
        w: &World<'_, G>,
        clock: &Clock,
    ) -> Vec<Out<G>> {
        debug_assert!(targets.is_sorted());
        let ns = &self.ns[..];
        self.rows
            .par_chunks_mut(ROWS_PER_TASK)
            .zip(self.due.par_chunks_mut(ROWS_PER_TASK))
            .zip(self.marks.par_chunks_mut(ROWS_PER_TASK))
            .enumerate()
            .map(|(c, ((rows, due), marks))| {
                let mut out = Out::new(phase);
                let ns = &ns[c * ROWS_PER_TASK..c * ROWS_PER_TASK + rows.len()];
                let (Some(&lo), Some(&hi)) = (ns.first(), ns.last()) else {
                    return out;
                };
                let (a, b) = (
                    targets.partition_point(|&n| n < lo),
                    targets.partition_point(|&n| n <= hi),
                );
                if a == b {
                    return out;
                }
                clock.time(|| {
                    let mut i = a;
                    while i < b {
                        let n = targets[i];
                        let end = i + targets[i..b].partition_point(|&t| t == n);
                        if let Ok(j) = ns.binary_search(&n)
                            && marks[j] & GONE == 0
                        {
                            let id = Id { kind, n };
                            marks[j] |= TOUCHED;
                            out.begin(id);
                            K::apply(id, &mut rows[j], &letters[i..end], w, &mut out);
                            if out.end(&mut due[j]) == Fate::Leaves {
                                marks[j] |= GONE;
                            }
                        }
                        i = end;
                    }
                });
                out
            })
            .collect()
    }

    fn push(&mut self, prev: &mut dyn Any, n: u32, row: Box<dyn Any>, due: Tick) -> Encoded {
        let prev = own::<K>(prev);
        let Ok(row) = row.downcast::<K>() else {
            unreachable!("a row pushed to another kind's table")
        };
        debug_assert!(self.ns.last().is_none_or(|&last| last < n));
        prev.ns.push(n);
        prev.rows.push((*row).clone());
        self.ns.push(n);
        self.rows.push(*row);
        self.due.push(due);
        self.marks.push(0);
        let row = &self.rows[self.rows.len() - 1];
        Encoded {
            sent: row.sent().to_bytes(),
            saved: row.saved().to_bytes(),
        }
    }

    fn commit(&mut self, prev: &mut dyn Any, kind: u16, record: &mut Record, clock: &Clock) {
        let prev = own::<K>(prev);
        let ns = &self.ns[..];
        let parts: Vec<Changes> = prev
            .rows
            .par_chunks_mut(ROWS_PER_TASK)
            .zip(self.rows.par_chunks(ROWS_PER_TASK))
            .zip(self.marks.par_chunks_mut(ROWS_PER_TASK))
            .enumerate()
            .map(|(c, ((old, new), marks))| {
                let mut part = Changes::default();
                if marks.iter().all(|&m| m == 0) {
                    return part;
                }
                let ns = &ns[c * ROWS_PER_TASK..];
                clock.time(|| {
                    for j in 0..new.len() {
                        if marks[j] == 0 {
                            continue;
                        }
                        let id = Id { kind, n: ns[j] };
                        if marks[j] & GONE != 0 {
                            part.despawned.push(id);
                            part.saved.push((id, None));
                            continue;
                        }
                        marks[j] = 0;
                        let sent = new[j].sent();
                        if sent != old[j].sent() {
                            part.shown.push((id, sent.to_bytes()));
                        }
                        let saved = new[j].saved();
                        if saved != old[j].saved() {
                            part.saved.push((id, Some(saved.to_bytes())));
                        }
                        old[j].clone_from(&new[j]);
                    }
                });
                part
            })
            .collect();
        let mut gone = false;
        for part in parts {
            gone |= !part.despawned.is_empty();
            record.despawned.extend(part.despawned);
            record.shown.extend(part.shown);
            record.saved.extend(part.saved);
        }
        if gone {
            let keep: Vec<bool> = self.marks.iter().map(|&m| m & GONE == 0).collect();
            keep_only(&mut prev.ns, &keep);
            keep_only(&mut prev.rows, &keep);
            keep_only(&mut self.ns, &keep);
            keep_only(&mut self.rows, &keep);
            keep_only(&mut self.due, &keep);
            keep_only(&mut self.marks, &keep);
        }
    }

    fn hash(&self, kind: u16) -> u64 {
        let ns = &self.ns[..];
        self.rows
            .par_chunks(ROWS_PER_TASK)
            .zip(self.due.par_chunks(ROWS_PER_TASK))
            .enumerate()
            .map(|(c, (rows, due))| {
                let rows = rows.iter().zip(due).zip(&ns[c * ROWS_PER_TASK..]);
                rows.fold(0u64, |h, ((row, due), n)| {
                    h.wrapping_add(canon::hash(&(kind, n, due, row)))
                })
            })
            .reduce(|| 0, u64::wrapping_add)
    }

    fn saved(&self, kind: u16, into: &mut BTreeMap<Id, Vec<u8>>) {
        for (&n, row) in self.ns.iter().zip(&self.rows) {
            into.insert(Id { kind, n }, row.saved().to_bytes());
        }
    }
}

fn keep_only<T>(v: &mut Vec<T>, keep: &[bool]) {
    let mut at = keep.iter();
    v.retain(|_| at.next().copied().unwrap_or(true));
}

fn own<K: 'static>(prev: &mut dyn Any) -> &mut Table<K> {
    let Some(prev) = prev.downcast_mut::<Table<K>>() else {
        unreachable!("a kind's last-tick table is its own")
    };
    prev
}
