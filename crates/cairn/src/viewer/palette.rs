use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use world::WorldCamera;

use super::{Answers, Step, Viewer};
use crate::palette::{
    Armed, GROUND, KINDS, Order, Palette, Pick, Pictures, SIDES, Settled, Tab, WIDTHS, panel_part,
    why,
};

const GIVE_UP_AFTER: Duration = Duration::from_secs(120);
const FIRST_PAGE: usize = 20;
const AWAITED: Duration = Duration::from_secs(10);

/// A `palette` command: it changes what the panel shows, and is answered once the panel shows it.
#[derive(Debug, PartialEq)]
pub enum Ask {
    Say,
    Open(bool),
    Tab(String),
    Search(String),
    Order(Order),
    Size(f32),
    Width(f32),
    Follow(bool),
    Pick(usize),
    List { out: PathBuf, top: usize },
    Await { name: String, within: Duration },
    Frames(u32),
}

pub struct Asking {
    ask: Ask,
    asked: Instant,
    applied: bool,
    frames: Vec<Duration>,
    last: Option<Instant>,
}

impl Asking {
    pub fn new(ask: Ask) -> Self {
        Self {
            ask,
            asked: Instant::now(),
            applied: false,
            frames: Vec::new(),
            last: None,
        }
    }
}

pub fn parse(said: &[&str]) -> Result<Ask, String> {
    let number = |word: &str, what: &str| {
        word.parse::<f32>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.0)
            .ok_or_else(|| format!("palette {what} wants a number above 0, not {word}"))
    };
    Ok(match said {
        [] => Ask::Say,
        ["open"] => Ask::Open(true),
        ["close"] => Ask::Open(false),
        ["tab", name] => Ask::Tab((*name).to_owned()),
        ["search", words @ ..] => Ask::Search(words.join(" ")),
        ["order", "fits"] => Ask::Order(Order::Fits),
        ["order", "plain"] => Ask::Order(Order::Plain),
        ["order", "listed"] => Ask::Order(Order::Listed),
        ["size", px] => Ask::Size(number(px, "size")?),
        ["width", px] => Ask::Width(number(px, "width")?),
        ["follow", "on"] => Ask::Follow(true),
        ["follow", "off"] => Ask::Follow(false),
        ["pick", n] => Ask::Pick(
            n.parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("palette pick wants a place from 1, not {n}"))?,
        ),
        ["list", out] | ["list", out, "--top", _] => {
            let top = match said {
                [_, _, _, n] => n
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("--top wants a count above 0, not {n}"))?,
                _ => FIRST_PAGE,
            };
            Ask::List {
                out: PathBuf::from(out),
                top,
            }
        }
        ["await", name] => Ask::Await {
            name: (*name).to_owned(),
            within: AWAITED,
        },
        ["frames", n] => Ask::Frames(
            n.parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("palette frames wants a count above 0, not {n}"))?,
        ),
        _ => {
            return Err("palette takes open, close, tab NAME, search WORDS, order \
                        fits|plain|listed, size PX, width PX, follow on|off, pick N, \
                        list FILE [--top N], await NAME or frames N"
                .into());
        }
    })
}

type WorldCameras<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldCamera>>;

/// Carries out the palette's command, then answers it once the panel shows what was asked.
pub(super) fn answer(
    mut viewer: ResMut<'_, Viewer>,
    mut palette: ResMut<'_, Palette>,
    pictures: Res<'_, Pictures>,
    mut armed: ResMut<'_, Armed>,
    answers: Option<Res<'_, Answers>>,
    camera: WorldCameras<'_, '_>,
    settled: Res<'_, Settled>,
) {
    let Step::Palette(asking) = &mut viewer.step else {
        return;
    };
    let say = |line: &str| {
        if let Some(answers) = &answers {
            answers.say(line);
        }
    };
    if !asking.applied {
        asking.applied = true;
        if let Err(e) = apply(&asking.ask, &mut palette) {
            say(&format!("error: {e}"));
            viewer.step = Step::Idle;
        }
        return;
    }
    let placed = camera.single().ok();
    let answer = match &asking.ask {
        Ask::Await { name, within } => {
            match palette.lists.as_ref().and_then(|l| l.late.get(name)) {
                Some(late) if settled.0 => Some(Ok(format!(
                    "the list {name} showed {:.0} ms after its file changed",
                    late.as_secs_f64() * 1000.0
                ))),
                None if asking.asked.elapsed() > *within => Some(Err(format!(
                    "no list {name} showed in {} s",
                    within.as_secs()
                ))),
                _ => None,
            }
        }
        Ask::Frames(n) => {
            let now = Instant::now();
            if let Some(last) = asking.last.replace(now) {
                asking.frames.push(now - last);
            }
            (asking.frames.len() >= *n as usize).then(|| Ok(frames(&asking.frames)))
        }
        _ if !settled.0 => None,
        Ask::Pick(n) => Some(pick(&mut palette, &mut armed, *n)),
        Ask::List { out, top } => Some(list(&mut palette, out, *top)),
        _ => Some(Ok(String::new())),
    };
    let answer = match answer {
        None if asking.asked.elapsed() > GIVE_UP_AFTER => Some(Err(format!(
            "the palette did not settle in {} s",
            GIVE_UP_AFTER.as_secs()
        ))),
        answer => answer,
    };
    let Some(answer) = answer else {
        return;
    };
    match answer {
        Ok(said) => {
            let state = state(&mut palette, &pictures, &armed, placed.map(|(c, _)| c));
            let said = if said.is_empty() {
                state
            } else {
                format!("{said}; {state}")
            };
            say(&format!("ok {said}"));
        }
        Err(e) => say(&format!("error: {e}")),
    }
    viewer.step = Step::Idle;
}

