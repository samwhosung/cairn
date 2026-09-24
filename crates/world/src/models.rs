use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU16;
use std::sync::{Arc, Weak};

use bevy::asset::{LoadState, RecursiveDependencyLoadState, UntypedAssetId};
use bevy::camera::primitives::Sphere;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use crate::Residency;
use crate::adt::AdtTile;
use crate::billboard::BillboardCard;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::doodad_sound::{SoundHost, idle_has_sound_keys};
use crate::ground::{Ground, ground_under};
use crate::light::{LightBuffer, LightRooms, point_light};
use crate::m2::M2Model;
use crate::model::{ModelSubmesh, submesh_mesh};
use crate::model_material::{
    BatchId, BatchLook, GroundShade, ModelMaterial, ModelMaterials, Variant,
};
use crate::placements::{PlacedModel, Placement, Placements, prop_placements};
use crate::portal::{WmoGroupVis, WmoPortalInstance};
use crate::probes::{PropLobeLight, PropProbeSlot, PropProbes, fold_interior_probe};
use crate::stream::Streamer;
use crate::visibility::{DoodadFade, MatAlpha, ModelPart, alpha_bits, probe_bits};
use crate::wmo::{DoodadBase, WmoModel};

enum ModelHandle {
    M2(Handle<M2Model>),
    Wmo {
        handle: Handle<WmoModel>,
        doodad_set: u16,
        name_set: u16,
        props: Option<Vec<Prop>>,
    },
}

impl ModelHandle {
    fn id(&self) -> UntypedAssetId {
        match self {
            ModelHandle::M2(h) => h.id().untyped(),
            ModelHandle::Wmo { handle, .. } => handle.id().untyped(),
        }
    }
}

struct Prop {
    handle: Handle<M2Model>,
    transform: Transform,
    groups: Arc<[u16]>,
    light: PropLight,
}

enum PropLight {
    Exterior,
    Interior {
        ambient: [f32; 3],
        diffuse: [f32; 3],
        lights: Vec<PropLobeLight>,
    },
}

struct Furnishing {
    model: ModelHandle,
    transform: Transform,
    spawned: bool,
    entities: Vec<Entity>,
    forms: Vec<Arc<[Handle<Mesh>]>>,
}

