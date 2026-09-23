use std::sync::Arc;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::light::{LightBuffer, RIG_ORIGIN_OFFSET, RIG_PALETTE_OFFSET, RIG_TABLE_OFFSET};

/// The width of the rig slot field in a part's `MeshTag`.
pub const RIG_SLOT_BITS: u32 = 11;
pub const MAX_RIG_SLOTS: usize = 1 << RIG_SLOT_BITS;
/// Bones every live rig together may hold.
pub const MAX_PALETTE_BONES: usize = 131_072;
pub const BONE_ROWS: u64 = 3;
pub const BONE_BYTES: u64 = BONE_ROWS * 16;

/// Every rig's skinning frames, written where the model shader reads them: per slot the first
/// bone's index and the world position the rows are measured from, per bone three rows of
/// `frame × inverse bind pose` relative to that position.
#[derive(Resource)]
pub struct RigPalettes {
    rows: Vec<[f32; 4]>,
    table: Vec<u32>,
    origins: Vec<[f32; 4]>,
    slot_len: Vec<u32>,
    free_slots: Vec<u16>,
    slot_high: usize,
    free_ranges: Vec<(u32, u32)>,
    dirty: Vec<(u32, u32)>,
    table_dirty: bool,
    origins_dirty: bool,
}

impl Default for RigPalettes {
    fn default() -> Self {
        Self {
            rows: vec![[0.0; 4]; 3 * MAX_PALETTE_BONES],
            table: vec![0; MAX_RIG_SLOTS],
            origins: vec![[0.0; 4]; MAX_RIG_SLOTS],
            slot_len: vec![0; MAX_RIG_SLOTS],
            free_slots: Vec::new(),
            slot_high: 1,
            free_ranges: vec![(0, MAX_PALETTE_BONES as u32)],
            dirty: Vec::new(),
            table_dirty: true,
            origins_dirty: true,
        }
    }
}

impl RigPalettes {
    fn alloc(&mut self, bones: u32) -> Option<(u16, u32)> {
        if bones == 0 {
            return None;
        }
        let (i, &(base, len)) = self
            .free_ranges
            .iter()
            .enumerate()
            .find(|(_, r)| r.1 >= bones)?;
        let slot = match self.free_slots.pop() {
            Some(s) => s,
            None if self.slot_high < MAX_RIG_SLOTS => {
                self.slot_high += 1;
                (self.slot_high - 1) as u16
            }
            None => return None,
        };
        if len == bones {
            self.free_ranges.remove(i);
        } else {
            self.free_ranges[i] = (base + bones, len - bones);
        }
        self.rows[3 * base as usize..3 * (base + bones) as usize].fill([0.0; 4]);
        self.table[slot as usize] = base;
        self.slot_len[slot as usize] = bones;
        self.table_dirty = true;
        Some((slot, base))
    }

    fn free(&mut self, slot: u16) {
        let s = slot as usize;
        let Some(&len) = self.slot_len.get(s).filter(|&&l| l > 0) else {
            return;
        };
        let base = self.table[s];
        self.slot_len[s] = 0;
        self.free_slots.push(slot);
        self.table_dirty = true;
        self.rows[3 * base as usize..3 * (base + len) as usize].fill([0.0; 4]);
        self.dirty.push((base, len));
        self.set_origin(slot, Vec3::ZERO);
        let i = self.free_ranges.partition_point(|&(b, _)| b < base);
        self.free_ranges.insert(i, (base, len));
        if i + 1 < self.free_ranges.len() {
            let (nb, nl) = self.free_ranges[i + 1];
            if base + len == nb {
                self.free_ranges[i].1 += nl;
                self.free_ranges.remove(i + 1);
            }
        }
        if i > 0 {
            let (pb, pl) = self.free_ranges[i - 1];
            if pb + pl == base {
                self.free_ranges[i - 1].1 += self.free_ranges[i].1;
                self.free_ranges.remove(i);
            }
        }
    }

