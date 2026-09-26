use std::collections::BTreeMap;

use bevy::prelude::*;
use terrain::{DOODAD_SCALE_STEPS, Doodad, WmoInstance, placement_to_world, world_to_placement};

use super::{PlacedModel, Placement};
use crate::coords::{placement_rotation, wow_to_bevy};
use crate::source::{m2_url, wmo_url};

/// A placement as a map's files hold it: a doodad's record or a building's, read into the world's
/// axes.
#[derive(Clone, Debug, PartialEq)]
pub enum Filed {
    Doodad(Doodad),
    Building(WmoInstance),
}

impl Filed {
    /// A new placement of the install's model at `model`, a building when it names a `.wmo`, at
    /// the world's origin; `None` when it names neither a building nor a doodad.
    pub fn of(model: &str, unique_id: u32, doodad_set: u16) -> Option<Self> {
        let extension = std::path::Path::new(model)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let (position, rotation) = ([0.0; 3], [0.0; 3]);
        if extension.as_deref() == Some("wmo") {
            return Some(Self::Building(WmoInstance {
                model: model.to_owned(),
                position,
                rotation,
                unique_id,
                doodad_set,
                name_set: 0,
            }));
        }
        matches!(extension.as_deref(), Some("m2" | "mdx" | "mdl")).then(|| {
            Self::Doodad(Doodad {
                model: model.to_owned(),
                position,
                rotation,
                scale: 1.0,
                unique_id,
            })
        })
    }

    pub fn unique_id(&self) -> u32 {
        match self {
            Self::Doodad(d) => d.unique_id,
            Self::Building(w) => w.unique_id,
        }
    }

    /// The model's path as the record names it.
    pub fn model(&self) -> &str {
        match self {
            Self::Doodad(d) => &d.model,
            Self::Building(w) => &w.model,
        }
    }

    /// World coordinates.
    pub fn position(&self) -> [f32; 3] {
        match self {
            Self::Doodad(d) => d.position,
            Self::Building(w) => w.position,
        }
    }

    /// Euler angles in degrees; the second is the heading.
    pub fn rotation(&self) -> [f32; 3] {
        match self {
            Self::Doodad(d) => d.rotation,
            Self::Building(w) => w.rotation,
        }
    }

    /// A building's record holds none: 1.
    pub fn scale(&self) -> f32 {
        match self {
            Self::Doodad(d) => d.scale,
            Self::Building(_) => 1.0,
        }
    }

    /// Stood at `position`, turned to `rotation` and scaled by `scale`, each rounded as the files
    /// hold it: the place to what its distance from the map's corner keeps, a doodad's scale to a
    /// 1024th, from 1 to 65535 of them. A building keeps its scale of 1.
    #[must_use]
    pub fn stood(self, position: [f32; 3], rotation: [f32; 3], scale: f32) -> Self {
        let position = placement_to_world(world_to_placement(position));
        match self {
            Self::Doodad(d) => Self::Doodad(Doodad {
                position,
                rotation,
                scale: scale_steps(scale).clamp(1.0, f32::from(u16::MAX)) / DOODAD_SCALE_STEPS,
                ..d
            }),
            Self::Building(w) => Self::Building(WmoInstance {
                position,
                rotation,
                ..w
            }),
        }
    }

    /// Whether a doodad's record holds `scale` without clamping it.
    pub fn holds_scale(scale: f32) -> bool {
        (1.0..=f32::from(u16::MAX)).contains(&scale_steps(scale))
    }

    #[must_use]
    pub fn with_id(self, unique_id: u32) -> Self {
        match self {
            Self::Doodad(d) => Self::Doodad(Doodad { unique_id, ..d }),
            Self::Building(w) => Self::Building(WmoInstance { unique_id, ..w }),
        }
    }

    /// Model space to world, in Bevy's axes.
    pub fn transform(&self) -> Transform {
        Transform {
            translation: wow_to_bevy(self.position()),
            rotation: placement_rotation(self.rotation()),
            scale: Vec3::splat(self.scale()),
        }
    }

    pub fn placed_model(&self) -> PlacedModel {
        match self {
            Self::Doodad(d) => PlacedModel::Doodad {
                url: m2_url(&d.model),
            },
            Self::Building(w) => PlacedModel::Building {
                url: wmo_url(&w.model),
                doodad_set: w.doodad_set,
                name_set: w.name_set,
            },
        }
    }

    pub(super) fn placement(&self) -> Placement {
        Placement {
            model: self.placed_model(),
            transform: self.transform(),
            filed: Some(self.clone()),
            refs: 0,
        }
    }
}

fn scale_steps(scale: f32) -> f32 {
    (scale * DOODAD_SCALE_STEPS).round()
}

/// What changed at run time of what the map places: a placement stood anew under its unique id,
/// over what the map's files or an earlier edit put there, or one taken away. An edit holds while
/// the tiles that name its id come and go.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct PlacementEdits(BTreeMap<u32, Option<Filed>>);

impl PlacementEdits {
    pub fn place(&mut self, filed: Filed) {
        self.0.insert(filed.unique_id(), Some(filed));
    }

    pub fn remove(&mut self, unique_id: u32) {
        self.0.insert(unique_id, None);
    }

    /// In the order of their unique ids.
    pub fn iter(&self) -> impl Iterator<Item = (u32, Option<&Filed>)> {
        self.0.iter().map(|(&id, f)| (id, f.as_ref()))
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_placement_stands_as_its_record_would_hold_it() {
        let lamp = Filed::of("World\\Lamp.m2", 9, 0).expect("a doodad");
        let stood = lamp.stood([-9_433.123, 44.000_01, 57.5], [0.0, 90.0, 0.0], 1.300_07);
        let stored = world_to_placement(stood.position());
        assert_eq!(placement_to_world(stored), stood.position());
        assert_eq!(stood.scale(), 1331.0 / 1024.0);
        assert!(Filed::holds_scale(63.9) && !Filed::holds_scale(64.0));
        assert!(!Filed::holds_scale(0.0));
        let inn = Filed::of("World\\Inn.WMO", 4, 1).expect("a building");
        assert_eq!(inn.transform().scale, Vec3::ONE);
        assert_eq!(inn.stood([1.0; 3], [0.0; 3], 3.0).scale(), 1.0);
        assert!(Filed::of("World\\Lamp.blp", 9, 0).is_none());
    }
}
