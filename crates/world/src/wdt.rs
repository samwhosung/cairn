use std::io::{self, Cursor};

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use wdt::{WdtFile, WdtReader};

/// A map's WDT as an asset: which of its 64×64 ADT tiles exist.
#[derive(Asset, TypePath)]
pub struct WdtIndex(WdtFile);

impl WdtIndex {
    /// Whether tile `(tile_x, tile_y)`, as in `Map_<x>_<y>.adt`, has terrain.
    pub fn has_tile(&self, tile_x: u32, tile_y: u32) -> bool {
        self.0
            .get_tile(tile_x as usize, tile_y as usize)
            .is_some_and(|t| t.has_adt)
    }
}

#[derive(Default, TypePath)]
pub(crate) struct WdtLoader;

impl AssetLoader for WdtLoader {
    type Asset = WdtIndex;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        _ctx: &mut LoadContext<'_>,
    ) -> Result<WdtIndex, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let wdt = WdtReader::new(Cursor::new(bytes))
            .read()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(WdtIndex(wdt))
    }

    fn extensions(&self) -> &[&str] {
        &["wdt"]
    }
}