    fn set_origin(&mut self, slot: u16, origin: Vec3) {
        let word = [origin.x, origin.y, origin.z, 0.0];
        if let Some(cur) = self.origins.get_mut(slot as usize)
            && cur.map(f32::to_bits) != word.map(f32::to_bits)
        {
            *cur = word;
            self.origins_dirty = true;
        }
    }

    pub(crate) fn write_rig_worlds(
        &mut self,
        rig: &RigSkin,
        origin_relative: &[GlobalTransform],
        origin: Vec3,
    ) {
        let n = (origin_relative.len().min(rig.ibp.len()) as u32).min(rig.len);
        self.set_origin(rig.slot, origin);
        for (b, world) in origin_relative.iter().enumerate().take(n as usize) {
            let m = world.affine() * Affine3A::from_mat4(rig.ibp[b]);
            let (m3, t) = (m.matrix3, m.translation);
            let r = 3 * (rig.base as usize + b);
            self.rows[r] = [m3.x_axis.x, m3.y_axis.x, m3.z_axis.x, t.x];
            self.rows[r + 1] = [m3.x_axis.y, m3.y_axis.y, m3.z_axis.y, t.y];
            self.rows[r + 2] = [m3.x_axis.z, m3.y_axis.z, m3.z_axis.z, t.z];
        }
        if n > 0 {
            self.dirty.push((rig.base, n));
        }
    }

    /// A slot's rows as world-space matrices, the slot's origin added back.
    pub fn world_palette(&self, slot: u16) -> Option<Vec<Mat4>> {
        let s = slot as usize;
        let len = *self.slot_len.get(s)? as usize;
        let base = *self.table.get(s)? as usize;
        let o = self.origins.get(s)?;
        (len > 0).then(|| {
            (0..len)
                .map(|b| {
                    let r = 3 * (base + b);
                    let row = |i: usize| {
                        let mut v = self.rows[r + i];
                        v[3] += o[i];
                        Vec4::from_array(v)
                    };
                    Mat4::from_cols(row(0), row(1), row(2), Vec4::W).transpose()
                })
                .collect()
        })
    }
}

/// A rig's slot in [`RigPalettes`], on the entity that owns its pose; replacing or removing it
/// frees the slot.
#[derive(Component)]
#[component(on_replace = free_rig_skin)]
pub struct RigSkin {
    pub slot: u16,
    base: u32,
    len: u32,
    ibp: Arc<[Mat4]>,
}

impl RigSkin {
    /// Claims a slot for `ibp.len()` bones; `None` when the palette is full or `ibp` is empty.
    pub fn allocate(palettes: &mut RigPalettes, ibp: Arc<[Mat4]>) -> Option<Self> {
        let bones = ibp.len() as u32;
        let (slot, base) = palettes.alloc(bones)?;
        Some(Self {
            slot,
            base,
            len: bones,
            ibp,
        })
    }
}

fn free_rig_skin(mut world: DeferredWorld<'_>, ctx: HookContext) {
    let Some(slot) = world.get::<RigSkin>(ctx.entity).map(|r| r.slot) else {
        return;
    };
    if let Some(mut palettes) = world.get_resource_mut::<RigPalettes>() {
        palettes.free(slot);
    }
}

#[derive(Resource, Clone, Default, ExtractResource)]
struct PaletteUpload {
    ranges: Arc<Vec<(u32, Vec<[f32; 4]>)>>,
    table: Option<Arc<Vec<u32>>>,
    origins: Option<Arc<Vec<[f32; 4]>>>,
}

fn publish(mut palettes: ResMut<'_, RigPalettes>, mut out: ResMut<'_, PaletteUpload>) {
    let p = palettes.as_mut();
    if p.dirty.is_empty() && !p.table_dirty && !p.origins_dirty {
        if !out.ranges.is_empty() || out.table.is_some() || out.origins.is_some() {
            *out = PaletteUpload::default();
        }
        return;
    }
    let ranges = std::mem::take(&mut p.dirty)
        .into_iter()
        .map(|(base, len)| {
            let rows = p.rows[3 * base as usize..3 * (base + len) as usize].to_vec();
            (base, rows)
        })
        .collect();
    *out = PaletteUpload {
        ranges: Arc::new(ranges),
        table: std::mem::take(&mut p.table_dirty).then(|| Arc::new(p.table.clone())),
        origins: std::mem::take(&mut p.origins_dirty).then(|| Arc::new(p.origins.clone())),
    };
}

