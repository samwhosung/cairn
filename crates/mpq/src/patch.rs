use std::fs;
use std::path::{Path, PathBuf};

use crate::crypto::canonical;
use crate::error::ChainError;

pub(crate) struct PatchDir {
    root: PathBuf,
}

impl PatchDir {
    pub(crate) fn open(root: &Path) -> Result<Self, ChainError> {
        fs::read_dir(root).map_err(|source| ChainError::PatchDir {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// A part of `name` must name an entry the directory lists, so `..` leads nowhere.
    pub(crate) fn find(&self, name: &str) -> Result<Option<PathBuf>, ChainError> {
        let key: Vec<u8> = name.bytes().map(canonical).collect();
        let mut parts = key.split(|&byte| byte == b'\\').peekable();
        let mut found = vec![self.root.clone()];
        while let Some(part) = parts.next() {
            let last = parts.peek().is_none();
            let mut next = Vec::new();
            for dir in &found {
                let unlisted = |source| ChainError::PatchDir {
                    path: dir.clone(),
                    source,
                };
                for entry in fs::read_dir(dir).map_err(unlisted)? {
                    let path = entry.map_err(unlisted)?.path();
                    let named = path.file_name().is_some_and(|entry_name| {
                        let entry_name = entry_name.as_encoded_bytes().iter();
                        entry_name
                            .map(|&byte| canonical(byte))
                            .eq(part.iter().copied())
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
