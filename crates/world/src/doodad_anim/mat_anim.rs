use std::collections::{HashMap, HashSet};
use std::num::NonZeroU16;
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use model::{AlphaAnim, KeyAnim, SeqLoops};

use crate::mat_anim_table::MatAnimTable;
use crate::model_material::{ModelExtension, ModelMaterial};
use crate::rig::ModelAnimations;

const UV_STEPS: f32 = 4096.0;
const TINT_STEPS: f32 = 255.0;

#[derive(Component)]
pub(crate) struct MatAnim {
    anim: Arc<AlphaAnim>,
    origin: f64,
    seq_owner: Option<Entity>,
    last_seq: Option<usize>,
    pub(crate) alpha: f32,
}

impl MatAnim {
    pub(crate) fn new(anim: Arc<AlphaAnim>, origin: f64) -> Self {
        let alpha = anim.sample(None, 0.0, 0.0);
        Self {
            anim,
            origin,
            seq_owner: None,
            last_seq: None,
            alpha,
        }
    }

    pub(crate) fn following(anim: Arc<AlphaAnim>, seq_owner: Entity, origin: f64) -> Self {
        Self {
            seq_owner: Some(seq_owner),
            ..Self::new(anim, origin)
        }
    }
}

pub(super) fn sample_mat_anim(
    time: Res<'_, Time>,
    owners: SeqOwners<'_, '_>,
    mut q: Query<'_, '_, &mut MatAnim>,
) {
    let now = time.elapsed_secs_f64();
    for mut m in &mut q {
        let age = now - m.origin;
        let (seq, elapsed) = match owner_playing(&owners, m.seq_owner) {
            Some(p) => {
                m.last_seq = Some(p.seq);
                (Some(p.seq), p.clip_time)
            }
            None if m.seq_owner.is_some() => (m.last_seq, 0.0),
            None => (None, age as f32),
        };
        m.alpha = m.anim.sample(seq, elapsed, age);
    }
}

#[derive(Component)]
pub(crate) struct AnimMatPart;

#[derive(Clone)]
pub(crate) enum MatLoop<V> {
    Shared(Arc<KeyAnim<V>>),
    PerSeq {
        seqs: Arc<SeqLoops<V>>,
        seq_owner: Entity,
    },
}

