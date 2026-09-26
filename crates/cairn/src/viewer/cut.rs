use std::collections::BTreeSet;
use std::f32::consts::TAU;

use bevy::prelude::*;
use world::coords::wow_to_bevy;
use world::sight::{Seen, Sight};

use super::command::Ask;

/// Yards round the point a shot is cut to that its sight lines clear.
const CUT_RADIUS: f32 = 2.0;
/// Rings of sight lines, each a share of [`CUT_RADIUS`] out and a number of lines round it.
const RINGS: [(f32, u32); 3] = [(0.0, 1), (0.5, 8), (1.0, 16)];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cut {
    pub left_out: BTreeSet<u32>,
    /// The ground stands between the eye and the point.
    pub ground_hides: bool,
}

/// The placements to leave out: those the sight lines from the eye to the disc round `to` meet
/// short of it, and those whose boxes come within `near` yards of the eye; never one whose box
/// holds `to`. Positions are WoW's.
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
    for (ring, (share, lines)) in RINGS.into_iter().enumerate() {
        for k in 0..lines {
            let angle = TAU * k as f32 / lines as f32;
            let off = across * ops::cos(angle) + up * ops::sin(angle);
            let point = to + off * (CUT_RADIUS * share);
            let Ok(dir) = Dir3::new(point - eye) else {
                continue;
            };
            let reach = eye.distance(point);
            let nearer = sight.cast(eye, dir, reach).nearer(reach);
            if ring == 0 {
                out.ground_hides = nearer.terrain;
            }
            out.left_out.extend(placed(&nearer.models));
        }
    }
    for held in placed(&sight.within(to, 0.0)) {
        out.left_out.remove(&held);
    }
    out
}
