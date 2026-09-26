use std::collections::BTreeSet;
use std::f32::consts::TAU;

use bevy::prelude::*;
use world::coords::wow_to_bevy;
use world::sight::{Seen, Sight};

use super::command::Ask;

const CUT_RADIUS_YD: f32 = 2.0;

struct Ring {
    share_of_radius: f32,
    lines: u32,
}

const RINGS: [Ring; 3] = [
    Ring {
        share_of_radius: 0.0,
        lines: 1,
    },
    Ring {
        share_of_radius: 0.5,
        lines: 8,
    },
    Ring {
        share_of_radius: 1.0,
        lines: 16,
    },
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cut {
    pub left_out: BTreeSet<u32>,
    pub ground_hides_cut_to: bool,
}

/// The placements a shot leaves out: those the sight lines from the eye to the disc round its
/// `cut_to` point meet short of it and those whose boxes come within its `cut_near` yards of the
/// eye, but for one whose box holds the point; and every one its `leave_out` names. The eye is
/// in WoW's axes.
pub fn cut(sight: &Sight<'_, '_>, eye_wow: Vec3, ask: &Ask) -> Cut {
    let mut out = cut_between(sight, eye_wow, ask.cut_to, ask.cut_near);
    out.left_out.extend(&ask.leave_out);
    out
}

fn cut_between(sight: &Sight<'_, '_>, eye_wow: Vec3, to: Option<Vec3>, near: Option<f32>) -> Cut {
    let eye = wow_to_bevy(eye_wow.to_array());
    let placed = |seen: &Vec<Seen>| seen.iter().filter_map(Seen::placement).collect::<Vec<_>>();
    let mut out = Cut::default();
    if let Some(near) = near {
        out.left_out.extend(placed(&sight.within(eye, near)));
    }
    let Some(to) = to.map(|p| wow_to_bevy(p.to_array())) else {
        return out;
    };
    let Ok(axis) = Dir3::new(to - eye) else {
        return out;
    };
    let (across, up) = axis.any_orthonormal_pair();
    for (i, ring) in RINGS.iter().enumerate() {
        for k in 0..ring.lines {
            let angle = TAU * k as f32 / ring.lines as f32;
            let off = across * ops::cos(angle) + up * ops::sin(angle);
            let point = to + off * (CUT_RADIUS_YD * ring.share_of_radius);
            let Ok(dir) = Dir3::new(point - eye) else {
                continue;
            };
            let reach = eye.distance(point);
            let nearer = sight.cast(eye, dir, reach).nearer(reach);
            if i == 0 {
                out.ground_hides_cut_to = nearer.terrain;
            }
            out.left_out.extend(placed(&nearer.models));
        }
    }
    for held in placed(&sight.within(to, 0.0)) {
        out.left_out.remove(&held);
    }
    out
}
