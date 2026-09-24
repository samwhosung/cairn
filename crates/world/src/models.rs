use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU16;
use std::sync::{Arc, Weak};

use bevy::asset::{LoadState, RecursiveDependencyLoadState, UntypedAssetId};
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use crate::Residency;
use crate::adt::AdtTile;
use crate::billboard::BillboardCard;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::doodad_anim::{MaterialLoops, RigBuilder};
use crate::ground::{Ground, ground_under};
use crate::light::{LightBuffer, LightRooms, point_light};
use crate::liquid::{LiquidAssets, spawn_wmo_liquids};
use crate::m2::M2Model;
use crate::model::{ModelSubmesh, skinned_submesh_mesh, submesh_mesh};
use crate::model_material::{
    BatchId, BatchLook, GroundShade, ModelMaterial, ModelMaterials, Variant,
};
use crate::particles::{DrawSetGate, EmitClock, EmitterFrames, OwnerLoss, spawn_emitter};
use crate::placements::{PlacedModel, Placement, Placements, prop_placements};
use crate::portal::{WmoGroupVis, WmoPortalInstance};
use crate::probes::{ProbeSlot, Probes, PropLobeLight, fold_interior_probe};
use crate::ribbons::{RibbonSeq, spawn_ribbon};
use crate::stream::Streamer;
use crate::visibility::{DoodadFade, ModelPart, alpha_bits, probe_bits};
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
    skinned: HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn furnish(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    placements: Res<'_, Placements>,
    assets: (Res<'_, Assets<M2Model>>, Res<'_, Assets<WmoModel>>),
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    light: Option<Res<'_, LightBuffer>>,
    liquids: Option<Res<'_, LiquidAssets>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut probes: ResMut<'_, Probes>,
    mut furnished: ResMut<'_, Furnished>,
    mut residency: ResMut<'_, Residency>,
    mut loops: MaterialLoops<'_>,
) {
    let Some(light) = light else {
        return;
    };
    let ((m2s, wmos), (streamer, adts)) = (assets, ground);
    let Furnished {
        by_id,
        forms,
        skinned,
    } = &mut *furnished;
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
    skinned.retain(|_, weak| weak.strong_count() > 0);
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
    let now = loops.now();
    let mut spawner = Spawner {
        commands: &mut commands,
        meshes: &mut meshes,
        forms,
        skinned,
        cache: &mut cache,
        materials: &mut materials,
        light: &light.0,
        loops: &mut loops,
        now,
        liquids: liquids.as_deref(),
    };
    for f in by_id.values_mut().filter(|f| !f.spawned) {
        f.spawned = true;
        spawner.furnishing(f, (&m2s, &wmos), (&streamer, &adts), &mut probes);
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

struct Spawner<'a, 'w, 's, 'l> {
    commands: &'a mut Commands<'w, 's>,
    meshes: &'a mut Assets<Mesh>,
    forms: &'a mut HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
    skinned: &'a mut HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
    cache: &'a mut ModelMaterials,
    materials: &'a mut Assets<ModelMaterial>,
    light: &'a Buffer,
    loops: &'a mut MaterialLoops<'l>,
    now: f32,
    liquids: Option<&'a LiquidAssets>,
}

struct PropSite<'a> {
    building: Entity,
    streamer: &'a Streamer,
    adts: &'a Assets<AdtTile>,
}

struct PlacedDoodad {
    batches: Vec<Entity>,
    rig_root: Option<Entity>,
    effects: Vec<Entity>,
}

fn cached_form(
    cache: &mut HashMap<UntypedAssetId, Weak<[Handle<Mesh>]>>,
    meshes: &mut Assets<Mesh>,
    model: UntypedAssetId,
    build: impl Fn(&mut Assets<Mesh>) -> Arc<[Handle<Mesh>]>,
) -> Arc<[Handle<Mesh>]> {
    if let Some(form) = cache.get(&model).and_then(Weak::upgrade) {
        return form;
    }
    let form = build(meshes);
    cache.insert(model, Arc::downgrade(&form));
    form
}

impl Spawner<'_, '_, '_, '_> {
    fn furnishing(
        &mut self,
        f: &mut Furnishing,
        (m2s, wmos): (&Assets<M2Model>, &Assets<WmoModel>),
        (streamer, adts): (&Streamer, &Assets<AdtTile>),
        probes: &mut Probes,
    ) {
        match &f.model {
            ModelHandle::M2(h) => {
                let Some(m) = m2s.get(h) else {
                    return;
                };
                let shade = ground_shade(streamer, adts, &f.transform).unwrap_or(GroundShade::Lit);
                let id = h.id().untyped();
                let form = self.forms(id, &m.submeshes);
                let light = DoodadLight::Sky(shade);
                let placed =
                    self.doodad(m, id, &form, &f.transform, light, None, None, &mut f.forms);
                let mut ents = placed.batches;
                self.doodad_lights(m, &f.transform, None, &mut ents);
                ents.extend(placed.rig_root);
                ents.extend(placed.effects);
                f.entities = ents;
                f.forms.push(form);
            }
            ModelHandle::Wmo {
                handle,
                props,
                name_set,
                ..
            } => {
                let Some(m) = wmos.get(handle) else {
                    return;
                };
                let form = self.forms(handle.id().untyped(), &m.submeshes);
                let building =
                    self.building(handle, *name_set, m, &form, &f.transform, &mut f.entities);
                f.forms.push(form);
                let site = PropSite {
                    building,
                    streamer,
                    adts,
                };
                for prop in props.iter().flatten() {
                    let Some(pm) = m2s.get(&prop.handle) else {
                        continue;
                    };
                    let form = self.forms(prop.handle.id().untyped(), &pm.submeshes);
                    let ents = self.prop(pm, &form, prop, &site, probes, &mut f.forms);
                    f.entities.extend(ents);
                    f.forms.push(form);
                }
            }
        }
    }

    fn forms(&mut self, model: UntypedAssetId, submeshes: &[ModelSubmesh]) -> Arc<[Handle<Mesh>]> {
        cached_form(self.forms, self.meshes, model, |meshes| {
            submeshes
                .iter()
                .map(|s| meshes.add(submesh_mesh(&s.geometry)))
                .collect()
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn doodad(
        &mut self,
        m: &M2Model,
        model: UntypedAssetId,
        form: &[Handle<Mesh>],
        transform: &Transform,
        light: DoodadLight,
        building: Option<Entity>,
        room: Option<&WmoGroupVis>,
        placement_forms: &mut Vec<Arc<[Handle<Mesh>]>>,
    ) -> PlacedDoodad {
        let (radius, center) = m.fade_sphere(transform.scale.x);
        let fade = DrawSetGate {
            building,
            room: room.cloned(),
            ..DrawSetGate::sphere(radius, transform.transform_point(center))
        };
        let (skinned, meshes) = (&mut *self.skinned, &mut *self.meshes);
        let mut rig = RigBuilder::spawn(
            self.commands,
            m,
            transform,
            || {
                let form = cached_form(skinned, meshes, model, |meshes| {
                    m.submeshes
                        .iter()
                        .map(|s| meshes.add(skinned_submesh_mesh(&s.geometry)))
                        .collect()
                });
                placement_forms.push(form.clone());
                form
            },
            fade.clone(),
            self.now,
        );
        let placed = Placed {
            model,
            transform,
            is_wmo: false,
            light,
            radius,
            local_center: center,
        };
        let batches = self.batches(&m.submeshes, form, &placed, rig.as_mut());
        let effects = self.spawn_effects(m, transform, &fade, rig.as_mut());
        let rig_root = rig.map(|r| r.finish(self.commands));
        PlacedDoodad {
            batches,
            rig_root,
            effects,
        }
    }

    fn spawn_effects(
        &mut self,
        m: &M2Model,
        transform: &Transform,
        fade: &DrawSetGate,
        mut rig: Option<&mut RigBuilder>,
    ) -> Vec<Entity> {
        let clock = rig.as_ref().and_then(|r| r.sequence_player());
        let mut out = Vec::new();
        for em in &m.emitters {
            let owner = rig
                .as_deref_mut()
                .and_then(|r| r.bone_anchor(self.commands, em.def.bone))
                .map(|a| (a, em.bone_pivot));
            let frames = EmitterFrames {
                owner,
                on_owner_loss: OwnerLoss::Free,
                ..EmitterFrames::default()
            };
            let clock = clock.map_or(EmitClock::Pinned, EmitClock::Host);
            if let Some(e) = spawn_emitter(self.commands, em, *transform, frames, clock) {
                self.commands.entity(e).insert(fade.clone());
                out.push(e);
            }
        }
        let mut carrier = None;
        for rb in &m.ribbons {
            let anchor = rig
                .as_deref_mut()
                .and_then(|r| r.bone_anchor(self.commands, rb.def.bone));
            let (owner, use_pivot) = if let Some(a) = anchor {
                (a, true)
            } else {
                let c = *carrier.get_or_insert_with(|| self.commands.spawn(*transform).id());
                (c, false)
            };
            if let Some(e) = spawn_ribbon(
                self.commands,
                rb,
                owner,
                use_pivot,
                transform.scale.max_element(),
                clock.map_or(RibbonSeq::Fixed(0), RibbonSeq::Host),
                None,
                Some(fade.clone()),
            ) {
                out.push(e);
            }
        }
        out.extend(carrier);
        out
    }

    fn prop(
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
                m.rooms.group_nav.len(),
                name_set,
            ))
            .id();
        out.push(instance);
        let has_portals = m.rooms.has_portals();
        let placed = Placed {
            model: handle.id().untyped(),
            transform,
            is_wmo: true,
            light: DoodadLight::Sky(GroundShade::Lit),
            radius: f32::INFINITY,
            local_center: Vec3::ZERO,
        };
        let batches = self.batches(&m.submeshes, form, &placed, None);
        for (&entity, &group) in batches.iter().zip(&m.submesh_group) {
            if has_portals {
                self.commands.entity(entity).insert(WmoGroupVis {
                    instance,
                    groups: Arc::from([group].as_slice()),
                });
            }
            out.push(entity);
        }
        if let Some(liquids) = self.liquids {
            out.extend(spawn_wmo_liquids(
                self.commands,
                self.meshes,
                liquids,
                &m.rooms,
                &m.material_diff_colors,
                *transform,
                instance,
            ));
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
        mut rig: Option<&mut RigBuilder>,
    ) -> Vec<Entity> {
        let (shade, probe) = match placed.light {
            DoodadLight::Sky(shade) => (shade, None),
            DoodadLight::Probe(slot) => (GroundShade::Lit, Some(slot)),
        };
        let mut out = Vec::with_capacity(submeshes.len());
        for (i, (sub, mesh)) in submeshes.iter().zip(form).enumerate() {
            let g = &sub.geometry;
            let steady_interior_prop = probe.is_some() && sub.billboard.is_none();
            let seq_owner = rig
                .as_deref()
                .filter(|_| g.uv_seq.is_some() || g.rgb_seq.is_some())
                .map(RigBuilder::root);
            let look = batch_look(sub, i, placed, shade, probe.is_some(), seq_owner);
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
            self.loops
                .register(self.materials, cutout.id(), blended.id(), g, seq_owner);
            let tag = MeshTag(match probe {
                Some(slot) => probe_bits(slot),
                None => alpha_bits(1.0),
            });
            let card = sub.billboard.as_ref().map(|info| {
                rig.as_deref_mut()
                    .and_then(|r| r.card(self.commands, info))
                    .unwrap_or_else(|| BillboardCard::new(info, placed.transform))
            });
            let mut e = self.commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(cutout.clone()),
                ModelPart,
                tag,
            ));
            let fade_center = if let (Some(info), Some(card)) = (&sub.billboard, card) {
                e.insert((
                    Transform::from_translation(placed.transform.transform_point(info.pivot)),
                    card,
                ));
                if let Some(aabb) = sub.aabb {
                    e.insert((aabb, NoAutoAabb));
                }
                Vec3::ZERO
            } else {
                e.insert(*placed.transform);
                match rig.as_deref_mut() {
                    Some(r) => r.add_batch(&mut e, i, mesh, sub.aabb),
                    None => {
                        if let Some(aabb) = sub.aabb {
                            e.insert((aabb, NoAutoAabb));
                        }
                    }
                }
                placed.local_center
            };
            self.loops.loops_for(g, seq_owner).insert(&mut e);
            if !steady_interior_prop {
                e.insert(DoodadFade {
                    radius: placed.radius,
                    local_center: fade_center,
                    cutout,
                    blend: blended,
                });
            }
            out.push(e.id());
        }
        out
    }
}

fn batch_look(
    sub: &ModelSubmesh,
    i: usize,
    placed: &Placed<'_>,
    shade: GroundShade,
    probe_lit: bool,
    seq_owner: Option<Entity>,
) -> BatchLook {
    let g = &sub.geometry;
    BatchLook {
        texture: sub.texture.clone(),
        blend: g.blend,
        two_sided: g.two_sided,
        is_wmo: placed.is_wmo,
        interior: g.interior || probe_lit,
        emissive: g.emissive,
        additive: g.additive,
        no_depth_write: g.no_depth_write,
        no_depth_test: g.no_depth_test,
        fog_policy: g.fog_policy,
        env_map: g.env_map,
        shade,
        batch_order: Some(NonZeroU16::MIN.saturating_add(u16::try_from(i).unwrap_or(u16::MAX))),
        uv_offset_at_rest: g.uv_anim.as_ref().map_or([0.0, 0.0], |a| a.sample(0.0)),
        tint_at_rest: g.rgb_anim.as_ref().map_or([1.0; 3], |a| a.sample(0.0)),
        animated: (g.uv_anim.is_some() || g.rgb_anim.is_some()).then_some(BatchId {
            model: placed.model,
            index: i,
        }),
        seq_owner,
        wmo_class: g.wmo_batch,
        sidn: g.sidn,
        window: g.window,
        skybox: false,
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