impl Furnishing {
    fn new(p: &Placement, server: &AssetServer) -> Self {
        let model = match &p.model {
            PlacedModel::Doodad { url } => ModelHandle::M2(server.load(url)),
            PlacedModel::Building {
                url,
                doodad_set,
                name_set,
            } => ModelHandle::Wmo {
                handle: server.load(url),
                doodad_set: *doodad_set,
                name_set: *name_set,
                props: None,
            },
        };
        Self {
            model,
            transform: p.transform,
            spawned: false,
            entities: Vec::new(),
            forms: Vec::new(),
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct Furnished {
    by_id: BTreeMap<u32, Furnishing>,
    forms: HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn furnish(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    placements: Res<'_, Placements>,
    assets: (Res<'_, Assets<M2Model>>, Res<'_, Assets<WmoModel>>),
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    light: Option<Res<'_, LightBuffer>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut probes: ResMut<'_, PropProbes>,
    mut furnished: ResMut<'_, Furnished>,
    mut residency: ResMut<'_, Residency>,
) {
    let Some(light) = light else {
        return;
    };
    let ((m2s, wmos), (streamer, adts)) = (assets, ground);
    let Furnished { by_id, forms } = &mut *furnished;
    by_id.retain(|id, f| {
        let keep = placements.get(*id).is_some();
        if !keep {
            for &e in &f.entities {
                commands.entity(e).try_despawn();
            }
        }
        keep
    });
    forms.retain(|_, weak| weak.strong_count() > 0);
    for (id, p) in placements.iter() {
        by_id
            .entry(id)
            .or_insert_with(|| Furnishing::new(p, &server));
    }
    for f in by_id.values_mut() {
        if let ModelHandle::Wmo {
            handle,
            doodad_set,
            props: props @ None,
            ..
        } = &mut f.model
            && let Some(m) = wmos.get(&*handle)
        {
            *props = Some(resolve_props(m, *doodad_set, &f.transform, &server));
        }
    }
    let ready = by_id
        .values()
        .filter(|f| !f.spawned)
        .all(|f| arrived_with_props(f, &server, &wmos, &streamer, &adts));
    residency.models = ready && by_id.values().all(|f| f.spawned);
    if !ready || residency.models {
        return;
    }
    let mut spawner = Spawner {
        commands: &mut commands,
        meshes: &mut meshes,
        forms,
        cache: &mut cache,
        materials: &mut materials,
        light: &light.0,
    };
    for f in by_id.values_mut().filter(|f| !f.spawned) {
        f.spawned = true;
        match &f.model {
            ModelHandle::M2(h) => {
                let Some(m) = m2s.get(h) else {
                    continue;
                };
                let shade =
                    ground_shade(&streamer, &adts, &f.transform).unwrap_or(GroundShade::Lit);
                let (entities, form) =
                    spawner.placed_doodad(m, h.id().untyped(), &f.transform, shade);
                f.entities = entities;
                f.forms.push(form);
            }
            ModelHandle::Wmo {
                handle,
                props,
                name_set,
                ..
            } => {
                let Some(m) = wmos.get(handle) else {
                    continue;
                };
                let form = spawner.forms(handle.id().untyped(), &m.submeshes);
                let instance =
                    spawner.building(handle, *name_set, m, &form, &f.transform, &mut f.entities);
                f.forms.push(form);
                for prop in props.iter().flatten() {
                    let Some(pm) = m2s.get(&prop.handle) else {
                        continue;
                    };
                    let form = spawner.forms(prop.handle.id().untyped(), &pm.submeshes);
                    let ents =
                        spawner.prop(pm, &form, prop, instance, &streamer, &adts, &mut probes);
                    f.entities.extend(ents);
                    f.forms.push(form);
                }
            }
        }
    }
    residency.models = true;
}

fn failed(server: &AssetServer, id: UntypedAssetId) -> bool {
    matches!(server.load_state(id), LoadState::Failed(_))
}

fn arrived(server: &AssetServer, id: UntypedAssetId) -> bool {
    failed(server, id)
        || matches!(
            server.recursive_dependency_load_state(id),
            RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_)
        )
}

fn arrived_with_props(
    f: &Furnishing,
    server: &AssetServer,
    wmos: &Assets<WmoModel>,
    streamer: &Streamer,
    adts: &Assets<AdtTile>,
) -> bool {
    arrived(server, f.model.id())
        && match &f.model {
            ModelHandle::M2(_) => shade_known(streamer, adts, &f.transform),
            ModelHandle::Wmo { handle, props, .. } => {
                wmos.get(handle).is_none()
                    || props.iter().flatten().all(|p| {
                        arrived(server, p.handle.id().untyped())
                            && (matches!(p.light, PropLight::Interior { .. })
                                || shade_known(streamer, adts, &p.transform))
                    })
            }
        }
}

fn shade_known(streamer: &Streamer, adts: &Assets<AdtTile>, at: &Transform) -> bool {
    ground_shade(streamer, adts, at).is_some()
}

pub(crate) fn ground_shade(
    streamer: &Streamer,
    adts: &Assets<AdtTile>,
    at: &Transform,
) -> Option<GroundShade> {
    let pos = at.translation;
    match ground_under(streamer, adts, pos) {
        Ground::Absent => Some(GroundShade::Lit),
        Ground::Pending => None,
        Ground::Tile(adt) => {
            let shadowed = terrain::mcsh_shadowed_at(&adt.chunks, bevy_to_wow(pos));
            Some(if shadowed.unwrap_or(false) {
                GroundShade::Shadowed
            } else {
                GroundShade::Lit
            })
        }
    }
}

fn resolve_props(
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

struct Spawner<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    meshes: &'a mut Assets<Mesh>,
    forms: &'a mut HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
    cache: &'a mut ModelMaterials,
    materials: &'a mut Assets<ModelMaterial>,
    light: &'a Buffer,
}

impl Spawner<'_, '_, '_> {
    fn forms(&mut self, model: UntypedAssetId, submeshes: &[ModelSubmesh]) -> Arc<[Handle<Mesh>]> {
        if let Some(form) = self.forms.get(&model).and_then(Weak::upgrade) {
            return form;
        }
        let form: Arc<[Handle<Mesh>]> = submeshes
            .iter()
            .map(|s| self.meshes.add(submesh_mesh(&s.geometry)))
            .collect();
        self.forms.insert(model, Arc::downgrade(&form));
        form
    }

    fn placed_doodad(
        &mut self,
        m: &M2Model,
        model: UntypedAssetId,
        transform: &Transform,
        shade: GroundShade,
    ) -> (Vec<Entity>, Arc<[Handle<Mesh>]>) {
        let form = self.forms(model, &m.submeshes);
        let mut entities = self.doodad(m, model, &form, transform, DoodadLight::Sky(shade));
        let parts = entities.clone();
        entities.extend(self.sound_host(m, transform, None, parts));
        self.doodad_lights(m, transform, None, &mut entities);
        (entities, form)
    }

    fn doodad(
        &mut self,
        m: &M2Model,
        model: UntypedAssetId,
        form: &[Handle<Mesh>],
        transform: &Transform,
        light: DoodadLight,
    ) -> Vec<Entity> {
        let (radius, center) = m.fade_sphere(transform.scale.x);
        let placed = Placed {
            model,
            transform,
            is_wmo: false,
            light,
            radius,
            local_center: center,
        };
        self.batches(&m.submeshes, form, &placed)
    }

    #[allow(clippy::too_many_arguments)]
    fn prop(
        &mut self,
        m: &M2Model,
        form: &[Handle<Mesh>],
        prop: &Prop,
        instance: Entity,
        streamer: &Streamer,
        adts: &Assets<AdtTile>,
        probes: &mut PropProbes,
    ) -> Vec<Entity> {
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
                    .alloc(probe)
                    .map_or(DoodadLight::Sky(GroundShade::Lit), DoodadLight::Probe)
            }
        };
        let model = prop.handle.id().untyped();
        let mut ents = self.doodad(m, model, form, &prop.transform, light);
        let parts = ents.clone();
        if let DoodadLight::Probe(slot) = light {
            let owner = if let Some(&e) = ents.first() {
                e
            } else {
                let carrier = self.commands.spawn_empty().id();
                ents.push(carrier);
                carrier
            };
            self.commands.entity(owner).insert(PropProbeSlot(slot));
        }
        let room = (!prop.groups.is_empty()).then(|| WmoGroupVis {
            instance,
            groups: prop.groups.clone(),
        });
        if let Some(room) = &room {
            for &e in &ents {
                self.commands.entity(e).insert(room.clone());
            }
        }
        ents.extend(self.sound_host(m, &prop.transform, room.clone(), parts));
        self.doodad_lights(m, &prop.transform, room.as_ref(), &mut ents);
        ents
    }

