use mpq::Chain;
use wmo::{ParsedWmo, parse_wmo};

use crate::{Error, RenderSubmesh, wmo_group_submeshes};

/// Group flags `0x8` (exterior) and `0x40` (exterior-lit). The client lights and draws a group
/// with neither as interior.
pub(crate) const EXTERIOR_BITS: u32 = 0x48;

/// Loads a WMO root and every group file it names (`<stem>_NNN.wmo`), as one submesh per
/// non-empty group render batch. A group file the chain cannot read is skipped.
pub fn load_wmo(chain: &Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>, Error> {
    let root_path = raw_path.to_ascii_lowercase();
    let bytes = chain.read(&root_path).map_err(Error::Chain)?;
    let root = parse_wmo_root(&bytes)?;
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let mut out = Vec::new();
    for gi in 0..root.group_count() {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read(&group_path) else {
            continue;
        };
        out.extend(wmo_group_submeshes(&gbytes, &root));
    }
    Ok(out)
}

pub(crate) fn find_wmo_chunk(bytes: &[u8], magic: [u8; 4]) -> Option<&[u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let data_start = off + 8;
        let data_end = data_start.saturating_add(size).min(bytes.len());
        if bytes[off..off + 4] == magic {
            return Some(&bytes[data_start..data_end]);
        }
        off = data_end;
    }
    None
}

/// One `MODD` placement: an M2 the root places inside itself, in WMO model space (WoW axes, Z up).
#[derive(Debug, Clone)]
pub struct WmoDoodad {
    /// The M2 path as `MODN` spells it; empty when the name offset does not resolve.
    pub model: String,
    pub position: [f32; 3],
    /// Quaternion `(x, y, z, w)`.
    pub orientation: [f32; 4],
    pub scale: f32,
    /// The baked light colour (`+0x24`, BGRA on disk) as `[r, g, b, a]`: the client's base light
    /// for a doodad of an interior group.
    pub color: [u8; 4],
}

/// One `MODS` doodad set: `start..start + count` of [`WmoRoot::doodads`]. Set 0 always shows; a
/// placement adds the one set its `MODF` entry names.
#[derive(Debug, Clone, Copy)]
pub struct WmoDoodadSet {
    pub start: u32,
    pub count: u32,
}

/// One `MOGI` entry: a group's class and bounding box, in WMO model space.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupInfo {
    /// Neither exterior flag (`0x8`, `0x40`) is set: the client lights and draws it as interior.
    pub interior: bool,
    /// Flag `0x40000`: the client shows the root's [`WmoRoot::skybox`] while its visibility pass
    /// reaches this group.
    pub show_skybox: bool,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

/// A parsed WMO root: the tables its group files resolve against.
pub struct WmoRoot {
    pub(crate) parsed: wmo::WmoRoot,
    doodads: Vec<WmoDoodad>,
    doodad_sets: Vec<WmoDoodadSet>,
    group_infos: Vec<WmoGroupInfo>,
    portals: WmoPortals,
    fogs: Vec<WmoFog>,
    skybox: Option<String>,
}

impl WmoRoot {
    /// Number of group files (`<stem>_NNN.wmo`) the root names.
    pub fn group_count(&self) -> u32 {
        self.parsed.n_groups
    }

    /// The `MODD` placements, which [`Self::doodad_sets`] index.
    pub fn doodads(&self) -> &[WmoDoodad] {
        &self.doodads
    }

    /// The `MODS` doodad sets.
    pub fn doodad_sets(&self) -> &[WmoDoodadSet] {
        &self.doodad_sets
    }

    /// The `MOGI` entries, one per group.
    pub fn group_infos(&self) -> &[WmoGroupInfo] {
        &self.group_infos
    }

    /// Each `MOMT` material's `TerrainType.dbc` id (`+0x20`), indexed by a face's `MOPY` material:
    /// the sound of walking on it.
    pub fn material_ground_types(&self) -> Vec<u32> {
        self.parsed
            .materials
            .iter()
            .map(|m| m.ground_type)
            .collect()
    }

    /// Each `MOMT` material's diffuse colour (`+0x1C`), RGB in `0..=1`. An interior liquid takes its
    /// material's as its body colour ([`LiquidMesh::material_id`](crate::LiquidMesh::material_id)).
    pub fn material_diff_colors(&self) -> Vec<[f32; 3]> {
        self.parsed
            .materials
            .iter()
            .map(|m| {
                let [red, green, blue] = m.diff_color;
                [
                    f32::from(red) / 255.0,
                    f32::from(green) / 255.0,
                    f32::from(blue) / 255.0,
                ]
            })
            .collect()
    }

