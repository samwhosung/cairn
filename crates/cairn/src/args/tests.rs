use super::*;

fn parsed(line: &str) -> Result<Args, String> {
    parse(line.split_whitespace().map(str::to_owned))
}

#[test]
fn a_bare_command_walks_northshire() {
    let args = parsed("").expect("parses");
    let alone = Joining {
        how: Join::Alone,
        name: "Human".into(),
        game: None,
        world: None,
    };
    assert_eq!(args.mode, Mode::Window(alone));
    assert!(!args.start_flying);
    assert_eq!(args.pose.target, HUMAN_START);
    assert_eq!(args.size, DEFAULT_SIZE);
    assert_eq!(args.pose, Pose::orbit(HUMAN_START, 0.0, 12.0, 16.0));
    assert_eq!((args.map.as_str(), args.time.minute), ("Azeroth", 720));
}

#[test]
fn the_map_and_the_hour_are_taken_as_given() {
    let args = parsed("shot --map 1 --time 06:30 --out a.png").expect("parses");
    assert_eq!((args.map.as_str(), args.time.minute), ("1", 390));
    let args = parsed("--time 23:59 --map Kalimdor").expect("parses");
    assert_eq!((args.map.as_str(), args.time.minute), ("Kalimdor", 1439));
}

#[test]
fn a_shot_takes_either_camera_form() {
    let orbit = parsed("shot --at 1,2,3 --az 90 --el 30 --dist 10 --out a/b.png --size 64x32")
        .expect("parses");
    assert_eq!(
        orbit.pose,
        Pose::orbit(Vec3::new(1.0, 2.0, 3.0), 90.0, 30.0, 10.0)
    );
    assert_eq!(orbit.size, UVec2::new(64, 32));
    assert_eq!(orbit.mode, Mode::Shot(PathBuf::from("a/b.png")));
    let look = parsed("shot --out x.PNG --look 1,0,0 --eye 0,0,0").expect("parses");
    assert_eq!(look.pose, Pose::look(Vec3::ZERO, Vec3::X));
}

#[test]
fn the_glow_is_on_unless_left_out() {
    assert!(parsed("").expect("parses").glow);
    assert!(!parsed("--no-glow --map 1").expect("parses").glow);
    assert!(!parsed("shot --no-glow --out a.png").expect("parses").glow);
    assert!(parsed("--no-glow --no-glow").is_err());
}

#[test]
fn the_window_can_start_flying() {
    let args = parsed("--fly --map 1").expect("parses");
    assert!(args.start_flying && matches!(args.mode, Mode::Window(_)));
    assert!(parsed("--fly --fly").is_err());
    assert!(parsed("shot --fly --out a.png").is_err());
}

#[test]
fn the_window_can_be_muted() {
    assert!(!parsed("").expect("parses").mute);
    assert!(parsed("--mute --fly").expect("parses").mute);
    assert!(parsed("--mute --mute").is_err());
    assert!(parsed("shot --mute --out a.png").is_err());
}

#[test]
fn the_walker_is_a_human_male_unless_told() {
    assert_eq!(parsed("").expect("parses").look, Look::default());
    let args = parsed(
        "--race Tauren --sex female --skin 3 --hair 2 --hair-color 1 --face 4 --facial-hair 5",
    )
    .expect("parses");
    assert_eq!(
        args.look,
        Look {
            race: 6,
            sex: 1,
            skin: 3,
            face: 4,
            hair: 2,
            hair_color: 1,
            facial_hair: 5,
        }
    );
    assert_eq!(args.look.race_name(), "tauren");
    assert_eq!(parsed("--race 8 --sex 0").expect("parses").look.race, 8);
}

#[test]
fn a_window_joins_alone_by_address_or_hosting_on_a_port_as_its_race_unless_named() {
    let joining = |line: &str| match parsed(line).expect("parses").mode {
        Mode::Window(joining) => Some(joining),
        Mode::Shot(_) => None,
    };
    let as_ = |how, name: &str| {
        Some(Joining {
            how,
            name: name.into(),
            game: None,
            world: None,
        })
    };
    assert_eq!(joining(""), as_(Join::Alone, "Human"));
    assert_eq!(joining("shot --out a.png"), None);
    let addr = SocketAddr::from(([127, 0, 0, 1], 7000));
    assert_eq!(
        joining("--connect 127.0.0.1:7000 --race orc"),
        as_(Join::Connect(addr), "Orc")
    );
    assert_eq!(
        joining("--host --name Brother"),
        as_(Join::Host(DEFAULT_PORT), "Brother")
    );
    assert_eq!(
        joining("--race 7 --host 7100"),
        as_(Join::Host(7100), "Gnome")
    );
}