    fn sound_host(
        &mut self,
        m: &M2Model,
        transform: &Transform,
        room: Option<WmoGroupVis>,
        parts: Vec<Entity>,
    ) -> Option<Entity> {
        let anims = m.animations.as_ref().filter(|a| idle_has_sound_keys(a))?;
        let (radius, center) = m.fade_sphere(transform.scale.x);
        let fade = Sphere {
            center: transform.transform_point(center).into(),
            radius,
        };
        let host = SoundHost::new(anims, parts, fade, room)?;
        Some(self.commands.spawn((host, anims.clone(), *transform)).id())
    }

    fn doodad_lights(
        &mut self,
        m: &M2Model,
        transform: &Transform,
        room: Option<&WmoGroupVis>,
        out: &mut Vec<Entity>,
    ) {
        for l in m.lights.iter().filter(|l| l.casts()) {
            let world = transform.transform_point(wow_to_bevy(l.position));
            let mut e = self.commands.spawn((
                point_light(l.diffuse_color, l.diffuse_intensity),
                Transform::from_translation(world),
            ));
            if let Some(room) = room {
                e.insert(LightRooms(room.clone()));
            }
            out.push(e.id());
        }
    }

    fn building(
        &mut self,
        handle: &Handle<WmoModel>,
        name_set: u16,
        m: &WmoModel,
        form: &[Handle<Mesh>],
        transform: &Transform,
        out: &mut Vec<Entity>,
    ) -> Entity {
        let instance = self
            .commands
            .spawn(WmoPortalInstance::new(
                handle.clone(),
                transform,
                m.group_nav.len(),
                name_set,
            ))
            .id();
        out.push(instance);
        let has_portals = !m.portal_refs.is_empty() && !m.portal_infos.is_empty();
        let placed = Placed {
            model: handle.id().untyped(),
            transform,
            is_wmo: true,
            light: DoodadLight::Sky(GroundShade::Lit),
            radius: f32::INFINITY,
            local_center: Vec3::ZERO,
        };
        let batches = self.batches(&m.submeshes, form, &placed);
        for (&entity, &group) in batches.iter().zip(&m.submesh_group) {
            if has_portals {
                self.commands.entity(entity).insert(WmoGroupVis {
                    instance,
                    groups: Arc::from([group].as_slice()),
                });
            }
            out.push(entity);
        }
        for (i, l) in m.lights.iter().enumerate() {
            if !l.is_omni() {
                continue;
            }
            let rooms: Arc<[u16]> = m
                .group_light_refs
                .iter()
                .enumerate()
                .filter(|(_, refs)| refs.contains(&(i as u16)))
                .map(|(g, _)| g as u16)
                .collect();
            let world = transform.transform_point(wow_to_bevy(l.position));
            let mut e = self.commands.spawn((
                point_light(l.color, l.intensity),
                Transform::from_translation(world),
            ));
            if !rooms.is_empty() {
                e.insert(LightRooms(WmoGroupVis {
                    instance,
                    groups: rooms,
                }));
            }
            out.push(e.id());
        }
        instance
    }