impl<V> MatLoop<V> {
    fn seq_owner(&self) -> Option<Entity> {
        match self {
            MatLoop::Shared(_) => None,
            MatLoop::PerSeq { seq_owner, .. } => Some(*seq_owner),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Playing {
    seq: usize,
    clip_time: f32,
}

pub(crate) trait MaterialChannel: Copy + PartialEq + Send + Sync + 'static {
    const IDENTITY: Self;
    fn sample(anim: &KeyAnim<Self>, t: f32) -> Self;
    fn at_rest(ext: &ModelExtension) -> Self;
    fn slot(ext: &mut ModelExtension) -> &mut f32;
    fn row(self, at_rest: Self) -> [f32; 4];
}

impl MaterialChannel for [f32; 2] {
    const IDENTITY: Self = [0.0, 0.0];

    fn sample(anim: &KeyAnim<Self>, t: f32) -> Self {
        anim.sample(t)
    }

    fn at_rest(ext: &ModelExtension) -> Self {
        [ext.sun_scale.z, ext.sun_scale.w]
    }

    fn slot(ext: &mut ModelExtension) -> &mut f32 {
        &mut ext.anim_slots.x
    }

    fn row(self, at_rest: Self) -> [f32; 4] {
        [
            quantize(self[0], UV_STEPS) - at_rest[0],
            quantize(self[1], UV_STEPS) - at_rest[1],
            0.0,
            0.0,
        ]
    }
}

impl MaterialChannel for [f32; 3] {
    const IDENTITY: Self = [1.0, 1.0, 1.0];

    fn sample(anim: &KeyAnim<Self>, t: f32) -> Self {
        anim.sample(t)
    }

    fn at_rest(ext: &ModelExtension) -> Self {
        [ext.tint.x, ext.tint.y, ext.tint.z]
    }

    fn slot(ext: &mut ModelExtension) -> &mut f32 {
        &mut ext.anim_slots.y
    }

    fn row(self, at_rest: Self) -> [f32; 4] {
        [
            quantize(self[0], TINT_STEPS) - at_rest[0],
            quantize(self[1], TINT_STEPS) - at_rest[1],
            quantize(self[2], TINT_STEPS) - at_rest[2],
            0.0,
        ]
    }
}

fn quantize(x: f32, steps: f32) -> f32 {
    (x * steps).round() / steps
}

pub(crate) struct MatAnimEntry<V> {
    anim: MatLoop<V>,
    slot: NonZeroU16,
    at_rest: V,
}

impl<V: MaterialChannel> MatAnimEntry<V> {
    fn row(&self, now: f32, gseq_now: f64, playing: Option<Playing>) -> [f32; 4] {
        let value = match &self.anim {
            MatLoop::Shared(anim) => V::sample(anim, now),
            MatLoop::PerSeq { seqs, .. } => playing
                .and_then(|p| {
                    seqs.seq(Some(p.seq))
                        .map(|l| V::sample(l, l.clock(p.clip_time, gseq_now)))
                })
                .unwrap_or(V::IDENTITY),
        };
        value.row(self.at_rest)
    }
}

#[derive(Resource)]
pub(crate) struct AnimMaterials<V>(HashMap<AssetId<ModelMaterial>, MatAnimEntry<V>>);

impl<V> Default for AnimMaterials<V> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

pub(crate) type UvAnimMaterials = AnimMaterials<[f32; 2]>;
pub(crate) type TintAnimMaterials = AnimMaterials<[f32; 3]>;

pub(crate) fn register<V: MaterialChannel>(
    reg: &mut AnimMaterials<V>,
    table: &mut MatAnimTable,
    materials: &mut Assets<ModelMaterial>,
    id: AssetId<ModelMaterial>,
    anim: MatLoop<V>,
) {
    if reg.0.contains_key(&id) {
        return;
    }
    let Some(slot) = table.alloc() else {
        warn_once!("the animated-material table is full: a material loop stays still");
        return;
    };
    let Some(mat) = materials.get_mut(id) else {
        table.free(slot);
        return;
    };
    let at_rest = V::at_rest(&mat.extension);
    *V::slot(&mut mat.extension) = f32::from(slot.get());
    reg.0.insert(
        id,
        MatAnimEntry {
            anim,
            slot,
            at_rest,
        },
    );
}

fn playing_seq(player: &AnimationPlayer, anims: &ModelAnimations) -> Option<Playing> {
    player
        .playing_animations()
        .filter_map(|(node, active)| {
            let clip = anims.clips.iter().find(|c| c.node == *node)?;
            Some((clip.seq_index, active.seek_time(), active.weight()))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(seq, clip_time, _)| Playing { seq, clip_time })
        .or_else(|| {
            anims.idle_clip().map(|c| Playing {
                seq: c.seq_index,
                clip_time: 0.0,
            })
        })
}

type SeqOwners<'w, 's> = Query<'w, 's, (&'static AnimationPlayer, &'static ModelAnimations)>;

fn owner_playing(owners: &SeqOwners<'_, '_>, owner: Option<Entity>) -> Option<Playing> {
    let (player, anims) = owners.get(owner?).ok()?;
    playing_seq(player, anims)
}

fn tick<V: MaterialChannel>(
    reg: &mut AnimMaterials<V>,
    table: &mut MatAnimTable,
    materials: &Assets<ModelMaterial>,
    drawn: &HashSet<AssetId<ModelMaterial>>,
    owners: &SeqOwners<'_, '_>,
    now: f32,
) {
    reg.0.retain(|id, entry| {
        if !materials.contains(*id) {
            table.free(entry.slot);
            return false;
        }
        if drawn.contains(id) {
            let playing = owner_playing(owners, entry.anim.seq_owner());
            table.set(entry.slot, entry.row(now, f64::from(now), playing));
        }
        true
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tick_anim_materials(
    time: Res<'_, Time>,
    mut uv: ResMut<'_, UvAnimMaterials>,
    mut tint: ResMut<'_, TintAnimMaterials>,
    materials: Res<'_, Assets<ModelMaterial>>,
    mut table: ResMut<'_, MatAnimTable>,
    parts: Query<'_, '_, (&MeshMaterial3d<ModelMaterial>, &Visibility), With<AnimMatPart>>,
    owners: SeqOwners<'_, '_>,
    mut drawn: Local<'_, HashSet<AssetId<ModelMaterial>>>,
) {
    if uv.0.is_empty() && tint.0.is_empty() {
        return;
    }
    drawn.clear();
    for (mat, vis) in &parts {
        if *vis != Visibility::Hidden {
            drawn.insert(mat.id());
        }
    }
    let now = time.elapsed_secs();
    tick(&mut uv, &mut table, &materials, &drawn, &owners, now);
    tick(&mut tint, &mut table, &materials, &drawn, &owners, now);
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use std::time::Duration;

    use model::{AlphaSeq, UvAnim};

    use super::*;

    fn uv_loop() -> UvAnim {
        KeyAnim {
            period: 2.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, [0.1, 0.2]), (1.0, [0.5, 0.6]), (2.0, [0.1, 0.2])],
        }
    }

    #[test]
    fn the_built_offset_plus_the_row_is_the_quantized_sample() {
        let anim = uv_loop();
        let at_rest = anim.sample(0.0);
        let entry = MatAnimEntry {
            anim: MatLoop::Shared(Arc::new(anim.clone())),
            slot: NonZeroU16::MIN,
            at_rest,
        };
        for t in [0.0_f32, 0.35, 1.0, 1.7] {
            let d = entry.row(t, f64::from(t), None);
            let s = anim.sample(t);
            assert!(
                (at_rest[0] + d[0] - quantize(s[0], UV_STEPS)).abs() < 1e-6,
                "{t}"
            );
            assert!(
                (at_rest[1] + d[1] - quantize(s[1], UV_STEPS)).abs() < 1e-6,
                "{t}"
            );
            assert_eq!([d[2], d[3]], [0.0, 0.0]);
        }
    }

    #[test]
    fn a_per_placement_scroll_reads_its_rigs_sequence() {
        let seqs = SeqLoops::new(vec![
            None,
            Some(KeyAnim {
                period: 4.0,
                step: true,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, [0.0, 0.0]), (2.0, [0.0, 0.605])],
            }),
        ])
        .expect("slot 1 moves");
        let at_rest = [0.25, 0.5];
        let entry = MatAnimEntry {
            anim: MatLoop::PerSeq {
                seqs: Arc::new(seqs),
                seq_owner: Entity::PLACEHOLDER,
            },
            slot: NonZeroU16::MIN,
            at_rest,
        };
        let playing = |seq| {
            Some(Playing {
                seq,
                clip_time: 3.0,
            })
        };
        let d = entry.row(0.0, 0.0, playing(1));
        assert!((at_rest[1] + d[1] - 0.605).abs() < 1e-3, "{d:?}");
        let no_loop = entry.row(0.0, 0.0, playing(0));
        assert_eq!(no_loop, [-0.25, -0.5, 0.0, 0.0], "no offset at all");
        assert_eq!(entry.row(99.0, 99.0, None), no_loop);
    }

    #[test]
    fn the_tint_row_is_measured_from_the_built_tint() {
        let anim = KeyAnim {
            period: 1.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, [1.0, 0.5, 0.25]), (1.0, [0.0, 0.5, 0.25])],
        };
        let at_rest = anim.sample(0.0).map(|c| quantize(c, TINT_STEPS));
        let entry = MatAnimEntry {
            anim: MatLoop::Shared(Arc::new(anim.clone())),
            slot: NonZeroU16::MIN,
            at_rest,
        };
        let d = entry.row(0.4, 0.4, None);
        let want = anim.sample(0.4).map(|c| quantize(c, TINT_STEPS));
        for i in 0..3 {
            assert!((at_rest[i] + d[i] - want[i]).abs() < 1e-6, "channel {i}");
        }
    }