    /// The portal graph; empty when the root has no portals.
    pub fn portals(&self) -> &WmoPortals {
        &self.portals
    }

    /// The `MFOG` fogs, which [`WmoGroupHeader::fog_indices`](crate::WmoGroupHeader::fog_indices)
    /// index.
    pub fn fogs(&self) -> &[WmoFog] {
        &self.fogs
    }

    /// The `MOSB` skybox model, as a `.m2` path; `None` when the chunk is absent or names nothing.
    pub fn skybox(&self) -> Option<&str> {
        self.skybox.as_deref()
    }
}

/// Parses a WMO root file; `Err` when the bytes are not one.
pub fn parse_wmo_root(bytes: &[u8]) -> Result<WmoRoot, Error> {
    let ParsedWmo::Root(parsed) = parse_wmo(bytes).map_err(Error::Wmo)? else {
        return Err(Error::NotWmoRoot);
    };
    let (doodads, doodad_sets) = parse_wmo_doodads(bytes);
    let group_infos = parse_wmo_group_infos(bytes);
    let portals = parse_wmo_portals(bytes);
    let fogs = parse_wmo_fogs(bytes);
    let skybox = parse_skybox(bytes);
    Ok(WmoRoot {
        parsed,
        doodads,
        doodad_sets,
        group_infos,
        portals,
        fogs,
        skybox,
    })
}

fn parse_skybox(bytes: &[u8]) -> Option<String> {
    let mosb = find_wmo_chunk(bytes, *b"BSOM")?;
    let end = mosb.iter().position(|&b| b == 0).unwrap_or(mosb.len());
    let raw = std::str::from_utf8(&mosb[..end]).ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(crate::model_path(raw))
}

fn parse_wmo_group_infos(bytes: &[u8]) -> Vec<WmoGroupInfo> {
    let Some(mogi) = find_wmo_chunk(bytes, *b"IGOM") else {
        return Vec::new();
    };
    mogi.as_chunks::<32>()
        .0
        .iter()
        .map(|rec| {
            let f = |i: usize| f32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            let flags = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            WmoGroupInfo {
                interior: (flags & EXTERIOR_BITS) == 0,
                show_skybox: (flags & 0x40000) != 0,
                bbox_min: [f(4), f(8), f(12)],
                bbox_max: [f(16), f(20), f(24)],
            }
        })
        .collect()
}

/// A root's portal graph, in WMO model space. Each group owns the slice of [`Self::refs`] its
/// header names ([`WmoGroupHeader`](crate::WmoGroupHeader)).
#[derive(Debug, Clone, Default)]
pub struct WmoPortals {
    /// `MOPV`: every portal's polygon vertices, pooled.
    pub vertices: Vec<[f32; 3]>,
    /// `MOPT`: one entry per portal.
    pub infos: Vec<WmoPortalInfo>,
    /// `MOPR`: the refs, each joining a group to a neighbour through a portal.
    pub refs: Vec<WmoPortalRef>,
}

/// One `MOPT` portal: `count` of [`WmoPortals::vertices`] from `start_vertex`, and its plane
/// `[nx, ny, nz, d]`, where a point `p` lies at signed distance `n·p + d`.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalInfo {
    pub start_vertex: u16,
    pub count: u16,
    pub plane: [f32; 4],
}

/// One `MOPR` ref: `portal` indexes [`WmoPortals::infos`] and `group` is the group beyond it. The
/// client looks through it only from the side of the plane where `n·p + d` has the sign of `side`.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalRef {
    pub portal: u16,
    pub group: u16,
    pub side: i16,
}

