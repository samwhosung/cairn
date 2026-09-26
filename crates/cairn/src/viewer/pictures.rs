use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use world::coords::bevy_to_wow;
use world::{PlacedModel, Placements};

use super::{Answers, Lines};
use crate::args::{self, Mode};

const LAMPPOST_XY: [f32; 2] = [-9433.0, 44.0];
const FACING_THE_LAMPPOST_DEG: f32 = 215.0;
const START_Z: f32 = 57.5;
const SIZE: &str = "640x360";
const WITHIN: Duration = Duration::from_secs(300);

struct Viewer {
    app: App,
    send: Sender<String>,
    answers: Receiver<String>,
}

impl Viewer {
    fn open(flags: &str) -> Self {
        let argv = format!("view {flags}");
        let args = args::parse(argv.split_whitespace().map(str::to_owned)).expect("the flags");
        let (install, map, start) = crate::open(&args.map).expect("the map opens");
        let (send, lines) = channel();
        let (answer, answers) = channel::<String>();
        let answer = Mutex::new(answer);
        let mut app = App::new();
        app.insert_resource(Lines(Mutex::new(lines)))
            .insert_resource(Answers(Box::new(move |line| {
                if let Ok(answer) = answer.lock() {
                    answer.send(line.to_owned()).ok();
                }
            })));
        crate::client::assemble(&mut app, args, &install, map, start, std::convert::identity)
            .expect("the viewer assembles");
        app.finish();
        app.cleanup();
        Self { app, send, answers }
    }

    fn ask(&mut self, line: &str) -> String {
        self.send.send(line.to_owned()).expect("the viewer listens");
        let deadline = Instant::now() + WITHIN;
        loop {
            assert!(Instant::now() < deadline, "no answer to {line}");
            self.app.update();
            if let Ok(answer) = self.answers.try_recv()
                && !answer.starts_with("ready")
            {
                eprintln!("{line}\n  {answer}");
                return answer;
            }
        }
    }

    fn placements(&self) -> &Placements {
        self.app.world().resource::<Placements>()
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.send.send("quit".into()).ok();
        let deadline = Instant::now() + WITHIN;
        while self.app.should_exit().is_none() && Instant::now() < deadline {
            self.app.update();
        }
    }
}

fn pictures() -> Option<PathBuf> {
    std::env::var_os("WOW_DATA")?;
    std::env::var_os("CAIRN_PICTURES").map(PathBuf::from)
}

fn shot_alone(flags: &str, out: &Path) {
    let argv = format!("shot {flags} --out {}", out.display());
    let mut args = args::parse(argv.split_whitespace().map(str::to_owned)).expect("the flags");
    args.mode = Mode::Shot(out.to_path_buf());
    let (install, map, start) = crate::open(&args.map).expect("the map opens");
    let mut app = App::new();
    crate::client::assemble(&mut app, args, &install, map, start, std::convert::identity)
        .expect("the shot assembles");
    app.finish();
    app.cleanup();
    let deadline = Instant::now() + WITHIN;
    while app.should_exit().is_none() {
        assert!(Instant::now() < deadline, "the shot never came");
        app.update();
    }
    assert_eq!(app.should_exit(), Some(AppExit::Success));
}

fn load(path: &Path) -> Image {
    let bytes = std::fs::read(path).expect("the picture");
    Image::from_buffer(
        &bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::MAIN_WORLD,
    )
    .expect("a PNG")
}

fn pixels_differing(a: &Path, b: &Path) -> usize {
    let (a, b) = (load(a), load(b));
    assert_eq!(a.size(), b.size());
    let (a, b) = (a.data.unwrap_or_default(), b.data.unwrap_or_default());
    let (a, b) = (a.as_chunks::<4>().0, b.as_chunks::<4>().0);
    a.iter().zip(b).filter(|(p, q)| p[..3] != q[..3]).count()
}

fn pixels_by_placement(list: &Path) -> BTreeMap<u32, usize> {
    let text = std::fs::read_to_string(list).expect("the list");
    text.lines()
        .skip_while(|l| !l.starts_with("id "))
        .skip(1)
        .map(|row| {
            let words: Vec<&str> = row.split_whitespace().collect();
            let id = words.first().and_then(|w| w.parse().ok()).expect("an id");
            let pixels = words.get(2).and_then(|w| w.parse().ok()).expect("pixels");
            (id, pixels)
        })
        .collect()
}

