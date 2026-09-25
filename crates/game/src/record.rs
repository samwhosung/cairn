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