/// Parses a root's portal graph (`MOPV`, `MOPT`, `MOPR`); an absent chunk gives an empty table.
pub fn parse_wmo_portals(bytes: &[u8]) -> WmoPortals {
    let f = |b: &[u8], i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u16 = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let i16 = |b: &[u8], i: usize| i16::from_le_bytes([b[i], b[i + 1]]);

    let vertices = find_wmo_chunk(bytes, *b"VPOM")
        .map(|c| {
            c.as_chunks::<12>()
                .0
                .iter()
                .map(|r| [f(r, 0), f(r, 4), f(r, 8)])
                .collect()
        })
        .unwrap_or_default();
    let infos = find_wmo_chunk(bytes, *b"TPOM")
        .map(|c| {
            c.as_chunks::<20>()
                .0
                .iter()
                .map(|r| WmoPortalInfo {
                    start_vertex: u16(r, 0),
                    count: u16(r, 2),
                    plane: [f(r, 4), f(r, 8), f(r, 12), f(r, 16)],
                })
                .collect()
        })
        .unwrap_or_default();
    let refs = find_wmo_chunk(bytes, *b"RPOM")
        .map(|c| {
            c.as_chunks::<8>()
                .0
                .iter()
                .map(|r| WmoPortalRef {
                    portal: u16(r, 0),
                    group: u16(r, 2),
                    side: i16(r, 4),
                })
                .collect()
        })
        .unwrap_or_default();
    WmoPortals {
        vertices,
        infos,
        refs,
    }
}

/// One `MFOG` fog (48 bytes), in WMO model space; distances in yards.
#[derive(Debug, Clone, Copy)]
pub struct WmoFog {
    pub flags: u32,
    /// Sphere centre.
    pub pos: [f32; 3],
    /// The fog weighs fully inside this radius.
    pub radius_inner: f32,
    /// The fog weighs nothing beyond this radius.
    pub radius_outer: f32,
    pub fog_end: f32,
    /// Fog start as a fraction of `fog_end`.
    pub fog_start_scalar: f32,
    /// The raw colour dword.
    pub color: u32,
    /// The underwater fog: end, start fraction and raw colour, as above.
    pub uw_fog_end: f32,
    pub uw_fog_start_scalar: f32,
    pub uw_color: u32,
}

/// Parses a root's `MFOG` fogs; empty when the chunk is absent.
pub fn parse_wmo_fogs(bytes: &[u8]) -> Vec<WmoFog> {
    let Some(mfog) = find_wmo_chunk(bytes, *b"GOFM") else {
        return Vec::new();
    };
    let f = |b: &[u8], i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    mfog.as_chunks::<48>()
        .0
        .iter()
        .map(|r| WmoFog {
            flags: u(r, 0),
            pos: [f(r, 4), f(r, 8), f(r, 12)],
            radius_inner: f(r, 0x10),
            radius_outer: f(r, 0x14),
            fog_end: f(r, 0x18),
            fog_start_scalar: f(r, 0x1c),
            color: u(r, 0x20),
            uw_fog_end: f(r, 0x24),
            uw_fog_start_scalar: f(r, 0x28),
            uw_color: u(r, 0x2c),
        })
        .collect()
}

/// The root's WMO id (`MOHD` `+0x20`), its key into `WMOAreaTable`; `0` when the header is too
/// short.
pub fn wmo_root_id(root_bytes: &[u8]) -> u32 {
    find_wmo_chunk(root_bytes, *b"DHOM")
        .filter(|mohd| mohd.len() >= 0x24)
        .map_or(0, |mohd| {
            u32::from_le_bytes([mohd[0x20], mohd[0x21], mohd[0x22], mohd[0x23]])
        })
}

fn parse_wmo_doodads(bytes: &[u8]) -> (Vec<WmoDoodad>, Vec<WmoDoodadSet>) {
    let modn = find_wmo_chunk(bytes, *b"NDOM").unwrap_or(&[]);
    let resolve = |off: usize| -> String {
        if off >= modn.len() {
            return String::new();
        }
        let end = modn[off..]
            .iter()
            .position(|&b| b == 0)
            .map_or(modn.len(), |p| off + p);
        String::from_utf8_lossy(&modn[off..end]).into_owned()
    };

    let mut doodads = Vec::new();
    if let Some(modd) = find_wmo_chunk(bytes, *b"DDOM") {
        for rec in modd.as_chunks::<40>().0 {
            let f = |i: usize| f32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            let name_and_flags = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            doodads.push(WmoDoodad {
                model: resolve((name_and_flags & 0x00FF_FFFF) as usize),
                position: [f(4), f(8), f(12)],
                orientation: [f(16), f(20), f(24), f(28)],
                scale: f(32),
                color: [rec[38], rec[37], rec[36], rec[39]],
            });
        }
    }

    let mut doodad_sets = Vec::new();
    if let Some(mods) = find_wmo_chunk(bytes, *b"SDOM") {
        for rec in mods.as_chunks::<32>().0 {
            let u = |i: usize| u32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            doodad_sets.push(WmoDoodadSet {
                start: u(20),
                count: u(24),
            });
        }
    }
    (doodads, doodad_sets)
}