    #[test]
    fn a_units_batch_takes_the_alpha_of_the_sequence_it_plays() {
        use bevy::animation::graph::AnimationNodeIndex;

        use crate::rig::AnimClip;

        let constant = |v: f32| AlphaSeq {
            color: Some(KeyAnim {
                period: 1.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, v)],
            }),
            weight: None,
        };
        let anim = AlphaAnim::new(vec![constant(1.0), constant(0.0)]).expect("a loop");
        let clip = |anim_id, seq_index, node| AnimClip {
            anim_id,
            seq_index,
            node: AnimationNodeIndex::new(node),
            upper_node: None,
            looping: true,
            duration: 1.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
            events: Arc::from([]),
        };
        let anims = ModelAnimations {
            graph: Handle::default(),
            clips: vec![clip(0, 0, 1), clip(1, 1, 2)],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            moving_idle: None,
            pose: Arc::default(),
        };
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, sample_mat_anim);
        let mut player = AnimationPlayer::default();
        player.play(AnimationNodeIndex::new(2));
        let unit = app.world_mut().spawn((player, anims)).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::following(Arc::new(anim), unit, 0.0))
            .id();
        app.update();
        let alpha = |app: &App| app.world().get::<MatAnim>(part).expect("an alpha").alpha;
        assert_eq!(alpha(&app), 0.0, "the second sequence hides it");
        let mut player = app
            .world_mut()
            .get_mut::<AnimationPlayer>(unit)
            .expect("a player");
        player.stop_all();
        player.play(AnimationNodeIndex::new(1));
        app.update();
        assert_eq!(alpha(&app), 1.0, "the first shows it");
    }

    #[test]
    fn a_hidden_batch_s_alpha_rises_again() {
        let fade = AlphaAnim::new(vec![AlphaSeq {
            color: Some(KeyAnim {
                period: 2.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, 1.0), (1.0, 0.0), (2.0, 1.0)],
            }),
            weight: None,
        }])
        .expect("a loop");
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, sample_mat_anim);
        let part = app
            .world_mut()
            .spawn((MatAnim::new(Arc::new(fade), 0.0), Visibility::Hidden))
            .id();
        let mut alpha_after = |ms| {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_millis(ms));
            app.update();
            app.world().get::<MatAnim>(part).expect("an alpha").alpha
        };
        assert!(alpha_after(1000).abs() < 1e-6);
        assert!((alpha_after(750) - 0.75).abs() < 1e-5);
    }
}
