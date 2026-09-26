use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::archive::{Archive, BlockEntry};
use crate::crypto::canonical;
use crate::error::ChainError;
use crate::patch::PatchDir;

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

/// The archives the client mounts, lowest priority first, and any patch directory laid over them.
/// A file in a later archive replaces the same path in earlier ones, and a delete marker hides it.
/// Reads open their own file handles, so threads can read through one shared chain in parallel.
/// The default chain mounts nothing.
#[derive(Default)]
pub struct Chain {
    archives: Vec<Archive>,
    patch_dir: Option<PatchDir>,
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
                patch_dir: None,
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
        Ok(Self {
            archives,
            patch_dir: None,
        })
    }

    /// Lays the directory `dir` over every archive, as one more patch archive would be: a file in
    /// it at the path the archives name a file by is read in place of theirs. The directory is
    /// read at each lookup, so a file written into it later is read as it is then.
    pub fn with_patch_dir(mut self, dir: impl AsRef<Path>) -> Result<Self, ChainError> {
        self.patch_dir = Some(PatchDir::open(dir.as_ref())?);
        Ok(self)
    }

    /// Whether the patch directory has `name`, or some archive has an entry for it and the
    /// winning one is not a delete marker.
    pub fn contains(&self, name: &str) -> bool {
        matches!(self.patched(name), Ok(Some(_)))
            || self
                .resolve(name)
                .is_some_and(|(_, entry)| !entry.is_delete_marker())
    }

    /// The archive whose entry for `name` wins, even when that entry is a delete marker, whether
    /// or not the patch directory has the file.
    pub fn archive_of(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|(archive, _)| archive.path())
    }

    /// Reads `name` from the patch directory, or else from the archive whose entry for it wins.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, ChainError> {
        if let Some(path) = self.patched(name)? {
            return std::fs::read(&path).map_err(|source| ChainError::PatchDir { path, source });
        }
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

    /// Every file an archive's `(listfile)` names and the archives hold, with the size its
    /// winning archive gives. Files that no listfile names can be read but are not listed, and the
    /// patch directory is not listed.
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

    fn patched(&self, name: &str) -> Result<Option<PathBuf>, ChainError> {
        self.patch_dir
            .as_ref()
            .map_or(Ok(None), |dir| dir.find(name))
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

    fn base_and_patch() -> (TempDir, TempDir) {
        let data = TempDir::new();
        data.write(
            "dbc.MPQ",
            &archive(&[
                Entry::new("World\\kept.txt", FLAG_EXISTS, b"base"),
                Entry::new("World\\replaced.txt", FLAG_EXISTS, b"base"),
                Entry::new("World\\deleted.txt", FLAG_EXISTS, b"base"),
            ]),
        );
        data.write(
            "patch.MPQ",
            &archive(&[Entry::new(
                "World\\deleted.txt",
                FLAG_EXISTS | FLAG_DELETE_MARKER,
                &[],
            )]),
        );
        data.write("outside.txt", b"outside");
        let patch = TempDir::new();
        patch.write("WORLD/Replaced.TXT", b"patched");
        patch.write("WORLD/deleted.txt", b"patched");
        patch.write("WORLD/new.txt", b"patched");
        std::fs::create_dir_all(patch.path().join("WORLD/kept.txt")).expect("a directory");
        (data, patch)
    }

    #[test]
    fn the_patch_directory_is_read_before_every_archive() {
        let (data, patch) = base_and_patch();
        let bare = Chain::open(data.path()).expect("open the chain");
        assert_eq!(bare.read("world/replaced.txt").expect("read"), b"base");
        assert!(!bare.contains("world/new.txt"));
        let chain = bare.with_patch_dir(patch.path()).expect("lay the patch");
        for name in [
            "World\\replaced.txt",
            "world/REPLACED.txt",
            "WORLD\\Replaced.TXT",
        ] {
            assert_eq!(chain.read(name).expect("read"), b"patched", "{name}");
        }
        assert_eq!(chain.read("world\\deleted.txt").expect("read"), b"patched");
        assert_eq!(chain.read("world/new.txt").expect("read"), b"patched");
        assert!(chain.contains("world/new.txt") && chain.contains("world/deleted.txt"));
        assert_eq!(chain.read("world\\kept.txt").expect("read"), b"base");
        patch.write("WORLD/new.txt", b"rewritten");
        patch.write("WORLD/later.txt", b"later");
        assert_eq!(chain.read("world/new.txt").expect("read"), b"rewritten");
        assert_eq!(chain.read("world/later.txt").expect("read"), b"later");
    }

    #[test]
    fn no_name_leads_out_of_the_patch_directory() {
        let (data, patch) = base_and_patch();
        let chain = Chain::open(data.path())
            .and_then(|chain| chain.with_patch_dir(patch.path()))
            .expect("lay the patch");
        let absolute = data.path().join("outside.txt").display().to_string();
        let data_dir = data.path().file_name().expect("a name").display();
        let relative = format!("../{data_dir}/outside.txt");
        assert!(patch.path().join(&relative).is_file());
        for name in [
            relative.as_str(),
            &relative.replace('/', "\\"),
            &absolute,
            "world/../world/new.txt",
            "",
            "world/",
        ] {
            assert!(!chain.contains(name), "{name}");
            assert!(
                matches!(chain.read(name), Err(ChainError::NotFound(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn entries_that_only_case_tells_apart_answer_as_one() {
        let (data, patch) = base_and_patch();
        patch.write("world/Both.txt", b"lower");
        if patch.path().join("WORLD/both.txt").exists() {
            eprintln!("skipped: the temporary directory ignores case");
            return;
        }
        patch.write("world/other.txt", b"lower");
        let chain = Chain::open(data.path())
            .and_then(|chain| chain.with_patch_dir(patch.path()))
            .expect("lay the patch");
        assert_eq!(chain.read("World\\Other.txt").expect("read"), b"lower");
        assert_eq!(chain.read("world/replaced.txt").expect("read"), b"patched");
        patch.write("WORLD/both.TXT", b"upper");
        assert!(matches!(
            chain.read("world/both.txt"),
            Err(ChainError::Ambiguous { .. })
        ));
    }

    #[test]
    fn a_patch_directory_that_cannot_be_listed_is_refused() {
        let (data, patch) = base_and_patch();
        let missing = patch.path().join("missing");
        let file = patch.path().join("WORLD/new.txt");
        for dir in [missing, file] {
            let refused = Chain::open(data.path()).and_then(|chain| chain.with_patch_dir(&dir));
            assert!(
                matches!(refused, Err(ChainError::PatchDir { ref path, .. }) if *path == dir),
                "{}",
                dir.display()
            );
        }
    }
}
