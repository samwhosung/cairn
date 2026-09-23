use wowfile::{ByteExt, chunks};

use crate::error::Error;
use crate::mcnk::{McnkChunk, read_mcnk};

/// MDDF: a placed M2 doodad.
#[derive(Debug, Clone)]
pub struct DoodadPlacement {
    /// Index into [`RootAdt::models`].
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    /// 1024 is 1.0.
    pub scale: u16,
    pub flags: u16,
}

/// MODF: a placed WMO.
#[derive(Debug, Clone)]
pub struct WmoPlacement {
    /// Index into [`RootAdt::wmos`].
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub flags: u16,
    pub doodad_set: u16,
    /// The `WMOAreaTable` name set: one WMO can be several named places.
    pub name_set: u16,
}

#[derive(Debug, Clone)]
pub struct RootAdt {
    /// MTEX, indexed by [`crate::MclyLayer::texture_id`].
    pub textures: Vec<String>,
    /// MMDX paths in MMID order.
    pub models: Vec<String>,
    /// MWMO paths in MWID order.
    pub wmos: Vec<String>,
    pub doodad_placements: Vec<DoodadPlacement>,
    pub wmo_placements: Vec<WmoPlacement>,
    /// The MCNK terrain chunks in file order, normally 256.
    pub mcnk_chunks: Vec<McnkChunk>,
}

/// Parses an ADT from its first chunk. Unknown chunks are skipped, and a partial record at the
/// end of a placement or offset list is dropped; only a chunk too short for its MCNK header
/// fails the tile.
pub fn parse_adt(bytes: &[u8]) -> Result<RootAdt, Error> {
    let mut textures = Vec::new();
    let mut mmdx: &[u8] = &[];
    let mut mmid = Vec::new();
    let mut mwmo: &[u8] = &[];
    let mut mwid = Vec::new();
    let mut doodad_placements = Vec::new();
    let mut wmo_placements = Vec::new();
    let mut mcnk_chunks = Vec::new();
    for (magic, data) in chunks(bytes) {
        match &magic {
            b"XETM" => textures = non_empty_strings(data),
            b"XDMM" => mmdx = data,
            b"DIMM" => mmid = u32_list(data),
            b"OMWM" => mwmo = data,
            b"DIWM" => mwid = u32_list(data),
            b"FDDM" => doodad_placements = read_mddf(data),
            b"FDOM" => wmo_placements = read_modf(data),
            b"KNCM" => mcnk_chunks.push(read_mcnk(data)?),
            _ => {}
        }
    }
    Ok(RootAdt {
        textures,
        models: mmid.iter().map(|&o| cstring_at(mmdx, o as usize)).collect(),
        wmos: mwid.iter().map(|&o| cstring_at(mwmo, o as usize)).collect(),
        doodad_placements,
        wmo_placements,
        mcnk_chunks,
    })
}

fn cstring_at(blob: &[u8], offset: usize) -> String {
    let tail = &blob[offset.min(blob.len())..];
    let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

fn non_empty_strings(blob: &[u8]) -> Vec<String> {
    blob.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn u32_list(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

fn vec3(r: &[u8], offset: usize) -> [f32; 3] {
    [0, 4, 8].map(|i| r.f32_at(offset + i).unwrap_or_default())
}

fn read_mddf(data: &[u8]) -> Vec<DoodadPlacement> {
    data.as_chunks::<36>()
        .0
        .iter()
        .map(|r| DoodadPlacement {
            name_id: r.u32_at(0).unwrap_or_default(),
            unique_id: r.u32_at(4).unwrap_or_default(),
            position: vec3(r, 8),
            rotation: vec3(r, 20),
            scale: r.u16_at(32).unwrap_or_default(),
            flags: r.u16_at(34).unwrap_or_default(),
        })
        .collect()
}

fn read_modf(data: &[u8]) -> Vec<WmoPlacement> {
    data.as_chunks::<64>()
        .0
        .iter()
        .map(|r| WmoPlacement {
            name_id: r.u32_at(0).unwrap_or_default(),
            unique_id: r.u32_at(4).unwrap_or_default(),
            position: vec3(r, 8),
            rotation: vec3(r, 20),
            flags: r.u16_at(56).unwrap_or_default(),
            doodad_set: r.u16_at(58).unwrap_or_default(),
            name_set: r.u16_at(60).unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn chunk(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = magic.to_vec();
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_tile() {
        let mut mddf = vec![0u8; 36];
        mddf[32..34].copy_from_slice(&7u16.to_le_bytes());
        let mut modf = vec![0u8; 64];
        modf[58..60].copy_from_slice(&3u16.to_le_bytes());
        let mut mcnk = vec![0u8; 128];
        mcnk[0x14..0x18].copy_from_slice(&128u32.to_le_bytes());
        let heights: Vec<u8> = (0..145u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
        mcnk.extend(chunk(*b"TVCM", &heights));

        let mut b = chunk(*b"XETM", b"tex1.blp\0tex2.blp\0");
        b.extend(chunk(*b"XDMM", b"model.m2\0"));
        b.extend(chunk(*b"DIMM", &0u32.to_le_bytes()));
        b.extend(chunk(*b"OMWM", b"world.wmo\0"));
        b.extend(chunk(*b"DIWM", &0u32.to_le_bytes()));
        b.extend(chunk(*b"FDDM", &mddf));
        b.extend(chunk(*b"FDOM", &modf));
        b.extend(chunk(*b"KNCM", &mcnk));

        let root = parse_adt(&b).expect("parses");
        assert_eq!(root.textures, ["tex1.blp", "tex2.blp"]);
        assert_eq!(root.models, ["model.m2"]);
        assert_eq!(root.wmos, ["world.wmo"]);
        assert_eq!(root.doodad_placements.len(), 1);
        assert_eq!(root.doodad_placements[0].scale, 7);
        assert_eq!(root.wmo_placements.len(), 1);
        assert_eq!(root.wmo_placements[0].doodad_set, 3);
        assert_eq!(root.mcnk_chunks.len(), 1);
        let heights = &root.mcnk_chunks[0]
            .heights
            .as_ref()
            .expect("MCVT parsed")
            .heights;
        assert_eq!(heights.len(), 145);
        assert_eq!(heights[1], 1.0);
    }

    #[test]
    fn a_partial_trailing_placement_is_dropped() {
        let root = parse_adt(&chunk(*b"FDDM", &[0u8; 10])).expect("parses");
        assert!(root.doodad_placements.is_empty());
    }

    #[test]
    fn a_name_offset_past_the_blob_is_an_empty_name() {
        let mut b = chunk(*b"XDMM", b"a.m2\0");
        b.extend(chunk(*b"DIMM", &99u32.to_le_bytes()));
        assert_eq!(parse_adt(&b).expect("parses").models, [""]);
    }

    #[test]
    fn a_short_mcnk_header_is_an_error() {
        assert!(matches!(
            parse_adt(&chunk(*b"KNCM", &[0u8; 40])),
            Err(Error::Truncated("MCNK header"))
        ));
    }
}
