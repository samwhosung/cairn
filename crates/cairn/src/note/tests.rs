use std::sync::Arc;

use bevy::render::render_resource::TextureFormat;

use super::*;

const SEPT_25_2026: u64 = 20_721;

#[test]
fn a_note_is_named_by_its_time_in_utc() {
    let day = |d: u64| UtcTime::at(Duration::from_secs(d * 86_400)).stamp();
    assert_eq!(day(0), "1970-01-01 00:00:00");
    assert_eq!(day(11_016), "2000-02-29 00:00:00");
    assert_eq!(day(11_017), "2000-03-01 00:00:00");
    assert_eq!(day(47_541), "2100-03-01 00:00:00");
    let taken = Duration::from_secs(SEPT_25_2026 * 86_400 + 17 * 3600 + 12 * 60 + 9);
    assert_eq!(UtcTime::at(taken).dir_name(), "2026-09-25T17-12-09Z");
    assert_eq!(UtcTime::at(taken).stamp(), "2026-09-25 17:12:09");
}

#[test]
fn two_notes_in_one_second_keep_both() {
    let root = std::env::temp_dir().join(format!("cairn-notes-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let name = "2026-09-25T17-12-09Z";
    let first = make_unique_dir(&root, name).expect("the first");
    let second = make_unique_dir(&root, name).expect("the second");
    assert_eq!(first, root.join("2026-09-25T17-12-09Z"));
    assert_eq!(second, root.join("2026-09-25T17-12-09Z-2"));
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[test]
fn the_look_point_is_where_the_sight_line_passes_the_body_within_its_bounds() {
    let (eye, forward) = (Vec3::ZERO, Dir3::NEG_Z);
    let at = |feet: Vec3, blocked| look_nearest_the_feet(eye, forward, feet, blocked);
    let ahead = |yd: f32| Vec3::new(0.0, 0.0, -yd);
    assert_eq!(at(Vec3::new(3.0, -2.0, -12.0), None), ahead(12.0));
    assert_eq!(at(ahead(1.0), None), ahead(AIMS_TRUE_FROM), "too near");
    assert_eq!(at(ahead(-40.0), None), ahead(AIMS_TRUE_FROM), "behind");
    assert_eq!(at(ahead(90.0), None), ahead(STANDS_WITHIN), "too far");
    assert_eq!(at(ahead(12.0), Some(8.0)), ahead(7.0), "a wall first");
}

const LOOK: [f32; 3] = [-9436.093, 48.994, 83.627];

fn facts() -> Facts {
    Facts {
        taken: Duration::from_secs(SEPT_25_2026 * 86_400 + 3),
        map: "Azeroth".into(),
        map_id: 0,
        minute: 14 * 60 + 5,
        glow: false,
        frame_px: UVec2::new(3200, 1800),
        window_points: UVec2::new(1600, 900),
        eye_wow: [-9447.402, 58.318, 86.845],
        flying: false,
        feet_wow: [-9436.1, 49.0, 81.6],
        heading: -std::f32::consts::FRAC_PI_2,
        spot: UVec2::new(812, 395),
    }
}

#[test]
fn a_note_says_what_the_ray_met_and_how_to_see_it_again() {
    let hit = Sighting {
        point: world::coords::wow_to_bevy([-9431.5, 43.25, 84.5]),
        distance: 17.0,
        seen: Seen::Doodad {
            file: Arc::from("world/lamp.m2"),
            unique_id: 12_345,
        },
        tile: (32, 48),
    };
    let met = Met {
        hit,
        from_eye: 17.25,
    };
    let text = facts().text(LOOK, Some(&met));
    eprintln!("{text}");
    for line in [
        "note: 2026-09-25 00:00:03 UTC",
        "map: Azeroth (0) at 14:05",
        "frame: frame.png, 3200x1800, the spot ringed",
        "camera: --eye -9447.402,58.318,86.845 --look -9436.093,48.994,83.627",
        "player: walking, feet at -9436.1,49,81.6, facing 270.0 degrees from north toward west",
        "spot: pixel 812,395 from the top left",
        "met: doodad, unique id 12345, world/lamp.m2",
        "at: -9431.5,43.25,84.5, 17.25 yd from the eye, over world/maps/azeroth/azeroth_32_48.adt",
        "see it: cairn shot --map Azeroth --time 14:05 --no-glow --eye -9447.402,58.318,86.845 \
         --look -9436.093,48.994,83.627 --size 3200x1800 --out view.png",
        "walk there: cairn --map Azeroth --time 14:05 --no-glow --eye -9447.402,58.318,86.845 \
         --look -9436.093,48.994,83.627 --size 1600x900",
    ] {
        assert!(text.lines().any(|l| l == line), "no line {line}");
    }
    let nothing = facts().text(LOOK, None);
    assert!(nothing.contains("\nmet: nothing within the far clip, 350 yd of view depth\n"));
    assert!(!nothing.contains("\nat: "));
}

#[test]
fn the_flags_a_note_gives_are_taken_by_the_shot_and_the_window_alike() {
    let text = facts().text(LOOK, None);
    let flags = |prefix: &str| {
        let line = text.lines().find_map(|l| l.strip_prefix(prefix));
        crate::args::parse(line.expect(prefix).split_whitespace().map(str::to_owned))
    };
    let shot = flags("see it: cairn ").expect("the shot takes them");
    let window = flags("walk there: cairn ").expect("the window takes them");
    assert_eq!(shot.pose, window.pose);
    let [eye, look] = [facts().eye_wow, LOOK].map(Vec3::from_array);
    assert_eq!((shot.pose.eye, shot.pose.target), (eye, look));
    assert_eq!(
        (shot.size, window.size),
        (UVec2::new(3200, 1800), UVec2::new(1600, 900))
    );
    assert!(!shot.glow && !window.glow);
}

#[test]
fn the_ring_marks_round_the_spot_and_not_on_it() {
    let mut frame = Image::new_fill(
        bevy::render::render_resource::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        &[10, 20, 30, 255],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::MAIN_WORLD,
    );
    let spot = UVec2::new(60, 5);
    ring(&mut frame, spot);
    let at = |x: u32, y: u32| frame.get_color_at(x, y).expect("in the frame").to_srgba();
    let untouched = Color::srgb_u8(10, 20, 30).to_srgba();
    assert_eq!(at(spot.x, spot.y), untouched, "the spot itself");
    assert_eq!(at(spot.x - 8, spot.y), MAGENTA.to_srgba(), "on the ring");
    assert_eq!(at(spot.x - 10, spot.y), Color::BLACK.to_srgba(), "its edge");
    assert_eq!(at(spot.x - 4, spot.y), untouched, "inside it");
}
