use bevy::prelude::*;

use super::stream::{
    CollisionStreamer, Hull, Placement, PlacementModel, TileId, WeldGroup, despawn_all,
};
use crate::placements::{Filed, PlacementEdits};
use crate::source::{m2_url, wmo_url};

pub(super) struct Edited {
    filed: Option<Filed>,
    placement: Option<Placement>,
}

impl Edited {
    pub(super) fn placement(&mut self) -> Option<&mut Placement> {
        self.placement.as_mut()
    }

    pub(super) fn standing(&self) -> Option<&Placement> {
        self.placement.as_ref()
    }
}

pub(super) fn follow_edits(
    mut commands: Commands<'_, '_>,
    edits: Res<'_, PlacementEdits>,
    mut streamer: ResMut<'_, CollisionStreamer>,
) {
    if !edits.is_changed() {
        return;
    }
    for (uid, filed) in edits.iter() {
        let unchanged = streamer
            .edits
            .get(&uid)
            .is_some_and(|e| e.filed.as_ref() == filed);
        if unchanged {
            continue;
        }
        if let Some(old) = streamer.edits.remove(&uid) {
            despawn_all(
                &mut commands,
                old.placement.map(|p| p.entities).unwrap_or_default(),
            );
        }
        take_down_tile_colliders(&mut commands, &mut streamer, uid);
        let placement = filed.map(|f| {
            let model = match f {
                Filed::Doodad(d) => PlacementModel::M2 {
                    hull: Hull::Unasked(m2_url(&d.model)),
                    welded: false,
                    group: WeldGroup::Own,
                },
                Filed::Building(w) => PlacementModel::Wmo {
                    hull: Hull::Unasked(wmo_url(&w.model)),
                    doodad_set: w.doodad_set,
                },
            };
            Placement::unowned(model, f.transform())
        });
        let filed = filed.cloned();
        streamer.edits.insert(uid, Edited { filed, placement });
    }
}

fn take_down_tile_colliders(
    commands: &mut Commands<'_, '_>,
    streamer: &mut CollisionStreamer,
    uid: u32,
) {
    let Some(p) = streamer.placements.get_mut(&uid) else {
        return;
    };
    despawn_all(commands, std::mem::take(&mut p.entities));
    p.unwelded_props.clear();
    match (&p.model, p.owner) {
        (
            Some(PlacementModel::M2 {
                welded: true,
                group,
                ..
            }),
            Some(owner),
        ) => {
            let group = *group;
            unweld(commands, streamer, owner, group);
        }
        (Some(PlacementModel::Wmo { .. }), _) => p.model = None,
        _ => {}
    }
}

fn unweld(
    commands: &mut Commands<'_, '_>,
    streamer: &mut CollisionStreamer,
    owner: TileId,
    group: WeldGroup,
) {
    if let Some(tile) = streamer.tiles_by_ask.get_mut(&owner) {
        despawn_all(commands, tile.welds.remove(&group).unwrap_or_default());
    }
    for p in streamer.placements.values_mut() {
        if let Some(PlacementModel::M2 {
            welded,
            group: its_group,
            ..
        }) = &mut p.model
            && p.owner == Some(owner)
            && *its_group == group
        {
            *welded = false;
        }
    }
}
