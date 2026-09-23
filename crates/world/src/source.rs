use std::future::ready;
use std::io;
use std::path::Path;
use std::sync::Arc;

use bevy::asset::AssetApp;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetReaderFuture, AssetSourceBuilder, ErasedAssetReader,
    PathStream, Reader, VecReader,
};
use bevy::prelude::App;
use bevy::tasks::ConditionalSendFuture;
use mpq::{Chain, ChainError};

/// The asset source that reads the install's archives: `mpq://world/maps/azeroth/azeroth.wdt`.
pub const MPQ_SOURCE: &str = "mpq";

const SAMPLER_MARKER: char = '@';

/// Registers the `mpq://` source over the patch chain in `data`, the install's `Data` directory.
/// Call it before adding `AssetPlugin` (part of `DefaultPlugins`), which builds the sources.
pub fn register_source(app: &mut App, data: &Path) -> Result<(), ChainError> {
    let reader = MpqReader {
        chain: Arc::new(Chain::open(data)?),
    };
    app.register_asset_source(
        MPQ_SOURCE,
        AssetSourceBuilder::new(move || -> Box<dyn ErasedAssetReader> { Box::new(reader.clone()) }),
    );
    Ok(())
}

#[derive(Clone)]
struct MpqReader {
    chain: Arc<Chain>,
}

impl MpqReader {
    fn read_file(&self, path: &Path) -> Result<VecReader, AssetReaderError> {
        let name = path.to_str().ok_or_else(|| not_found(path))?;
        let name = strip_sampler_marker(name).unwrap_or_else(|| name.to_owned());
        match self.chain.read(&name) {
            Ok(bytes) => Ok(VecReader::new(bytes)),
            Err(ChainError::NotFound(_) | ChainError::Deleted { .. }) => Err(not_found(path)),
            Err(e) => Err(AssetReaderError::Io(Arc::new(io::Error::other(e)))),
        }
    }
}

impl AssetReader for MpqReader {
    fn read<'a>(&'a self, path: &'a Path) -> impl AssetReaderFuture<Value: Reader + 'a> {
        ready(self.read_file(path))
    }

    // The archives hold no `.meta` files; not found tells the asset server to use the loader's
    // default settings.
    fn read_meta<'a>(&'a self, path: &'a Path) -> impl AssetReaderFuture<Value: Reader + 'a> {
        ready(Err::<VecReader, _>(not_found(path)))
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<Box<PathStream>, AssetReaderError>> {
        ready(Err(not_found(path)))
    }

    fn is_directory<'a>(
        &'a self,
        _path: &'a Path,
    ) -> impl ConditionalSendFuture<Output = Result<bool, AssetReaderError>> {
        ready(Ok(false))
    }
}

fn not_found(path: &Path) -> AssetReaderError {
    AssetReaderError::NotFound(path.to_path_buf())
}

/// Whether a texture repeats along u and along v; an axis that does not repeat clamps to its edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub u: bool,
    pub v: bool,
}

impl Repeat {
    pub const BOTH: Self = Self { u: true, v: true };
}

/// The `mpq://` URL of a texture sampled with `repeat`.
///
/// A sampler rides on its image and images are keyed by path, so a texture that models sample
/// two ways needs two paths. Repeating on both axes keeps the bare path; any other mode marks the
/// stem (`leaves01@cc.blp`), keeping the extension that picks the loader. Paths are lowercased so
/// one file named in two cases loads once.
pub fn texture_url(internal: &str, repeat: Repeat) -> String {
    let path = internal.replace('\\', "/").to_ascii_lowercase();
    let tag = match (repeat.u, repeat.v) {
        (true, true) => return format!("{MPQ_SOURCE}://{path}"),
        (true, false) => "rc",
        (false, true) => "cr",
        (false, false) => "cc",
    };
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{MPQ_SOURCE}://{stem}{SAMPLER_MARKER}{tag}.{ext}"),
        None => format!("{MPQ_SOURCE}://{path}{SAMPLER_MARKER}{tag}"),
    }
}

