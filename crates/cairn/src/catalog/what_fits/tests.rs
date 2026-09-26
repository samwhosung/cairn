use super::*;

fn parsed(line: &str) -> Result<Asked, String> {
    parse(line.split_whitespace().map(str::to_owned))
}

#[test]
fn a_spot_is_asked_for_on_a_map_or_in_a_zone_of_its_own() {
    assert_eq!(
        parsed("--at -9464,62"),
        Ok(Asked {
            dir: PathBuf::from("catalog"),
            what: What::Install {
                at: [-9464.0, 62.0],
                map: "Azeroth".to_owned(),
            },
            kind: None,
            borrows: None,
            top: 20,
            sheet: None,
        })
    );
    assert_eq!(
        parsed("out/c --at 1,2 --zone z --kind tree --borrows none --top 5 --sheet s.png"),
        Ok(Asked {
            dir: PathBuf::from("out/c"),
            what: What::OwnZone {
                at: [1.0, 2.0],
                root: PathBuf::from("z"),
            },
            kind: Some(0),
            borrows: Some(Borrows::Nothing),
            top: 5,
            sheet: Some(PathBuf::from("s.png")),
        })
    );
    assert_eq!(
        parsed("--replay j.txt --borrows Redridge").map(|a| (a.what, a.borrows)),
        Ok((
            What::Replay {
                history: PathBuf::from("j.txt")
            },
            Some(Borrows::Zone("Redridge".into()))
        ))
    );
    for line in [
        "",
        "--at 1",
        "--at 1,2 --map Kalimdor --zone z",
        "--at 1,2 --borrows none",
        "--replay j.txt --at 1,2",
        "--at 1,2 --kind bush",
        "--at 1,2 --top 0",
        "--at 1,2 --sheet s.jpg",
        "--at 1,2 --at 3,4",
        "--at 1,2 --size 5",
        "a b --at 1,2",
    ] {
        assert!(parsed(line).is_err(), "{line}");
    }
}