fn upload(
    queue: Res<'_, RenderQueue>,
    buffer: Option<Res<'_, LightBuffer>>,
    data: Option<Res<'_, PaletteUpload>>,
    mut last: Local<'_, usize>,
) {
    let (Some(buffer), Some(data)) = (buffer, data) else {
        return;
    };
    let id = Arc::as_ptr(&data.ranges) as usize;
    if *last == id {
        return;
    }
    *last = id;
    if let Some(table) = &data.table {
        let bytes: Vec<u8> = table.iter().flat_map(|v| v.to_le_bytes()).collect();
        queue.write_buffer(&buffer.0, RIG_TABLE_OFFSET, &bytes);
    }
    if let Some(origins) = &data.origins {
        let bytes: Vec<u8> = origins
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        queue.write_buffer(&buffer.0, RIG_ORIGIN_OFFSET, &bytes);
    }
    for (base, rows) in data.ranges.iter() {
        let bytes: Vec<u8> = rows
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        queue.write_buffer(
            &buffer.0,
            RIG_PALETTE_OFFSET + u64::from(*base) * BONE_BYTES,
            &bytes,
        );
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<RigPalettes>()
        .init_resource::<PaletteUpload>()
        .add_plugins(ExtractResourcePlugin::<PaletteUpload>::default())
        .add_systems(PostUpdate, publish.after(super::compose::RigFinalize));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_and_ranges_come_back_and_coalesce() {
        let mut p = RigPalettes::default();
        let (s1, b1) = p.alloc(10).expect("room");
        let (s2, b2) = p.alloc(20).expect("room");
        let (s3, b3) = p.alloc(30).expect("room");
        assert!(s1 >= 1);
        assert_eq!((b1, b2, b3), (0, 10, 30));
        p.free(s2);
        let (s4, b4) = p.alloc(20).expect("room");
        assert_eq!(b4, 10);
        p.free(s1);
        p.free(s4);
        p.free(s3);
        assert_eq!(p.free_ranges, vec![(0, MAX_PALETTE_BONES as u32)]);
    }

    #[test]
    fn freed_rows_and_origins_read_zero() {
        let mut p = RigPalettes::default();
        let (slot, base) = p.alloc(2).expect("room");
        p.rows[3 * base as usize] = [1.0; 4];
        p.set_origin(slot, Vec3::new(-9464.31, 62.17, 56.91));
        p.free(slot);
        assert_eq!(p.rows[3 * base as usize].map(f32::to_bits), [0; 4]);
        assert_eq!(p.origins[slot as usize].map(f32::to_bits), [0; 4]);
        assert!(p.dirty.contains(&(base, 2)));
    }

    #[test]
    fn the_palette_reads_back_in_world_space() {
        let mut p = RigPalettes::default();
        let ibp: Arc<[Mat4]> = Arc::from([Mat4::from_translation(-Vec3::Y)].as_slice());
        let skin = RigSkin::allocate(&mut p, ibp).expect("room");
        let origin = Vec3::new(-9464.31, 62.17, 56.91);
        let frame = Transform::from_rotation(Quat::from_rotation_y(0.7))
            .with_translation(Vec3::new(0.3, 1.4, -0.2));
        p.write_rig_worlds(&skin, &[GlobalTransform::from(frame)], origin);
        let want =
            Mat4::from_translation(origin) * frame.to_matrix() * Mat4::from_translation(-Vec3::Y);
        let got = p.world_palette(skin.slot).expect("allocated")[0];
        assert!(got.abs_diff_eq(want, 1e-2), "{got} vs {want}");
    }
}
