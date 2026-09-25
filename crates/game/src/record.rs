use std::collections::BTreeMap;

use crate::{Id, Tick};

/// Everything a tick changed in what is sent and what is saved. Entries come by row, and a row's in
/// the order they happened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub tick: Tick,
    /// Rows that joined or were spawned.
    pub came: Vec<Id>,
    pub despawned: Vec<Id>,
    /// Each row's sent fields as they now stand, encoded, where they changed or the row came.
    pub shown: Vec<(Id, Vec<u8>)>,
    /// Each row's saved fields, as for `shown`, and `None` for a row that despawned.
    pub saved: Vec<(Id, Option<Vec<u8>>)>,
}

impl Record {
    pub(crate) fn open(&mut self, tick: Tick) {
        self.tick = tick;
        self.came.clear();
        self.despawned.clear();
        self.shown.clear();
        self.saved.clear();
    }

    pub(crate) fn close(&mut self) {
        self.came.sort_unstable();
        self.despawned.sort_unstable();
        self.shown.sort_by_key(|e| e.0);
        self.saved.sort_by_key(|e| e.0);
    }
}

/// The saved state of every row, kept in memory from each tick's record.
#[derive(Clone, Debug, Default)]
pub struct Saves {
    rows: BTreeMap<Id, Vec<u8>>,
}

impl Saves {
    pub fn take(&mut self, record: &Record) {
        for (id, saved) in &record.saved {
            match saved {
                Some(bytes) => {
                    self.rows.insert(*id, bytes.clone());
                }
                None => {
                    self.rows.remove(id);
                }
            }
        }
    }

    pub fn rows(&self) -> &BTreeMap<Id, Vec<u8>> {
        &self.rows
    }

    /// The first row where these differ from `scan`, every row's saved fields as the world holds
    /// them.
    pub fn first_difference(&self, scan: &BTreeMap<Id, Vec<u8>>) -> Option<String> {
        let mut ours = self.rows.iter();
        let mut theirs = scan.iter();
        loop {
            match (ours.next(), theirs.next()) {
                (None, None) => return None,
                (Some((id, a)), Some((jd, b))) if id == jd && a == b => {}
                (Some((id, a)), Some((jd, b))) if id == jd => {
                    return Some(format!("{id:?}: saved {a:?}, and the world holds {b:?}"));
                }
                (Some((id, _)), Some((jd, _))) => {
                    let (missing, from) = if id < jd {
                        (id, "the world")
                    } else {
                        (jd, "the saves")
                    };
                    return Some(format!("{missing:?} is missing from {from}"));
                }
                (Some((id, _)), None) => return Some(format!("{id:?} is missing from the world")),
                (None, Some((jd, _))) => return Some(format!("{jd:?} is missing from the saves")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_follow_the_record_and_name_the_first_difference_from_a_scan() {
        let (a, b) = (Id::player(0), Id { kind: 1, n: 4 });
        let mut saves = Saves::default();
        saves.take(&Record {
            saved: vec![(a, Some(vec![1])), (b, Some(vec![2]))],
            ..Record::default()
        });
        saves.take(&Record {
            saved: vec![(a, Some(vec![3])), (b, None)],
            ..Record::default()
        });
        let scan = BTreeMap::from([(a, vec![3])]);
        assert_eq!(saves.first_difference(&scan), None);
        let changed = BTreeMap::from([(a, vec![4])]);
        assert!(
            saves
                .first_difference(&changed)
                .is_some_and(|d| d.contains("[3]"))
        );
        let more = BTreeMap::from([(a, vec![3]), (b, vec![2])]);
        assert!(
            saves
                .first_difference(&more)
                .is_some_and(|d| d.contains("the saves"))
        );
    }
}
