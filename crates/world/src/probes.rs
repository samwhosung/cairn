use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::math::Vec4;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::light::{LightBuffer, PROBE_REGION_OFFSET};
use crate::sh::sh_probe_coeffs;

pub(crate) const MAX_PROP_PROBES: usize = 8192;
pub(crate) const PROBE_ROWS: usize = 7;

const TOWARD_INTERIOR_LIGHT: Vec3 = Vec3::new(-0.30822, 0.9, -0.30822);

#[derive(Clone, Copy, Debug)]
pub(crate) struct PropLobeLight {
    pub pos: Vec3,
    pub color_i: [f32; 3],
    pub atten_start: f32,
    pub atten_end: f32,
}

pub(crate) fn fold_interior_probe(
    ambient: [f32; 3],
    diffuse: [f32; 3],
    ref_point: Vec3,
    lights: &[PropLobeLight],
) -> [Vec4; 7] {
    let mut lobes: Vec<(Vec3, [f32; 3])> = vec![(TOWARD_INTERIOR_LIGHT, diffuse)];
    for l in lights {
        let dv = l.pos - ref_point;
        let dist = dv.length();
        let gain = if dist <= l.atten_start {
            1.0
        } else if dist >= l.atten_end || l.atten_end <= l.atten_start {
            0.0
        } else {
            1.0 - (dist - l.atten_start) / (l.atten_end - l.atten_start)
        };
        if gain > 0.0 {
            lobes.push((dv / dist.max(1e-4), l.color_i.map(|c| c * gain)));
        }
    }
    sh_probe_coeffs(ambient, &lobes)
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ProbeKey([[u32; 4]; PROBE_ROWS]);

#[derive(Resource)]
pub(crate) struct PropProbes {
    rows: Arc<Vec<[[f32; 4]; PROBE_ROWS]>>,
    free: Vec<u16>,
    high: usize,
    by_key: HashMap<ProbeKey, u16>,
    slots: Vec<Option<(u32, ProbeKey)>>,
    generation: u64,
}

impl Default for PropProbes {
    fn default() -> Self {
        Self {
            rows: Arc::new(vec![[[0.0; 4]; PROBE_ROWS]; MAX_PROP_PROBES]),
            free: Vec::new(),
            high: 0,
            by_key: HashMap::new(),
            slots: vec![None; MAX_PROP_PROBES],
            generation: 0,
        }
    }
}

impl PropProbes {
    pub(crate) fn alloc(&mut self, coeffs: [Vec4; PROBE_ROWS]) -> Option<u16> {
        let key = ProbeKey(coeffs.map(|v| v.to_array().map(f32::to_bits)));
        if let Some(&slot) = self.by_key.get(&key)
            && let Some(Some((refs, _))) = self.slots.get_mut(slot as usize)
        {
            *refs += 1;
            return Some(slot);
        }
        let slot = match self.free.pop() {
            Some(s) => s,
            None if self.high < MAX_PROP_PROBES => {
                self.high += 1;
                (self.high - 1) as u16
            }
            None => return None,
        };
        Arc::make_mut(&mut self.rows)[slot as usize] = coeffs.map(|v| v.to_array());
        self.by_key.insert(key.clone(), slot);
        self.slots[slot as usize] = Some((1, key));
        self.generation += 1;
        Some(slot)
    }

    fn release(&mut self, slot: u16) {
        let Some(Some((refs, _))) = self.slots.get_mut(slot as usize) else {
            return;
        };
        *refs -= 1;
        if *refs > 0 {
            return;
        }
        if let Some((_, key)) = self.slots[slot as usize].take() {
            self.by_key.remove(&key);
        }
        Arc::make_mut(&mut self.rows)[slot as usize] = [[0.0; 4]; PROBE_ROWS];
        self.free.push(slot);
        self.generation += 1;
    }
}

#[derive(Component)]
#[component(on_replace = free_slot)]
pub(crate) struct PropProbeSlot(pub u16);

fn free_slot(mut world: DeferredWorld<'_>, ctx: HookContext) {
    if let Some(slot) = world.get::<PropProbeSlot>(ctx.entity).map(|s| s.0) {
        world.resource_mut::<PropProbes>().release(slot);
    }
}

#[derive(Resource, Clone, ExtractResource)]
struct ProbeExtract {
    rows: Arc<Vec<[[f32; 4]; PROBE_ROWS]>>,
    high: usize,
    generation: u64,
}

impl Default for ProbeExtract {
    fn default() -> Self {
        Self {
            rows: Arc::new(Vec::new()),
            high: 0,
            generation: u64::MAX,
        }
    }
}

pub(crate) struct ProbePlugin;

impl Plugin for ProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropProbes>()
            .init_resource::<ProbeExtract>()
            .add_plugins(ExtractResourcePlugin::<ProbeExtract>::default())
            .add_systems(PostUpdate, publish);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
        }
    }
}

fn publish(probes: Res<'_, PropProbes>, mut out: ResMut<'_, ProbeExtract>) {
    if out.generation != probes.generation {
        out.rows = Arc::clone(&probes.rows);
        out.high = probes.high;
        out.generation = probes.generation;
    }
}

fn upload(
    queue: Res<'_, RenderQueue>,
    buffer: Option<Res<'_, LightBuffer>>,
    data: Res<'_, ProbeExtract>,
    mut last: Local<'_, Option<u64>>,
) {
    let Some(buffer) = buffer else {
        return;
    };
    if *last == Some(data.generation) {
        return;
    }
    *last = Some(data.generation);
    let rows = &data.rows[..data.high.min(data.rows.len())];
    let bytes: Vec<u8> = rows
        .iter()
        .flatten()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    if !bytes.is_empty() {
        queue.write_buffer(&buffer.0, PROBE_REGION_OFFSET, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_probes_share_a_slot_until_the_last_goes() {
        let mut t = PropProbes::default();
        let c = [Vec4::splat(0.5); PROBE_ROWS];
        let a = t.alloc(c).expect("room");
        assert_eq!(t.alloc(c), Some(a));
        let other = t.alloc([Vec4::splat(0.25); PROBE_ROWS]).expect("room");
        assert_ne!(a, other);
        t.release(a);
        assert_eq!(t.alloc(c), Some(a), "one reference still holds it");
        t.release(a);
        t.release(a);
        assert_eq!(t.rows[a as usize], [[0.0; 4]; PROBE_ROWS]);
        assert_eq!(
            t.alloc([Vec4::ONE; PROBE_ROWS]),
            Some(a),
            "a freed slot is reused"
        );
    }

    #[test]
    fn a_light_past_its_window_adds_nothing() {
        let far = PropLobeLight {
            pos: Vec3::new(0.0, 0.0, 50.0),
            color_i: [1.0; 3],
            atten_start: 1.0,
            atten_end: 10.0,
        };
        let alone = fold_interior_probe([0.1; 3], [0.2; 3], Vec3::ZERO, &[]);
        assert_eq!(
            fold_interior_probe([0.1; 3], [0.2; 3], Vec3::ZERO, &[far]),
            alone
        );
    }
}
