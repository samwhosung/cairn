use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::archive::{Archive, BlockEntry};
use crate::crypto::canonical;
use crate::error::ChainError;

/// The base archives, lowest priority first. `base.MPQ` and `backup.MPQ` are not in the client's
/// chain.
const BASE_ARCHIVES: [&str; 10] = [
    "dbc.MPQ",
    "speech.MPQ",
    "fonts.MPQ",
    "interface.MPQ",
    "misc.MPQ",
    "sound.MPQ",
    "wmo.MPQ",
    "terrain.MPQ",
    "texture.MPQ",
    "model.MPQ",
];

/// A file the chain lists: its path as a listfile spells it, and its decompressed size.
pub struct ChainEntry {
    pub name: String,
    pub size: u64,
}

/// The archives the client mounts, lowest priority first. A file in a later archive replaces the
/// same path in earlier ones, and a delete marker hides it. Reads open their own file handles, so
/// threads can read through one shared chain in parallel.
pub struct Chain {
    archives: Vec<Archive>,
}

impl Chain {
    /// Opens every archive the client mounts from a `Data` directory, or one archive file. An
    /// archive that fails to open fails the chain, where the client would skip it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ChainError> {
        let path = path.as_ref();
        if !path.is_dir() {
            let archive = Archive::open(path).map_err(|source| ChainError::Open {
                path: path.to_path_buf(),
                source,
            })?;
            return Ok(Self {
                archives: vec![archive],
            });
        }
        let listing = std::fs::read_dir(path).map_err(|source| ChainError::List {
            dir: path.to_path_buf(),
            source,
        })?;
        let names: BTreeSet<String> = listing
            .filter_map(Result::ok)
            // `Path::is_file` follows symlinks; `DirEntry::file_type` does not.
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        let archives = mount_order(&names)
            .into_iter()
            .map(|name| {
                let path = path.join(name);
                Archive::open(&path).map_err(|source| ChainError::Open { path, source })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if archives.is_empty() {
            return Err(ChainError::NoArchives(path.to_path_buf()));
        }
        Ok(Self { archives })
    }

    /// Whether some archive has an entry for `name` and the winning one is not a delete marker.
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|(_, entry)| !entry.is_delete_marker())
    }

    /// The archive whose entry for `name` wins, even when that entry is a delete marker.
    pub fn archive_of(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|(archive, _)| archive.path())
    }

    /// Reads `name` from the archive whose entry for it wins.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, ChainError> {
        let (archive, entry) = self
            .resolve(name)
            .ok_or_else(|| ChainError::NotFound(name.to_owned()))?;
        if entry.is_delete_marker() {
            return Err(ChainError::Deleted {
                name: name.to_owned(),
                archive: archive.path().to_path_buf(),
            });
        }
        archive.read(name).map_err(|source| ChainError::Read {
            name: name.to_owned(),
            archive: archive.path().to_path_buf(),
            source,
        })
    }

    /// Every file an archive's `(listfile)` names and the chain contains, with the size its
    /// winning archive gives. Files that no listfile names can be read but are not listed.
    pub fn list(&self) -> Vec<ChainEntry> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for archive in &self.archives {
            let Ok(listfile) = archive.read("(listfile)") else {
                continue;
            };
            for name in String::from_utf8_lossy(&listfile)
                .split([';', '\r', '\n'])
                .map(str::trim)
            {
                let key: Vec<u8> = name.bytes().map(canonical).collect();
                if name.is_empty() || !seen.insert(key) {
                    continue;
                }
                if let Some((_, entry)) = self.resolve(name)
                    && !entry.is_delete_marker()
                {
                    out.push(ChainEntry {
                        name: name.to_owned(),
                        size: u64::from(entry.size),
                    });
                }
            }
        }
        out
    }

    fn resolve(&self, name: &str) -> Option<(&Archive, BlockEntry)> {
        self.archives
            .iter()
            .rev()
            .find_map(|archive| Some((archive, archive.find(name)?)))
    }
}