    fn batches(
        &mut self,
        submeshes: &[ModelSubmesh],
        form: &[Handle<Mesh>],
        placed: &Placed<'_>,
    ) -> Vec<Entity> {
        let (shade, probe) = match placed.light {
            DoodadLight::Sky(shade) => (shade, None),
            DoodadLight::Probe(slot) => (GroundShade::Lit, Some(slot)),
        };
        submeshes
            .iter()
            .zip(form)
            .enumerate()
            .map(|(i, (sub, mesh))| {
                let g = &sub.geometry;
                let steady_interior_prop = probe.is_some() && sub.billboard.is_none();
                let look = BatchLook {
                    texture: sub.texture.clone(),
                    blend: g.blend,
                    two_sided: g.two_sided,
                    is_wmo: placed.is_wmo,
                    interior: g.interior || probe.is_some(),
                    emissive: g.emissive,
                    additive: g.additive,
                    no_depth_write: g.no_depth_write,
                    no_depth_test: g.no_depth_test,
                    fog_policy: g.fog_policy,
                    env_map: g.env_map,
                    shade,
                    batch_order: Some(
                        NonZeroU16::MIN.saturating_add(u16::try_from(i).unwrap_or(u16::MAX)),
                    ),
                    uv_offset_at_rest: g.uv_anim.as_ref().map_or([0.0, 0.0], |a| a.sample(0.0)),
                    tint_at_rest: g.rgb_anim.as_ref().map_or([1.0; 3], |a| a.sample(0.0)),
                    animated: (g.uv_anim.is_some() || g.rgb_anim.is_some()).then_some(BatchId {
                        model: placed.model,
                        index: i,
                    }),
                    wmo_class: g.wmo_batch,
                    sidn: g.sidn,
                    window: g.window,
                    skybox: false,
                };
                let cutout = self
                    .cache
                    .get(self.materials, &look, Variant::Steady, self.light);
                let blended = if steady_interior_prop
                    || matches!(
                        g.blend,
                        ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
                    ) {
                    cutout.clone()
                } else {
                    self.cache
                        .get(self.materials, &look, Variant::FadeTwin, self.light)
                };
                let tag = MeshTag(match probe {
                    Some(slot) => probe_bits(slot),
                    None => alpha_bits(1.0),
                });
                let mut e = self.commands.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(cutout.clone()),
                    ModelPart,
                    tag,
                ));
                let fade_center = if let Some(info) = &sub.billboard {
                    e.insert((
                        Transform::from_translation(placed.transform.transform_point(info.pivot)),
                        BillboardCard::new(info, placed.transform),
                    ));
                    Vec3::ZERO
                } else {
                    e.insert(*placed.transform);
                    placed.local_center
                };
                if let Some(aabb) = sub.aabb {
                    e.insert((aabb, NoAutoAabb));
                }
                if let Some(anim) = &g.alpha_anim {
                    e.insert(MatAlpha(anim.sample(None, 0.0, 0.0)));
                }
                if !steady_interior_prop {
                    e.insert(DoodadFade {
                        radius: placed.radius,
                        local_center: fade_center,
                        cutout,
                        blend: blended,
                    });
                }
                e.id()
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum DoodadLight {
    Sky(GroundShade),
    Probe(u16),
}

struct Placed<'a> {
    model: UntypedAssetId,
    transform: &'a Transform,
    is_wmo: bool,
    light: DoodadLight,
    radius: f32,
    local_center: Vec3,
}
