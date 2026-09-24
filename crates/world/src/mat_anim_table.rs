use std::num::NonZeroU16;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::light::{LightBuffer, MATANIM_OFFSET, MATANIM_ROWS};

#[derive(Resource, Clone, ExtractResource)]
pub(crate) struct MatAnimTable {
    rows: Arc<Vec<[f32; 4]>>,
    generation: u64,
    next: NonZeroU16,
    free: Vec<NonZeroU16>,
}

impl Default for MatAnimTable {
    fn default() -> Self {
        Self {
            rows: Arc::new(vec![[0.0; 4]; MATANIM_ROWS]),
            generation: 0,
            next: NonZeroU16::MIN,
            free: Vec::new(),
        }
    }
}

impl MatAnimTable {
    pub(crate) fn alloc(&mut self) -> Option<NonZeroU16> {
        if let Some(slot) = self.free.pop() {
            return Some(slot);
        }
        let slot = self.next;
        if usize::from(slot.get()) >= MATANIM_ROWS {
            return None;
        }
        self.next = slot.checked_add(1)?;
        Some(slot)
    }

    pub(crate) fn free(&mut self, slot: NonZeroU16) {
        self.set(slot, [0.0; 4]);
        self.free.push(slot);
    }

    pub(crate) fn set(&mut self, slot: NonZeroU16, row: [f32; 4]) {
        let i = usize::from(slot.get());
        if i >= MATANIM_ROWS || self.rows[i].map(f32::to_bits) == row.map(f32::to_bits) {
            return;
        }
        Arc::make_mut(&mut self.rows)[i] = row;
        self.generation += 1;
    }

    #[cfg(test)]
    pub(crate) fn row(&self, i: usize) -> [f32; 4] {
        self.rows.get(i).copied().unwrap_or([0.0; 4])
    }
}

fn upload(
    queue: Res<'_, RenderQueue>,
    buffer: Option<Res<'_, LightBuffer>>,
    table: Option<Res<'_, MatAnimTable>>,
    mut last: Local<'_, Option<u64>>,
) {
    let (Some(buffer), Some(table)) = (buffer, table) else {
        return;
    };
    if *last == Some(table.generation) {
        return;
    }
    *last = Some(table.generation);
    let bytes: Vec<u8> = table
        .rows
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    queue.write_buffer(&buffer.0, MATANIM_OFFSET, &bytes);
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<MatAnimTable>()
        .add_plugins(ExtractResourcePlugin::<MatAnimTable>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn row_zero_stays_zero_with_every_slot_written() {
        let mut t = MatAnimTable::default();
        while let Some(slot) = t.alloc() {
            t.set(slot, [1.0, 2.0, 3.0, 4.0]);
        }
        assert_eq!(t.row(0), [0.0; 4]);
        assert_eq!(t.row(1), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_freed_slot_comes_back_first_and_zeroed() {
        let mut t = MatAnimTable::default();
        let a = t.alloc().expect("room");
        assert_eq!(a.get(), 1);
        t.set(a, [0.25, -0.5, 0.0, 0.0]);
        t.free(a);
        assert_eq!(t.row(1), [0.0; 4]);
        assert_eq!(t.alloc(), Some(a));
    }

    #[test]
    fn only_a_changed_row_moves_the_generation() {
        let mut t = MatAnimTable::default();
        let s = t.alloc().expect("room");
        assert_eq!(t.generation, 0);
        t.set(s, [0.1, 0.2, 0.0, 0.0]);
        t.set(s, [0.1, 0.2, 0.0, 0.0]);
        assert_eq!(t.generation, 1);
    }

    #[test]
    fn a_full_table_says_so_until_a_slot_frees() {
        let mut t = MatAnimTable::default();
        let slots: Vec<NonZeroU16> = std::iter::from_fn(|| t.alloc()).collect();
        assert_eq!(slots.len(), MATANIM_ROWS - 1);
        assert!(t.alloc().is_none());
        t.free(slots[7]);
        assert_eq!(t.alloc(), Some(slots[7]));
    }
}