/// The archives the client mounts out of `names`, lowest priority first.
fn mount_order(names: &BTreeSet<String>) -> Vec<String> {
    let find = |want: &str| {
        names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(want))
            .cloned()
    };
    let mut order: Vec<String> = BASE_ARCHIVES.into_iter().filter_map(&find).collect();
    order.extend(find("patch.MPQ"));
    let mut patches: Vec<String> = names
        .iter()
        .filter(|name| matches_patch_glob(name))
        .cloned()
        .collect();
    patches.sort_by_key(|name| name.to_ascii_lowercase());
    order.extend(patches);
    order.extend(find("speech2.MPQ"));
    order
}

fn matches_patch_glob(name: &str) -> bool {
    name.to_ascii_lowercase()
        .strip_prefix("patch-")
        .and_then(|rest| rest.strip_suffix(".mpq"))
        .is_some_and(|mid| mid.chars().count() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{FLAG_DELETE_MARKER, FLAG_EXISTS};
    use crate::fixture::{Entry, TempDir, archive};

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn patch_glob_matches_exactly_one_character_case_insensitively() {
        assert!(matches_patch_glob("patch-2.MPQ"));
        assert!(matches_patch_glob("patch-3.MPQ"));
        assert!(matches_patch_glob("PATCH-A.mpq"));
        assert!(!matches_patch_glob("patch-.MPQ"));
        assert!(!matches_patch_glob("patch-10.MPQ"));
        assert!(!matches_patch_glob("patch-33.MPQ"));
        assert!(!matches_patch_glob("patch.MPQ"));
        assert!(!matches_patch_glob("patch-2.MPQ.bak"));
        assert!(!matches_patch_glob("mypatch-2.MPQ"));
    }

    #[test]
    fn mount_order_follows_the_client() {
        let dir = names(&[
            "patch-2.MPQ",
            "backup.MPQ",
            "model.MPQ",
            "base.MPQ",
            "dbc.MPQ",
            "patch.MPQ",
            "eula.html",
            "patch-3.MPQ",
            "speech2.MPQ",
            "texture.MPQ",
        ]);
        assert_eq!(
            mount_order(&dir),
            [
                "dbc.MPQ",
                "texture.MPQ",
                "model.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
                "speech2.MPQ",
            ]
        );
    }

    #[test]
    fn patches_sort_ascending_by_case_folded_name() {
        let dir = names(&["patch-B.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-2.MPQ"]);
        assert_eq!(
            mount_order(&dir),
            ["patch-2.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-B.MPQ"]
        );
    }

    #[test]
    fn base_archives_are_found_case_insensitively() {
        let dir = names(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"]);
        assert_eq!(mount_order(&dir), ["DBC.mpq", "Model.MPQ", "PATCH.mpq"]);
    }

    #[test]
    fn later_archives_replace_and_delete_earlier_files() {
        let dir = TempDir::new();
        dir.write(
            "dbc.MPQ",
            &archive(&[
                Entry::new(
                    "(listfile)",
                    FLAG_EXISTS,
                    b"kept.txt\r\nreplaced.txt\r\ndeleted.txt",
                ),
                Entry::new("kept.txt", FLAG_EXISTS, b"base"),
                Entry::new("replaced.txt", FLAG_EXISTS, b"base"),
                Entry::new("deleted.txt", FLAG_EXISTS, b"base"),
            ]),
        );
        dir.write(
            "patch.MPQ",
            &archive(&[
                Entry::new("(listfile)", FLAG_EXISTS, b"replaced.txt;deleted.txt"),
                Entry::new("REPLACED.TXT", FLAG_EXISTS, b"patched"),
                Entry::new("deleted.txt", FLAG_EXISTS | FLAG_DELETE_MARKER, &[]),
            ]),
        );
        let chain = Chain::open(dir.path()).expect("open the chain");
        assert_eq!(chain.read("kept.txt").expect("read"), b"base");
        assert_eq!(chain.read("replaced.txt").expect("read"), b"patched");
        assert!(!chain.contains("deleted.txt"));
        assert!(matches!(
            chain.read("deleted.txt"),
            Err(ChainError::Deleted { .. })
        ));
        let listed: Vec<(String, u64)> = chain
            .list()
            .into_iter()
            .map(|entry| (entry.name, entry.size))
            .collect();
        assert_eq!(
            listed,
            [("kept.txt".to_owned(), 4), ("replaced.txt".to_owned(), 7)]
        );
    }
}
