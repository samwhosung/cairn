use std::fs;
use std::path::{Path, PathBuf};

use crate::error::ChainError;

/// A directory laid over the archives, its files named by the paths the archives name them by.
pub(crate) struct Patch {
    root: PathBuf,
}

impl Patch {
    pub(crate) fn open(root: &Path) -> Result<Self, ChainError> {
        fs::read_dir(root).map_err(|source| ChainError::Patch {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The file that answers to `name` in the directory as it is now, matched part by part as the
    /// archives match names: ASCII case ignored and `/` equal to `\`. Where only case tells two
    /// entries apart, both are searched. A part must name an entry, so `..` leads nowhere.
    pub(crate) fn find(&self, name: &str) -> Result<Option<PathBuf>, ChainError> {
        let mut found = vec![self.root.clone()];
        let mut parts = name.split(['/', '\\']).peekable();
        while let Some(part) = parts.next() {
            let last = parts.peek().is_none();
            let mut next = Vec::new();
            for dir in &found {
                let unlisted = |source| ChainError::Patch {
                    path: dir.clone(),
                    source,
                };
                for entry in fs::read_dir(dir).map_err(unlisted)? {
                    let path = entry.map_err(unlisted)?.path();
                    let named = path.file_name().is_some_and(|entry_name| {
                        entry_name
                            .as_encoded_bytes()
                            .eq_ignore_ascii_case(part.as_bytes())
                    });
                    if named && if last { path.is_file() } else { path.is_dir() } {
                        next.push(path);
                    }
                }
            }
            found = next;
        }
        found.sort();
        if let [first, second, ..] = &found[..] {
            return Err(ChainError::Ambiguous {
                name: name.to_owned(),
                first: first.clone(),
                second: second.clone(),
            });
        }
        Ok(found.pop())
    }
}
