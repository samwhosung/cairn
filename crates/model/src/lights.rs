use wowfile::ByteExt;

/// One M2 light, in model space (WoW axes) relative to `bone`.
///
/// Every track-valued field is its track's first key; an empty track reads as white for a
/// colour, `1` for the diffuse intensity, and `0` otherwise.
#[derive(Debug, Clone, Copy)]
pub struct M2Light {
    /// `1` is a point light; any other value is directional.
    pub light_type: u16,
    pub bone: i16,
    pub position: [f32; 3],
    pub ambient_color: [f32; 3],
    pub ambient_intensity: f32,
    pub diffuse_color: [f32; 3],
    pub diffuse_intensity: f32,
    /// The authored falloff range. The client ignores it: a point light falls off as
    /// `1 / (0.7·d + 0.03·d²)`.
    pub attenuation_start: f32,
    pub attenuation_end: f32,
    /// The light bone's +Z axis in model space at rest, which points toward a directional light.
    /// `[0, 0, 1]` when no bone on the chain has a rotation key.
    pub bone_z: [f32; 3],
    /// The visibility track's first key is `0`, which keeps the light dark. A keyless track
    /// leaves it on.
    pub visibility_off: bool,
}

impl M2Light {
    pub fn is_point(&self) -> bool {
        self.light_type == 1
    }

    /// Whether the client casts it: a point light its visibility track does not hold dark.
    pub fn casts(&self) -> bool {
        self.is_point() && !self.visibility_off
    }
}

/// One `MOLT` light, in WMO model space.
#[derive(Debug, Clone, Copy)]
pub struct WmoLight {
    pub light_type: u8,
    pub use_atten: bool,
    /// RGB in `0..=1`.
    pub color: [f32; 3],
    pub position: [f32; 3],
    pub intensity: f32,
    pub attenuation_start: f32,
    pub attenuation_end: f32,
}

impl WmoLight {
    /// An omni light, the only type the client lights with.
    pub fn is_omni(&self) -> bool {
        self.light_type == 0
    }
}

fn track_first_f32(bytes: &[u8], track: usize) -> Option<f32> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    bytes.f32_at(ofs)
}
fn track_first_u8(bytes: &[u8], track: usize) -> Option<u8> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    bytes.get(ofs).copied()
}
fn track_first_vec3(bytes: &[u8], track: usize) -> Option<[f32; 3]> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    Some([
        bytes.f32_at(ofs)?,
        bytes.f32_at(ofs + 4)?,
        bytes.f32_at(ofs + 8)?,
    ])
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_rotate_z(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

fn track_first_quat(bytes: &[u8], track: usize) -> Option<[f32; 4]> {
    let nval = bytes.u32_at(track + 0x14)? as usize;
    let ofs = bytes.u32_at(track + 0x18)? as usize;
    if nval == 0 {
        return None;
    }
    Some([
        bytes.f32_at(ofs)?,
        bytes.f32_at(ofs + 4)?,
        bytes.f32_at(ofs + 8)?,
        bytes.f32_at(ofs + 12)?,
    ])
}

fn bone_z_axis(bytes: &[u8], bone: i16) -> [f32; 3] {
    let (Some(count), Some(ofs)) = (bytes.u32_at(0x34), bytes.u32_at(0x38)) else {
        return [0.0, 0.0, 1.0];
    };
    let (count, ofs) = (count as usize, ofs as usize);
    let mut leaf_to_root: Vec<[f32; 4]> = Vec::new();
    let mut idx = bone;
    for _ in 0..=count {
        if idx < 0 || idx as usize >= count {
            break;
        }
        let rec = ofs + idx as usize * 0x6c;
        if let Some(q) = track_first_quat(bytes, rec + 0x28) {
            leaf_to_root.push(q);
        }
        let flags = bytes.u32_at(rec + 4).unwrap_or(0);
        if flags & 0x4 != 0 {
            break;
        }
        idx = bytes.u16_at(rec + 8).map_or(-1, |p| p as i16);
    }
    let mut q = [0.0, 0.0, 0.0, 1.0];
    for local in leaf_to_root.iter().rev() {
        q = quat_mul(q, *local);
    }
    quat_rotate_z(q)
}

fn read_m2_light(bytes: &[u8], rec: usize) -> Option<M2Light> {
    let bone = bytes.u16_at(rec + 0x02)? as i16;
    Some(M2Light {
        light_type: bytes.u16_at(rec)?,
        bone,
        bone_z: bone_z_axis(bytes, bone),
        position: [
            bytes.f32_at(rec + 0x04)?,
            bytes.f32_at(rec + 0x08)?,
            bytes.f32_at(rec + 0x0c)?,
        ],
        ambient_color: track_first_vec3(bytes, rec + 0x10).unwrap_or([1.0; 3]),
        ambient_intensity: track_first_f32(bytes, rec + 0x2c).unwrap_or(0.0),
        diffuse_color: track_first_vec3(bytes, rec + 0x48).unwrap_or([1.0; 3]),
        diffuse_intensity: track_first_f32(bytes, rec + 0x64).unwrap_or(1.0),
        attenuation_start: track_first_f32(bytes, rec + 0x80).unwrap_or(0.0),
        attenuation_end: track_first_f32(bytes, rec + 0x9c).unwrap_or(0.0),
        visibility_off: track_first_u8(bytes, rec + 0xb8) == Some(0),
    })
}

/// Reads an M2's lights, stopping at the first record that does not fit in `bytes`.
pub fn parse_m2_lights(bytes: &[u8]) -> Vec<M2Light> {
    let (Some(count), Some(ofs)) = (bytes.u32_at(0x11c), bytes.u32_at(0x120)) else {
        return Vec::new();
    };
    let (count, ofs) = (count as usize, ofs as usize);
    let mut out = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let rec = match ofs.checked_add(i * 0xd4) {
            Some(r) if r + 0xd4 <= bytes.len() => r,
            _ => break,
        };
        match read_m2_light(bytes, rec) {
            Some(light) => out.push(light),
            None => break,
        }
    }
    out
}