#[test]
fn a_window_that_serves_itself_may_run_a_game_on_knobs_it_names() {
    let game = |line: &str| match parsed(line).expect("parses").mode {
        Mode::Window(joining) => joining.game,
        Mode::Shot(_) => None,
    };
    assert_eq!(game(""), None);
    assert_eq!(
        game("--host --game melee --overlay a.knobs"),
        Some(GameChoice {
            name: "melee".into(),
            knobs: None,
            overlay: Some(PathBuf::from("a.knobs")),
        })
    );
    for wrong in [
        "--connect 127.0.0.1:7000 --game melee",
        "--knobs a.knobs",
        "shot --game melee --out a.png",
    ] {
        assert!(parsed(wrong).is_err(), "{wrong}");
    }
}

#[test]
fn a_window_keeps_its_own_world_where_it_is_told() {
    let world = |line: &str| match parsed(line).expect("parses").mode {
        Mode::Window(joining) => joining.world,
        Mode::Shot(_) => None,
    };
    assert_eq!(world("--host"), None, "the default is the binary's to give");
    assert_eq!(world("--world a.sqlite"), Some(PathBuf::from("a.sqlite")));
    assert_eq!(
        world("--host --game melee --world b.sqlite"),
        Some(PathBuf::from("b.sqlite"))
    );
    for wrong in [
        "--connect 127.0.0.1:7000 --world a.sqlite",
        "shot --world a.sqlite --out a.png",
    ] {
        assert!(parsed(wrong).is_err(), "{wrong}");
    }
}

#[test]
fn the_window_leaves_its_notes_where_it_is_told() {
    assert_eq!(parsed("").expect("parses").notes, None);
    let notes = parsed("--notes a/b --fly").expect("parses").notes;
    assert_eq!(notes, Some(PathBuf::from("a/b")));
    assert!(parsed("shot --notes a/b --out a.png").is_err());
}

#[test]
fn the_window_and_the_shot_alike_read_through_a_patch_directory() {
    assert_eq!(parsed("").expect("parses").patch, None);
    for line in ["--patch a/b --fly", "shot --patch a/b --out a.png"] {
        let patch = parsed(line).expect("parses").patch;
        assert_eq!(patch, Some(PathBuf::from("a/b")), "{line}");
    }
    assert!(parsed("--patch a --patch b").is_err());
}

#[test]
fn a_shot_ages_its_world_two_and_a_half_seconds_unless_told() {
    let age = |line: &str| parsed(line).expect("parses").world_age;
    assert_eq!(age("shot --out a.png"), Duration::from_millis(2500));
    assert_eq!(age("shot --age 0 --out a.png"), Duration::ZERO);
    assert_eq!(age("shot --age 4 --out a.png"), Duration::from_secs(4));
}

#[test]
fn a_display_shot_takes_its_subject_and_orbit() {
    let args = parsed("shot --display 3167 --age 2.5 --out a.png").expect("parses");
    assert_eq!(
        args.display,
        Some(Fixture {
            display: 3167,
            age: 2.5,
            scale: 1.0,
            at: NORTHSHIRE_HILLSIDE,
            az_deg: 0.0,
            el_deg: 10.0,
            dist: 5.0,
        })
    );
    let args =
        parsed("shot --display 10913 --scale 1.35 --at 1,2,3 --az 90 --el 20 --dist 7 --out a.png")
            .expect("parses");
    let f = args.display.expect("a display");
    assert_eq!(
        (f.age, f.scale, f.at, f.az_deg, f.el_deg, f.dist),
        (2.5, 1.35, Vec3::new(1.0, 2.0, 3.0), 90.0, 20.0, 7.0)
    );
}

#[test]
fn mistakes_are_refused() {
    for line in [
        "shot",
        "--out a.png",
        "shot --out a.jpg",
        "--eye 0,0,0",
        "--eye 0,0,0 --look 1,0,0 --az 3",
        "--eye 1,2,3 --look 1,2,3",
        "--eye 0,0 --look 1,0,0",
        "--at 0,0,0 --az 0 --el 91 --dist 5",
        "--at 0,0,0 --az 0 --el 10 --dist 0",
        "--at 0,0,0 --az north --el 10 --dist 5",
        "--size 1600",
        "--size 0x900",
        "--size 1600x900 --size 800x600",
        "--fov 90",
        "--eye",
        "--time 24:00",
        "--time 12:60",
        "--time 1230",
        "--time noon",
        "--race elf",
        "--race 9",
        "--sex other",
        "--hair -1",
        "--skin 256",
        "shot --race orc --out a.png",
        "--display 3167",
        "--age 2",
        "shot --age -1 --out a.png",
        "shot --display x --out a.png",
        "shot --display 1 --age -1 --out a.png",
        "shot --scale 2 --out a.png",
        "shot --display 1 --scale 0 --out a.png",
        "shot --display 1 --at 0,0,0 --out a.png",
        "shot --display 1 --at 0,0,0 --az 0 --el 10 --dist 0 --out a.png",
        "--connect nowhere",
        "--connect 127.0.0.1:7000 --host",
        "--host 70000",
        "--host --host",
        "--name Anna",
        "shot --name Anna --out a.png",
        "shot --host --out a.png",
        "shot --connect 127.0.0.1:7000 --out a.png",
    ] {
        assert!(parsed(line).is_err(), "{line}");
    }
}