#[cfg(test)]
pub(crate) mod test_bytes {
    pub(crate) fn chunk(reversed_magic: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + data.len());
        out.extend_from_slice(&reversed_magic);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    pub(crate) fn f3(v: [f32; 3]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_bytes::{chunk, f3};
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_portal_vertices_infos_and_refs() {
        let mut mopv = Vec::new();
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            mopv.extend(f3(v));
        }
        let mut mopt = Vec::new();
        mopt.extend_from_slice(&0u16.to_le_bytes());
        mopt.extend_from_slice(&3u16.to_le_bytes());
        mopt.extend(f3([0.0, 0.0, 1.0]));
        mopt.extend_from_slice(&(-2.5f32).to_le_bytes());
        let mut mopr = Vec::new();
        for (p, g, s) in [(0u16, 1u16, 1i16), (0, 7, -1)] {
            mopr.extend_from_slice(&p.to_le_bytes());
            mopr.extend_from_slice(&g.to_le_bytes());
            mopr.extend_from_slice(&s.to_le_bytes());
            mopr.extend_from_slice(&0u16.to_le_bytes());
        }
        let mut bytes = Vec::new();
        bytes.extend(chunk(*b"VPOM", &mopv));
        bytes.extend(chunk(*b"TPOM", &mopt));
        bytes.extend(chunk(*b"RPOM", &mopr));

        let p = parse_wmo_portals(&bytes);
        assert_eq!(p.vertices.len(), 3);
        assert_eq!(p.vertices[2], [0.0, 0.0, 1.0]);
        assert_eq!(p.infos.len(), 1);
        assert_eq!(p.infos[0].start_vertex, 0);
        assert_eq!(p.infos[0].count, 3);
        assert_eq!(p.infos[0].plane, [0.0, 0.0, 1.0, -2.5]);
        assert_eq!(p.refs.len(), 2);
        assert_eq!(
            (p.refs[0].portal, p.refs[0].group, p.refs[0].side),
            (0, 1, 1)
        );
        assert_eq!(
            (p.refs[1].portal, p.refs[1].group, p.refs[1].side),
            (0, 7, -1)
        );
    }

    #[test]
    fn absent_portal_chunks_yield_empty_graph() {
        let p = parse_wmo_portals(&[]);
        assert!(p.vertices.is_empty() && p.infos.is_empty() && p.refs.is_empty());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_mfog_records() {
        let mut mfog = Vec::new();
        for (pos, r_in, r_out, end, start_scalar, color) in [
            (
                [0.0f32, 0.0, 0.0],
                0.0f32,
                0.0f32,
                444.4445f32,
                0.25f32,
                0xff00_0000_u32,
            ),
            ([10.0, -4.0, 2.5], 5.0, 20.0, 80.0, 0.1, 0xff10_2030),
        ] {
            mfog.extend_from_slice(&0u32.to_le_bytes());
            mfog.extend(f3(pos));
            for v in [r_in, r_out, end, start_scalar] {
                mfog.extend_from_slice(&v.to_le_bytes());
            }
            mfog.extend_from_slice(&color.to_le_bytes());
            for v in [222.2f32, 0.5] {
                mfog.extend_from_slice(&v.to_le_bytes());
            }
            mfog.extend_from_slice(&0xff44_5566_u32.to_le_bytes());
        }
        let bytes = chunk(*b"GOFM", &mfog);

        let fogs = parse_wmo_fogs(&bytes);
        assert_eq!(fogs.len(), 2);
        assert_eq!(fogs[0].fog_end, 444.4445);
        assert_eq!(fogs[0].color, 0xff00_0000);
        assert_eq!(fogs[1].pos, [10.0, -4.0, 2.5]);
        assert_eq!(fogs[1].radius_inner, 5.0);
        assert_eq!(fogs[1].radius_outer, 20.0);
        assert_eq!(fogs[1].fog_start_scalar, 0.1);
        assert_eq!(fogs[1].uw_fog_end, 222.2);
        assert_eq!(fogs[1].uw_color, 0xff44_5566);
        assert!(parse_wmo_fogs(&[]).is_empty());
    }
}
