//! The palette drawn over the viewer's frame, on the GPU, from the catalog the tests make.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use bevy::prelude::*;

use super::a_catalog;
use crate::args;
use crate::viewer::{Answers, Lines};

const GOLDSHIRE_LAMPPOST: &str = "--at -9433,44,57.5 --az 215 --el 12 --dist 16";
const SIZE: UVec2 = UVec2::new(960, 540);
/// The panel's default width, in pixels of a frame one pixel to the point.
const PANEL: u32 = 440;
const WITHIN: Duration = Duration::from_secs(300);

struct View {
    app: App,
    send: Sender<String>,
    answers: Receiver<String>,
}

impl View {
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
}

impl Drop for View {
    fn drop(&mut self) {
        self.send.send("quit".into()).ok();
        let deadline = Instant::now() + WITHIN;
        while self.app.should_exit().is_none() && Instant::now() < deadline {
            self.app.update();
        }
    }
}

/// How many pixels two shots differ in left of column `x`, and from it on.
fn differing(a: &Path, b: &Path, x: u32) -> (usize, usize) {
    let (a, b) = (read(a), read(b));
    assert_eq!(a.dimensions(), b.dimensions());
    let mut out = (0, 0);
    for ((col, _, p), q) in a.enumerate_pixels().zip(b.pixels()) {
        if p.0[..3] != q.0[..3] {
            if col < x {
                out.0 += 1;
            } else {
                out.1 += 1;
            }
        }
    }
    out
}

fn read(path: &Path) -> image::RgbaImage {
    image::open(path).expect("the shot").to_rgba8()
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_palette_opens_beside_goldshire_and_leaves_the_world_as_it_was() {
    let (Some(_), Some(out)) = (
        std::env::var_os("WOW_DATA"),
        std::env::var_os("CAIRN_PICTURES").map(PathBuf::from),
    ) else {
        eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
        return;
    };
    let catalog = a_catalog("view");
    let lists = catalog.join("lists");
    let mut view = View::open(&format!(
        "{GOLDSHIRE_LAMPPOST} --size {}x{} --catalog {} --lists {}",
        SIZE.x,
        SIZE.y,
        catalog.display(),
        lists.display()
    ));
    let shoot = |view: &mut View, name: &str| {
        let path = out.join(format!("palette-{name}.png"));
        let answer = view.ask(&format!("shot {}", path.display()));
        assert!(answer.starts_with("ok "), "{answer}");
        path
    };
    let closed = shoot(&mut view, "1-closed");
    let open = view.ask("palette open");
    let edge = SIZE.x - PANEL;
    let covers = format!("the panel covers x {edge}..{} of", SIZE.x);
    assert!(open.contains(&covers), "{open}");
    assert!(open.contains("4 shown"), "{open}");
    let open = shoot(&mut view, "2-open");
    let trees = view.ask("palette tab tree");
    assert!(trees.contains("2 shown"), "{trees}");
    let trees = shoot(&mut view, "3-trees");
    view.ask("palette close");
    let again = shoot(&mut view, "4-closed-again");
    let (beside, panel) = differing(&closed, &open, edge);
    assert_eq!(beside, 0, "the world beside the panel is as it was");
    assert_eq!(
        panel,
        (PANEL * SIZE.y) as usize,
        "the panel covers its part of the frame"
    );
    assert_eq!(differing(&open, &trees, edge).0, 0);
    assert!(differing(&open, &trees, edge).1 > 0, "the trees' tab shows");
    assert_eq!(
        differing(&closed, &again, edge),
        (0, 0),
        "closed, it is gone"
    );
    std::fs::remove_dir_all(catalog).ok();
}