/// One `MOLT` record at `r`: the colour is BGRA, and `+0x18..+0x28` is a quaternion the client
/// never reads.
fn read_wmo_light(b: &[u8], r: usize) -> Option<WmoLight> {
    Some(WmoLight {
        light_type: b.u8_at(r)?,
        use_atten: b.u8_at(r + 1)? != 0,
        color: [
            f32::from(b.u8_at(r + 6)?) / 255.0,
            f32::from(b.u8_at(r + 5)?) / 255.0,
            f32::from(b.u8_at(r + 4)?) / 255.0,
        ],
        position: [b.f32_at(r + 8)?, b.f32_at(r + 12)?, b.f32_at(r + 16)?],
        intensity: b.f32_at(r + 0x14)?,
        attenuation_start: b.f32_at(r + 0x28)?,
        attenuation_end: b.f32_at(r + 0x2c)?,
    })
}

/// Reads a WMO root's `MOLT` lights, stopping at the first record past the end of the file.
pub fn parse_wmo_lights(root_bytes: &[u8]) -> Vec<WmoLight> {
    let b = root_bytes;
    let mut o = 0usize;
    while o + 8 <= b.len() {
        let Some(size) = b.u32_at(o + 4).map(|v| v as usize) else {
            break;
        };
        let data = o + 8;
        if &b[o..o + 4] == b"TLOM" || &b[o..o + 4] == b"MOLT" {
            let n = size / 0x30;
            let mut out = Vec::with_capacity(n.min(256));
            for i in 0..n {
                let Some(r) = data.checked_add(i * 0x30).filter(|&r| r + 0x30 <= b.len()) else {
                    break;
                };
                match read_wmo_light(b, r) {
                    Some(light) => out.push(light),
                    None => break,
                }
            }
            return out;
        }
        o = match data.checked_add(size) {
            Some(x) => x,
            None => break,
        };
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_bone_chain() -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        let put_u32 =
            |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let put_quat = |b: &mut Vec<u8>, at: usize, q: [f32; 4]| {
            for (i, c) in q.iter().enumerate() {
                b[at + 4 * i..at + 4 * i + 4].copy_from_slice(&c.to_le_bytes());
            }
        };
        let bones = 0x100;
        put_u32(&mut b, 0x34, 2);
        put_u32(&mut b, 0x38, bones as u32);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        put_u32(&mut b, bones + 8, 0xffff);
        put_u32(&mut b, bones + 0x28 + 0x14, 1);
        put_u32(&mut b, bones + 0x28 + 0x18, 0x300);
        put_quat(&mut b, 0x300, [s, 0.0, 0.0, s]);
        let r1 = bones + 0x6c;
        put_u32(&mut b, r1 + 8, 0);
        put_u32(&mut b, r1 + 0x28 + 0x14, 1);
        put_u32(&mut b, r1 + 0x28 + 0x18, 0x340);
        put_quat(&mut b, 0x340, [0.0, 0.0, s, s]);
        b
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn bone_z_axis_composes_parent_then_local() {
        let b = two_bone_chain();
        let root = bone_z_axis(&b, 0);
        assert!((root[0]).abs() < 1e-6 && (root[1] + 1.0).abs() < 1e-6 && root[2].abs() < 1e-6);
        let child = bone_z_axis(&b, 1);
        for (c, r) in child.iter().zip(root) {
            assert!((c - r).abs() < 1e-6, "child {child:?} vs root {root:?}");
        }
        assert_eq!(bone_z_axis(&vec![0u8; 0x400], 0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn inputs_too_short_for_the_tables_hold_no_lights() {
        assert!(parse_m2_lights(&[0u8; 16]).is_empty());
        assert!(parse_wmo_lights(&[0u8; 8]).is_empty());
    }
}
