use super::*;

#[test]
fn a_camera_is_given_as_the_shot_takes_it() {
    assert_eq!(
        parse("look --eye 1,2,3 --look 4,5,6"),
        Ok(Command::Look(Aim::Look {
            eye: Vec3::new(1.0, 2.0, 3.0),
            at: Vec3::new(4.0, 5.0, 6.0),
        }))
    );
    assert_eq!(
        parse("look --at 1,2,3 --az 90 --el 30 --dist 10"),
        Ok(Command::Look(Aim::Orbit {
            at: Vec3::new(1.0, 2.0, 3.0),
            az_deg: 90.0,
            el_deg: 30.0,
            dist: 10.0,
        }))
    );
}

#[test]
fn moves_turns_and_swings_sum_their_ways() {
    assert_eq!(
        parse("move forward 10 right 2.5 down 1"),
        Ok(Command::Move {
            forward: 10.0,
            left: -2.5,
            up: -1.0
        })
    );
    assert_eq!(
        parse("turn right 30"),
        Ok(Command::Turn {
            left: -30.0,
            up: 0.0
        })
    );
    assert_eq!(
        parse("orbit left 45 up 10 out 5"),
        Ok(Command::Orbit {
            left: 45.0,
            up: 10.0,
            closer: -5.0
        })
    );
}

#[test]
fn a_shot_takes_its_file_size_cuts_and_list() {
    assert_eq!(
        parse(
            "shot 'a b/c.PNG' --size 640x360 --cut-to 1,2,3 --cut-near 4 --leave-out 7,12 --seen"
        ),
        Ok(Command::Shot(Ask {
            out: PathBuf::from("a b/c.PNG"),
            size: Some(UVec2::new(640, 360)),
            cut_to: Some(Vec3::new(1.0, 2.0, 3.0)),
            cut_near: Some(4.0),
            leave_out: vec![7, 12],
            seen: true,
        }))
    );
    assert_eq!(
        parse("shot a.png"),
        Ok(Command::Shot(Ask {
            out: PathBuf::from("a.png"),
            size: None,
            cut_to: None,
            cut_near: None,
            leave_out: Vec::new(),
            seen: false,
        }))
    );
}

#[test]
fn blank_lines_and_comments_do_nothing() {
    for line in ["", "   ", "# a view of the barn", "#"] {
        assert_eq!(parse(line), Ok(Command::Nothing), "{line:?}");
    }
    assert_eq!(parse("where"), Ok(Command::Where));
    assert_eq!(parse(" quit "), Ok(Command::Quit));
}

#[test]
fn mistakes_are_named() {
    for line in [
        "fly 10",
        "look",
        "look --eye 1,2,3",
        "look --eye 1,2,3 --look 1,2,3",
        "look --size 4x4",
        "move",
        "move forward",
        "move forward ten",
        "move forward 1 back 2",
        "move sideways 3",
        "turn in 3",
        "orbit left 10 left 5",
        "shot",
        "shot a.jpg",
        "shot a.png --size 0x5",
        "shot a.png --size 4x4 --size 4x4",
        "shot a.png --cut-near 0",
        "shot a.png --cut-to 1,2",
        "shot a.png --seen --seen",
        "shot a.png --leave-out",
        "shot a.png --leave-out 7,x",
        "shot a.png --leave-out 7 --leave-out 8",
        "shot a.png --out b.png",
        "shot 'a.png",
        "where now",
        "quit 1",
    ] {
        assert!(parse(line).is_err(), "{line}");
    }
}