pub(crate) fn repeat_of(path: &str) -> Repeat {
    let Some((stem, _)) = path.rsplit_once('.') else {
        return Repeat::BOTH;
    };
    let (u, v) = match stem.rsplit_once(SAMPLER_MARKER) {
        Some((_, "rc")) => (true, false),
        Some((_, "cr")) => (false, true),
        Some((_, "cc")) => (false, false),
        _ => (true, true),
    };
    Repeat { u, v }
}

/// The `mpq://` URL of a model named by its `.mdx`, `.mdl` or `.m2` path: the archives hold
/// only the `.m2`.
pub fn m2_url(raw: &str) -> String {
    let path = raw.replace('\\', "/").to_ascii_lowercase();
    let stem = path
        .strip_suffix(".mdx")
        .or_else(|| path.strip_suffix(".mdl"))
        .or_else(|| path.strip_suffix(".m2"))
        .unwrap_or(&path);
    format!("{MPQ_SOURCE}://{stem}.m2")
}

/// The `mpq://` URL of a WMO root file.
pub fn wmo_url(raw: &str) -> String {
    format!(
        "{MPQ_SOURCE}://{}",
        raw.replace('\\', "/").to_ascii_lowercase()
    )
}

fn strip_sampler_marker(path: &str) -> Option<String> {
    let (stem, ext) = path.rsplit_once('.')?;
    let (base, tag) = stem.rsplit_once(SAMPLER_MARKER)?;
    matches!(tag, "rc" | "cr" | "cc").then(|| format!("{base}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAVES: &str =
        "World\\KhazModan\\Ironforge\\PassiveDoodads\\Trees\\IronForgeleaves01.blp";
    const LEAVES_PATH: &str = "world/khazmodan/ironforge/passivedoodads/trees/ironforgeleaves01";

    #[test]
    fn repeating_textures_keep_the_bare_path() {
        let url = texture_url(LEAVES, Repeat::BOTH);
        assert_eq!(url, format!("mpq://{LEAVES_PATH}.blp"));
        assert_eq!(repeat_of(&url), Repeat::BOTH);
        assert_eq!(strip_sampler_marker(&url), None);
    }

    #[test]
    fn other_modes_mark_the_stem_and_round_trip() {
        for (u, v, tag) in [
            (true, false, "rc"),
            (false, true, "cr"),
            (false, false, "cc"),
        ] {
            let url = texture_url(LEAVES, Repeat { u, v });
            assert_eq!(url, format!("mpq://{LEAVES_PATH}@{tag}.blp"));
            assert_eq!(repeat_of(&url), Repeat { u, v });
            let path = url.strip_prefix("mpq://").expect("an mpq url");
            assert_eq!(
                strip_sampler_marker(path),
                Some(format!("{LEAVES_PATH}.blp"))
            );
        }
    }

    #[test]
    fn an_at_sign_that_is_not_a_mode_is_part_of_the_name() {
        assert_eq!(strip_sampler_marker("world/foo@bar.blp"), None);
        assert_eq!(repeat_of("world/foo@bar.blp"), Repeat::BOTH);
    }

    #[test]
    fn model_urls_name_the_m2_the_archives_hold() {
        for raw in [
            "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.mdx",
            "WORLD\\AZEROTH\\ELWYNN\\PASSIVEDOODADS\\CAMPFIRE\\ELWYNNCAMPFIRE.MDL",
            "world/azeroth/elwynn/passivedoodads/campfire/elwynncampfire.m2",
        ] {
            assert_eq!(
                m2_url(raw),
                "mpq://world/azeroth/elwynn/passivedoodads/campfire/elwynncampfire.m2"
            );
        }
        assert_eq!(
            wmo_url("World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo"),
            "mpq://world/wmo/azeroth/buildings/goldshireinn/goldshireinn.wmo"
        );
    }
}