fn apply(ask: &Ask, palette: &mut Palette) -> Result<(), String> {
    match ask {
        Ask::Open(open) => palette.open = *open,
        Ask::Tab(name) => {
            let tab = palette
                .tabs()
                .into_iter()
                .find(|t| tab_word(t) == *name)
                .ok_or_else(|| {
                    let words: Vec<String> = palette.tabs().iter().map(tab_word).collect();
                    format!("no tab {name}: {}", words.join(", "))
                })?;
            palette.choose(tab);
        }
        Ask::Search(words) => palette.search.clone_from(words),
        Ask::Order(order) => {
            if !palette.orders().contains(order) {
                return Err("this tab has no such order".into());
            }
            palette.order = *order;
        }
        Ask::Size(side) => palette.side = side.clamp(*SIDES.start(), *SIDES.end()),
        Ask::Width(width) => palette.width = width.clamp(*WIDTHS.start(), *WIDTHS.end()),
        Ask::Follow(follows) => palette.follows = *follows,
        Ask::Say | Ask::Pick(_) | Ask::List { .. } | Ask::Await { .. } | Ask::Frames(_) => {}
    }
    Ok(())
}

/// A tab as the command names it: its kind, `all`, `recent`, or a list's name.
fn tab_word(tab: &Tab) -> String {
    match tab {
        Tab::Every => "all".to_owned(),
        Tab::Kind(k) => KINDS[*k].to_owned(),
        Tab::Recent => "recent".to_owned(),
        Tab::List(name) => name.clone(),
    }
}

fn pick(palette: &mut Palette, armed: &mut Armed, n: usize) -> Result<String, String> {
    let shown = palette.shown();
    let item = *shown
        .get(n - 1)
        .ok_or_else(|| format!("{} things are shown, not {n}", shown.len()))?;
    palette.pick(item, armed);
    Ok(String::new())
}

/// Writes the first `top` things shown as `cairn catalog fits` lists them, the spot first.
fn list(palette: &mut Palette, out: &std::path::Path, top: usize) -> Result<String, String> {
    let Some(catalog) = palette.catalog().cloned() else {
        return Err("the palette has no catalog".into());
    };
    let shown = palette.shown();
    let mut text = palette
        .ranked
        .as_ref()
        .map_or_else(String::new, |r| r.found.header.clone() + "\n");
    text.push_str("rank\tkind\tpath\twhy\tpicture\n");
    for (i, &item) in shown.iter().take(top).enumerate() {
        let it = &catalog.items[item];
        let said = why(&catalog, item, palette)
            .or_else(|| palette.said_of(item).map(str::to_owned))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{said}\t{}",
            i + 1,
            KINDS[it.kind],
            it.path,
            it.picture
        );
    }
    std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(format!(
        "wrote the first {} of {} to {}",
        shown.len().min(top),
        shown.len(),
        out.display()
    ))
}

fn frames(took: &[Duration]) -> String {
    let each = took.iter().sum::<Duration>().as_secs_f64() * 1000.0 / took.len() as f64;
    let most = took.iter().max().copied().unwrap_or_default().as_secs_f64() * 1000.0;
    format!(
        "{} frames, {each:.2} ms each, the slowest {most:.2} ms",
        took.len()
    )
}

fn state(
    palette: &mut Palette,
    pictures: &Pictures,
    armed: &Armed,
    camera: Option<&Camera>,
) -> String {
    if !palette.open {
        return "the palette is closed".to_owned();
    }
    if let Some(e) = palette.failed() {
        return format!("the palette is open but shows nothing: {e}");
    }
    let mut said = format!(
        "the palette is open on {}, {}",
        tab_word(&palette.tab),
        match palette.order {
            Order::Fits => "what fits here first",
            Order::Plain => "the most placed first",
            Order::Listed => "as listed",
        }
    );
    if palette.catalog().is_some() {
        let _ = write!(said, ", {} shown", palette.shown().len());
    }
    if !palette.search.is_empty() {
        let _ = write!(said, " for \"{}\"", palette.search);
    }
    if let (Some(ranked), Some(catalog)) = (&palette.ranked, palette.catalog()) {
        let zone = ranked
            .found
            .spot
            .zone
            .map_or("no zone", |z| catalog.tables.zones[z].name.as_str());
        let _ = write!(
            said,
            "; the spot {},{} in {zone}, ranked {:.1} ms after the camera settled",
            ranked.at[0],
            ranked.at[1],
            palette.took.unwrap_or_default().as_secs_f64() * 1000.0
        );
    }
    if let Some(trouble) = &palette.trouble {
        let _ = write!(said, "; the spot: {trouble}");
    }
    let (held, bytes) = pictures.held();
    let _ = write!(
        said,
        "; {held} pictures held, {:.1} MB, {} px across",
        bytes as f64 / (1u64 << 20) as f64,
        pictures.side()
    );
    match &armed.0 {
        Some(Pick::Model(path)) => {
            let _ = write!(said, "; armed the model {path}");
        }
        Some(Pick::Ground(path)) => {
            let _ = write!(said, "; armed the {} {path}", KINDS[GROUND]);
        }
        None => {}
    }
    let part =
        camera.and_then(|c| Some((panel_part(c, palette.width)?, c.physical_target_size()?)));
    if let Some((part, frame)) = part {
        let _ = write!(
            said,
            "; the panel covers x {}..{} of {}x{}",
            part.physical_position.x,
            part.physical_position.x + part.physical_size.x,
            frame.x,
            frame.y
        );
    }
    said
}

#[cfg(test)]
mod tests;
