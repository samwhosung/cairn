use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::stream::Streamer;

pub(crate) enum Ground<'a> {
    Absent,
    Pending,
    Tile(&'a AdtTile),
}

pub(crate) fn ground_under<'a>(
    streamer: &Streamer,
    adts: &'a Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Ground<'a> {
    let [x, y, _] = bevy_to_wow(bevy_pos);
    let tile = wdt::world_to_tile(x, y);
    match streamer.pending_or_arrived(tile) {
        None => Ground::Absent,
        Some(handle) => adts.get(handle).map_or(Ground::Pending, Ground::Tile),
    }
}

pub(crate) fn terrain_wow_z_under(
    streamer: &Streamer,
    adts: &Assets<AdtTile>,
    bevy_pos: Vec3,
) -> Option<f32> {
    match ground_under(streamer, adts, bevy_pos) {
        Ground::Tile(adt) => terrain::terrain_height_at(&adt.chunks, bevy_to_wow(bevy_pos)),
        _ => None,
    }
}
