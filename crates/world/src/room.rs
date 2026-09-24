use bevy::prelude::*;
use model::WmoFog;

use crate::light::Fog;

const CROSSFADE_SECS: f32 = 4.0;
const NOT_A_ROOM_FOG: u32 = 1;

#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct CameraRoom {
    pub indoors: bool,
    pub fog: Option<RoomFog>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct RoomFog {
    pub color: [f32; 3],
    pub end_yards: f32,
    pub start_fraction: f32,
}

pub(crate) fn select_room_fog(
    fogs: &[WmoFog],
    indices: [u8; 4],
    eye_local: [f32; 3],
) -> Option<RoomFog> {
    if fogs.len() < 2 {
        return None;
    }
    let mut near: Vec<(f32, &WmoFog)> = indices
        .iter()
        .filter_map(|&i| fogs.get(i as usize))
        .filter(|f| f.flags & NOT_A_ROOM_FOG == 0)
        .filter_map(|f| {
            let d = Vec3::from(eye_local).distance(Vec3::from(f.pos));
            (d <= f.radius_outer).then_some((d, f))
        })
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut acc = room_fog(&fogs[0]);
    for (d, f) in near {
        let span = f.radius_outer - f.radius_inner;
        let w = if span > 0.0 {
            (1.0 - (d - f.radius_inner) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let t = room_fog(f);
        for (a, b) in acc.color.iter_mut().zip(t.color) {
            *a += (b - *a) * w;
        }
        acc.end_yards += (t.end_yards - acc.end_yards) * w;
        acc.start_fraction += (t.start_fraction - acc.start_fraction) * w;
    }
    Some(acc)
}

fn room_fog(f: &WmoFog) -> RoomFog {
    let channel = |shift: u32| ((f.color >> shift) & 0xff) as f32 / 255.0;
    RoomFog {
        color: [channel(16), channel(8), channel(0)],
        end_yards: f.fog_end,
        start_fraction: f.fog_start_scalar,
    }
}

#[derive(Resource, Default)]
pub(crate) struct RoomCrossfade {
    weight: f32,
    held: Option<RoomFog>,
}

impl RoomCrossfade {
    pub(crate) fn weight(&self) -> f32 {
        self.weight
    }

    pub(crate) fn blend(
        &mut self,
        target: Option<RoomFog>,
        scene: Fog,
        farclip: f32,
        dt: f32,
    ) -> Fog {
        if target.is_some() {
            self.held = target;
        }
        let toward = if target.is_some() { 1.0 } else { -1.0 };
        self.weight = (self.weight + toward * dt / CROSSFADE_SECS).clamp(0.0, 1.0);
        let Some(room) = self.held else {
            return scene;
        };
        if self.weight <= 0.0 {
            self.held = None;
            return scene;
        }
        let end = room.end_yards.min(farclip);
        let start = end * room.start_fraction;
        let (mut color, k) = (scene.color, self.weight);
        for (c, r) in color.iter_mut().zip(room.color) {
            *c += (r - *c) * k;
        }
        Fog {
            color,
            start: scene.start + (start - scene.start) * k,
            end: scene.end + (end - scene.end) * k,
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn fog(flags: u32, pos: [f32; 3], inner: f32, outer: f32, end: f32, color: u32) -> WmoFog {
        WmoFog {
            flags,
            pos,
            radius_inner: inner,
            radius_outer: outer,
            fog_end: end,
            fog_start_scalar: 0.25,
            color,
            uw_fog_end: 0.0,
            uw_fog_start_scalar: 0.0,
            uw_color: 0,
        }
    }

    #[test]
    fn a_building_with_one_fog_keeps_the_scenes() {
        let fogs = [fog(0, [0.0; 3], 0.0, 0.0, 444.4, 0xffff_ffff)];
        assert!(select_room_fog(&fogs, [0; 4], [0.0; 3]).is_none());
    }

    #[test]
    fn a_flagged_fog_leaves_the_buildings_own() {
        let fogs = [
            fog(0, [0.0; 3], 0.0, 3.36, 194.4, 0xfffa_d890),
            fog(
                NOT_A_ROOM_FOG,
                [12.3, -0.6, 2.8],
                0.0,
                3.36,
                83.3,
                0xfffd_cf9e,
            ),
        ];
        let f = select_room_fog(&fogs, [1, 0, 0, 0], [12.3, -0.6, 2.8]).expect("engaged");
        assert!((f.end_yards - 194.4).abs() < 1e-3);
        assert!((f.color[0] - 250.0 / 255.0).abs() < 1e-6);
        assert!((f.color[2] - 144.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn a_near_fog_weighs_by_its_falloff() {
        let fogs = [
            fog(0, [0.0; 3], 0.0, 0.0, 200.0, 0xff00_0000),
            fog(0, [10.0, 0.0, 0.0], 5.0, 25.0, 80.0, 0xffff_ffff),
        ];
        let at = |x: f32| select_room_fog(&fogs, [1, 0, 0, 0], [x, 0.0, 0.0]).expect("engaged");
        assert!((at(10.0).end_yards - 80.0).abs() < 1e-4);
        assert!((at(25.0).end_yards - 140.0).abs() < 1e-3);
        assert!((at(40.0).end_yards - 200.0).abs() < 1e-4);
    }

    #[test]
    fn the_crossfade_takes_four_seconds_each_way_and_holds_the_room_leaving() {
        let mut fade = RoomCrossfade::default();
        let room = RoomFog {
            color: [1.0, 0.5, 0.0],
            end_yards: 194.4,
            start_fraction: 0.25,
        };
        let scene = Fog {
            color: [0.2; 3],
            start: -139.0,
            end: 278.0,
        };
        let half = fade.blend(Some(room), scene, 1100.0, 2.0);
        assert_eq!(fade.weight, 0.5);
        assert!((half.end - (278.0 + (194.4 - 278.0) * 0.5)).abs() < 1e-3);
        let full = fade.blend(Some(room), scene, 1100.0, 2.0);
        assert!((full.end - 194.4).abs() < 1e-3 && (full.start - 194.4 * 0.25).abs() < 1e-3);
        assert_eq!(full.color[0], 1.0);
        let leaving = fade.blend(None, scene, 1100.0, 2.0);
        assert!((leaving.end - (278.0 + (194.4 - 278.0) * 0.5)).abs() < 1e-3);
        assert_eq!(fade.blend(None, scene, 1100.0, 2.0), scene);
        assert!(fade.held.is_none());
    }
}
