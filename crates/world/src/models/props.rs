//! The doodads a building places inside itself: resolved from its root, lit as its rooms light
//! them, and spawned with it.

use std::sync::Arc;

use bevy::prelude::*;

use super::{DoodadLight, Spawner, ground_shade};
use crate::adt::AdtTile;
use crate::coords::wow_to_bevy;
use crate::m2::M2Model;
use crate::model_material::GroundShade;
use crate::placements::prop_placements;
use crate::portal::WmoGroupVis;
use crate::probes::{ProbeSlot, Probes, PropLobeLight, fold_interior_probe};
use crate::stream::Streamer;
use crate::wmo::{DoodadBase, WmoModel};

pub(super) struct Prop {
    pub(super) handle: Handle<M2Model>,
    pub(super) transform: Transform,
    pub(super) groups: Arc<[u16]>,
    pub(super) light: PropLight,
}

pub(super) enum PropLight {
    Exterior,
    Interior {
        ambient: [f32; 3],
        diffuse: [f32; 3],
        lights: Vec<PropLobeLight>,
    },
}

pub(super) struct PropSite<'a> {
    pub(super) building: Entity,
    pub(super) streamer: &'a Streamer,
    pub(super) adts: &'a Assets<AdtTile>,
}

pub(super) fn resolve_props(
    wmo: &WmoModel,
    doodad_set: u16,
    world: &Transform,
    server: &AssetServer,
) -> Vec<Prop> {
    prop_placements(wmo, doodad_set, world)
        .into_iter()
        .map(|p| {
            let light = match wmo.doodad_base.get(p.doodad) {
                Some(DoodadBase::Interior {
                    ambient,
                    diffuse,
                    light_refs,
                }) => PropLight::Interior {
                    ambient: *ambient,
                    diffuse: *diffuse,
                    lights: light_refs
                        .iter()
                        .filter_map(|&li| wmo.lights.get(li as usize))
                        .filter(|l| l.is_omni())
                        .map(|l| PropLobeLight {
                            pos: world.transform_point(wow_to_bevy(l.position)),
                            color_i: l.color.map(|c| c * l.intensity.max(0.0)),
                            atten_start: l.attenuation_start,
                            atten_end: l.attenuation_end,
                        })
                        .collect(),
                },
                _ => PropLight::Exterior,
            };
            Prop {
                handle: server.load(p.url),
                transform: p.transform,
                groups: p.groups,
                light,
            }
        })
        .collect()
}

impl Spawner<'_, '_, '_, '_> {
    pub(super) fn prop(
        &mut self,
        m: &M2Model,
        form: &[Handle<Mesh>],
        prop: &Prop,
        site: &PropSite<'_>,
        probes: &mut Probes,
        placement_forms: &mut Vec<Arc<[Handle<Mesh>]>>,
    ) -> Vec<Entity> {
        let (building, streamer, adts) = (site.building, site.streamer, site.adts);
        let light = match &prop.light {
            PropLight::Exterior => DoodadLight::Sky(
                ground_shade(streamer, adts, &prop.transform).unwrap_or(GroundShade::Lit),
            ),
            PropLight::Interior {
                ambient,
                diffuse,
                lights,
            } => {
                let (_, center) = m.fade_sphere(prop.transform.scale.x);
                let ref_point = prop.transform.transform_point(center);
                let probe = fold_interior_probe(*ambient, *diffuse, ref_point, lights);
                probes
                    .alloc_shared(probe)
                    .map_or(DoodadLight::Sky(GroundShade::Lit), DoodadLight::Probe)
            }
        };
        let model = prop.handle.id().untyped();
        let room = (!prop.groups.is_empty()).then(|| WmoGroupVis {
            instance: building,
            groups: prop.groups.clone(),
        });
        let placed = self.doodad(
            m,
            model,
            form,
            &prop.transform,
            light,
            Some(building),
            room.as_ref(),
            placement_forms,
        );
        let mut ents = placed.batches;
        if let DoodadLight::Probe(slot) = light {
            let owner = if let Some(&e) = ents.first() {
                e
            } else {
                let carrier = self.commands.spawn_empty().id();
                ents.push(carrier);
                carrier
            };
            self.commands.entity(owner).insert(ProbeSlot(slot));
        }
        if let Some(room) = &room {
            for &e in &ents {
                self.commands.entity(e).insert(room.clone());
            }
        }
        self.doodad_lights(m, &prop.transform, room.as_ref(), &mut ents);
        ents.extend(placed.rig_root);
        ents.extend(placed.effects);
        ents
    }
}