fn start() -> String {
    let [x, y] = LAMPPOST_XY;
    let az = FACING_THE_LAMPPOST_DEG;
    format!("--at {x},{y},{START_Z} --az {az} --el 12 --dist 16")
}

fn lamppost(placements: &Placements) -> (u32, Vec3) {
    let [x, y] = LAMPPOST_XY;
    placements
        .iter()
        .filter_map(|(id, placed)| {
            let PlacedModel::Doodad { url } = &placed.model else {
                return None;
            };
            let foot = Vec3::from(bevy_to_wow(placed.transform.translation));
            url.contains("lamppost").then_some((id, foot))
        })
        .min_by(|a, b| {
            let away = |f: Vec3| f.truncate().distance(Vec2::new(x, y));
            away(a.1).total_cmp(&away(b.1)).then(a.0.cmp(&b.0))
        })
        .expect("a lamppost in Goldshire")
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_viewer_shoots_as_the_shot_does_names_what_it_shows_and_cuts_away_what_stands_between() {
    let Some(dir) = pictures() else {
        eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
        return;
    };
    let at = |name: &str| dir.join(format!("viewer-{name}.png"));
    let start = start();
    let mut viewer = Viewer::open(&format!("{start} --no-glow --size {SIZE}"));
    let first = viewer.ask(&format!(
        "shot {} --size {SIZE}",
        at("1-as-a-shot").display()
    ));
    assert!(first.starts_with("ok "), "{first}");
    shot_alone(
        &format!("{start} --no-glow --size {SIZE}"),
        &at("1-by-cairn-shot"),
    );
    assert_eq!(
        pixels_differing(&at("1-as-a-shot"), &at("1-by-cairn-shot")),
        0,
        "the viewer's first shot is the shot's"
    );

    let (lamp, foot) = lamppost(viewer.placements());
    let toward = {
        let r = FACING_THE_LAMPPOST_DEG.to_radians();
        Vec3::new(ops::cos(r), ops::sin(r), 0.0)
    };
    let pole = foot + Vec3::Z * 2.2;
    let (eye, beyond) = (pole - toward * 6.0, pole + toward * 30.0);
    let xyz = |v: Vec3| format!("{},{},{}", v.x, v.y, v.z);
    viewer.ask(&format!("look --eye {} --look {}", xyz(eye), xyz(beyond)));
    let shoot = |viewer: &mut Viewer, name: &str, flags: &str| {
        let answer = viewer.ask(&format!(
            "shot {} --size {SIZE} {flags}",
            at(name).display()
        ));
        assert!(answer.starts_with("ok "), "{answer}");
        answer
    };
    shoot(&mut viewer, "2-through-a-lamppost", "--seen");
    let seen = pixels_by_placement(&at("2-through-a-lamppost").with_extension("txt"));
    let covered = *seen.get(&lamp).expect("the lamppost is listed");
    shoot(
        &mut viewer,
        "3-the-lamppost-left-out",
        &format!("--leave-out {lamp} --seen"),
    );
    let without = pixels_by_placement(&at("3-the-lamppost-left-out").with_extension("txt"));
    assert!(!without.contains_key(&lamp), "left out, it leaves the list");
    let changed = pixels_differing(&at("2-through-a-lamppost"), &at("3-the-lamppost-left-out"));
    assert!(
        covered > 0 && changed >= covered,
        "every pixel it covers changes, and its glow lights more: {covered} covered, {changed} changed"
    );

    let cut = shoot(
        &mut viewer,
        "4-cut-to-beyond-it",
        &format!("--cut-to {}", xyz(beyond)),
    );
    let left_out = cut.split(", left out ").nth(1).unwrap_or_default();
    assert!(
        left_out
            .split([' ', ',', ';'])
            .any(|id| id == lamp.to_string()),
        "{cut}"
    );
    shoot(&mut viewer, "5-again", "--seen");
    assert_eq!(
        pixels_differing(&at("2-through-a-lamppost"), &at("5-again")),
        0,
        "the same camera again is the same picture"
    );
    assert_eq!(
        pixels_by_placement(&at("5-again").with_extension("txt")),
        seen
    );
    let meshes = |viewer: &mut Viewer| {
        let world = viewer.app.world_mut();
        world.query::<&Mesh3d>().iter(world).count()
    };
    let before = meshes(&mut viewer);
    shoot(&mut viewer, "6-and-again", "--seen");
    assert_eq!(
        meshes(&mut viewer),
        before,
        "a world held still grows nothing"
    );
}
