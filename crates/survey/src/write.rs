//! The catalog on disk: every file written only when it is missing, each whole or not at all, and
//! the same install always giving the same bytes.

use std::fmt::Write as _;
use std::path::Path;

use light::LightCatalog;
use mpq::Chain;
use rayon::prelude::*;

use crate::pages::{self, Cell, PER_PAGE};
use crate::picture::{swatch as draw_swatch, write_atomically};
use crate::sky::{self, HOURS, hex};
use crate::text::{
    self, SkyLines, ZoneSound, ground_tsv, models_tsv, picture, size, stem, thousands, zones_tsv,
};
use crate::{Model, Survey, Zone, words};

/// What a run wrote and found already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub wrote: usize,
    pub kept: usize,
}

impl Written {
    fn add(&mut self, other: Written) {
        self.wrote += other.wrote;
        self.kept += other.kept;
    }
}

fn text_file(dir: &Path, rel: &str, body: impl FnOnce() -> String) -> Result<Written, String> {
    let path = dir.join(rel);
    if path.exists() {
        return Ok(Written { wrote: 0, kept: 1 });
    }
    let body = body();
    write_atomically(&path, |part| {
        std::fs::write(part, body).map_err(|e| format!("{}: {e}", part.display()))
    })?;
    Ok(Written { wrote: 1, kept: 0 })
}

fn picture_file(
    dir: &Path,
    rel: &str,
    draw: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Written, String> {
    let path = dir.join(rel);
    if path.exists() {
        return Ok(Written { wrote: 0, kept: 1 });
    }
    draw(&path)?;
    Ok(Written { wrote: 1, kept: 0 })
}

/// Writes the indexes, the detail files, the ground swatches and the zones' skies into `dir`.
/// `sounds` names a zone's music and ambience; `px_per_yard` says at what scale a model's picture
/// is drawn.
pub fn write(
    inv: &Survey,
    chain: &Chain,
    dir: &Path,
    sounds: &(dyn Fn(&Zone) -> ZoneSound + Sync),
    px_per_yard: &(dyn Fn(&Model) -> Option<f32> + Sync),
) -> Result<Written, String> {
    let lights = LightCatalog::load(chain).map_err(|e| format!("the lighting tables: {e}"))?;
    let mut done = Written::default();
    done.add(text_file(dir, "README.txt", || readme(inv))?);
    done.add(text_file(dir, "models.tsv", || models_tsv(inv))?);
    done.add(text_file(dir, "ground.tsv", || ground_tsv(inv))?);
    done.add(text_file(dir, "zones.tsv", || zones_tsv(inv, sounds))?);
    let models: Vec<Written> = inv
        .models
        .par_iter()
        .map(|m| {
            text_file(dir, &format!("models/{}.txt", m.key), || {
                text::model_txt(inv, m, px_per_yard(m))
            })
        })
        .collect::<Result<_, _>>()?;
    let grounds: Vec<Written> = (0..inv.grounds.len())
        .into_par_iter()
        .map(|g| {
            let key = &inv.grounds[g].key;
            let mut w = text_file(dir, &format!("ground/{key}.txt"), || {
                text::ground_txt(inv, g)
            })?;
            w.add(picture_file(dir, &text::swatch(key), |out| {
                draw_swatch(chain, &inv.grounds[g].path, out)
            })?);
            Ok(w)
        })
        .collect::<Result<_, String>>()?;
    let zones: Vec<Written> = inv
        .zones
        .par_iter()
        .map(|z| {
            let skies = z.heart.map(|heart| sky::skies(&lights, z.map, heart));
            let lines = SkyLines(skies.as_ref().map_or_else(Vec::new, sky_lines));
            let mut w = text_file(dir, &format!("zones/{}.txt", z.key), || {
                text::zone_txt(inv, z, &sounds(z), &lines)
            })?;
            if let Some(skies) = &skies {
                let title = format!("{}: the sky at its heart", z.name);
                w.add(picture_file(dir, &text::sky(z), |out| {
                    sky::draw(&title, skies, out)
                })?);
            }
            Ok(w)
        })
        .collect::<Result<_, String>>()?;
    for w in models.into_iter().chain(grounds).chain(zones) {
        done.add(w);
    }
    Ok(done)
}

fn sky_lines(skies: &[light::Atmosphere; 4]) -> Vec<String> {
    HOURS
        .iter()
        .zip(skies)
        .map(|((hour, _), a)| {
            let dome: Vec<String> = a.sky.iter().map(|c| hex(*c)).collect();
            format!(
                "{hour}: dome from the zenith down {}, fog {} from {:.0} to {:.0} yd; sun {}, ambient {}; river {} to {} deep, ocean {} to {} deep",
                dome.join(" "),
                hex(a.fog_color),
                a.fog_start_frac * a.fog_end.min(350.0),
                a.fog_end.min(350.0),
                hex(a.sun_diffuse),
                hex(a.ambient),
                hex(a.water_river[0]),
                hex(a.water_river[1]),
                hex(a.water_ocean[0]),
                hex(a.water_ocean[1]),
            )
        })
        .collect()
}

/// The models whose pictures are not in `dir` yet, by index.
pub fn pictures_missing(inv: &Survey, dir: &Path) -> Vec<usize> {
    (0..inv.models.len())
        .filter(|&i| !dir.join(picture(&inv.models[i])).exists())
        .collect()
}

/// The models of a page by kind and by zone: each page names its own cells.
struct Page {
    rel: String,
    title: String,
    first: usize,
    cells: Vec<(String, Cell)>,
}

fn model_cell(m: &Model, count: u32) -> (String, Cell) {
    let line = format!(
        "{}\t{}\t{} placed\t{}",
        m.path,
        size(m.bounds),
        thousands(count.into()),
        picture(m)
    );
    let tall = m.bounds.map_or(String::new(), |[lo, hi]| {
        format!("{:.1} yd tall, ", hi[2] - lo[2])
    });
    let cell = Cell {
        picture: picture(m).into(),
        name: stem(&m.path).to_owned(),
        facts: format!("{tall}{} placed", thousands(count.into())),
    };
    (line, cell)
}

fn paged(rel: &str, title: &str, cells: Vec<(String, Cell)>) -> Vec<Page> {
    let total = cells.len();
    let pages = total.div_ceil(PER_PAGE);
    let mut cells = cells.into_iter();
    (0..pages)
        .map(|p| {
            let chunk: Vec<(String, Cell)> = cells.by_ref().take(PER_PAGE).collect();
            let (first, last) = (p * PER_PAGE + 1, p * PER_PAGE + chunk.len());
            Page {
                rel: format!("{rel}-{:02}", p + 1),
                title: format!(
                    "{title}: {first} to {last} of {total} (page {} of {pages})",
                    p + 1
                ),
                first,
                cells: chunk,
            }
        })
        .collect()
}

const MODEL_KINDS: [&str; 6] = ["tree", "shrub", "rock", "fence", "prop", "building"];

fn plan(inv: &Survey) -> Vec<Page> {
    let mut pages = Vec::new();
    for kind in MODEL_KINDS {
        let mut of: Vec<&Model> = inv.models.iter().filter(|m| m.kind == kind).collect();
        of.sort_by(|a, b| {
            (b.on_ground + b.in_buildings)
                .cmp(&(a.on_ground + a.in_buildings))
                .then(a.key.cmp(&b.key))
        });
        let cells = of
            .iter()
            .map(|m| model_cell(m, m.on_ground + m.in_buildings))
            .collect();
        pages.extend(paged(
            &format!("pages/kind/{kind}"),
            &format!("{kind}s, most placed first"),
            cells,
        ));
    }
    for kind in words::ground_kinds() {
        let mut of: Vec<usize> = (0..inv.grounds.len())
            .filter(|&g| inv.grounds[g].kind == kind)
            .collect();
        of.sort_by(|&a, &b| {
            inv.grounds[b]
                .texels
                .total_cmp(&inv.grounds[a].texels)
                .then(a.cmp(&b))
        });
        let cells = of.iter().map(|&g| ground_cell(inv, g, None)).collect();
        pages.extend(paged(
            &format!("pages/ground/{kind}"),
            &format!("{kind} ground, most painted first"),
            cells,
        ));
    }
    for z in &inv.zones {
        for kind in MODEL_KINDS {
            let cells: Vec<(String, Cell)> = z
                .models
                .iter()
                .filter(|(m, _, _)| inv.models[*m].kind == kind)
                .map(|&(m, g, i)| model_cell(&inv.models[m], g + i))
                .collect();
            let title = format!("{kind}s placed in {}, most first", z.name);
            pages.extend(paged(
                &format!("pages/zone/{}/{kind}", z.key),
                &title,
                cells,
            ));
        }
        let cells = z
            .grounds
            .iter()
            .map(|&(g, t)| ground_cell(inv, g, Some((t, z))))
            .collect();
        let title = format!("ground painted in {}, most first", z.name);
        pages.extend(paged(
            &format!("pages/zone/{}/ground", z.key),
            &title,
            cells,
        ));
    }
    pages
}

fn ground_cell(inv: &Survey, g: usize, in_zone: Option<(f64, &Zone)>) -> (String, Cell) {
    let ground = &inv.grounds[g];
    let how = match in_zone {
        Some((t, z)) => format!(
            "{} of its ground",
            text::share(t, f64::from(z.chunks) * 4096.0)
        ),
        None => format!("{} chunks", thousands(ground.chunks.into())),
    };
    let line = format!(
        "{}\t{}\t{how}\t{}",
        ground.path,
        ground.kind,
        text::swatch(&ground.key)
    );
    let cell = Cell {
        picture: text::swatch(&ground.key).into(),
        name: stem(&ground.path).to_owned(),
        facts: format!("{}, {how}", ground.kind),
    };
    (line, cell)
}

/// Writes the pages of pictures and their lists, and `pages.tsv`, once every picture is in `dir`.
pub fn write_pages(inv: &Survey, dir: &Path) -> Result<Written, String> {
    let pages = plan(inv);
    let mut index = String::from("page\ttitle\tlist\n");
    for p in &pages {
        let _ = writeln!(index, "{}.png\t{}\t{}.txt", p.rel, p.title, p.rel);
    }
    let mut done = text_file(dir, "pages.tsv", || index)?;
    let written: Vec<Written> = pages
        .par_iter()
        .map(|p| {
            let first = p.first;
            let mut w = text_file(dir, &format!("{}.txt", p.rel), || {
                let mut out = format!("{}\n", p.title);
                for (i, (line, _)) in p.cells.iter().enumerate() {
                    let _ = writeln!(out, "{}\t{line}", first + i);
                }
                out
            })?;
            let cells: Vec<Cell> = p
                .cells
                .iter()
                .map(|(_, c)| Cell {
                    picture: dir.join(&c.picture),
                    name: c.name.clone(),
                    facts: c.facts.clone(),
                })
                .collect();
            w.add(picture_file(dir, &format!("{}.png", p.rel), |out| {
                pages::draw(&p.title, first, &cells, out)
            })?);
            Ok(w)
        })
        .collect::<Result<_, String>>()?;
    for w in written {
        done.add(w);
    }
    Ok(done)
}

fn readme(inv: &Survey) -> String {
    let buildings = inv.models.iter().filter(|m| m.building).count();
    format!(
        "\
The install's ground textures, doodads, buildings and zones, written by `cairn catalog` for an
agent to search and look through. Nothing here is edited by hand; delete a file and the next run
writes it again, the same as before.

What is here
  models.tsv   {models} models the maps place: {doodads} doodads and {buildings} buildings, a row each
  ground.tsv   {grounds} ground textures the maps paint, a row each
  zones.tsv    {zones} zones, a row each
  pages.tsv    every page of pictures and what it shows
  models/      each model's picture (.png) and what is known of it (.txt), at its install path
  ground/      each ground texture's swatch (.png) and where it is painted (.txt)
  zones/       each zone's sky (-sky.png) and its ground, water, music, ambience and models (.txt)
  pages/       pictures twenty to a page, numbered, each with a list (.txt) of what the numbers are:
               kind/   models by kind, most placed first: tree, shrub, rock, fence, prop, building
               ground/ ground textures by kind, most painted first
               zone/   a zone's models by kind and its ground, most first

Searching: every .tsv is tab-separated, a header first, a row a thing. Search them with grep:
  grep -i farmhouse models.tsv              models whose path says farmhouse
  grep -i pine models.tsv | grep 'Elwynn'   pines placed in Elwynn Forest
  awk -F'\\t' '$1 == \"tree\" && /Duskwood/' models.tsv   trees placed in Duskwood
  grep -i cobble ground.tsv                 cobbled ground
Names say only so much: an Elwynn pine is named ElwynnPine01, but many trees name no species. Look
at pages/zone/elwynn-forest/tree-01.png and the pages after it to see all of Elwynn's trees.

The pictures: each model is drawn from the north-west, 25 degrees above it, with its front (+x)
to the viewer's left and its left side (+y) to the right, through an orthographic camera, so a
yard is the same length anywhere in the picture. The dark figure beside it stands on the same
ground and is a player's height, 2.03 yd. Models are drawn at rest, lit by Azeroth's noon, on a
flat backdrop. A building shows the doodads its default set places.

Kinds are guesses from the words in a file's name; the pictures are the truth.
Sizes are yards at scale 1: how tall, then its extent forward (x) by left (y). Placements count
each doodad and building once, on the ground and inside the buildings that place it. A zone's
share of ground counts the texels a texture shows on under the client's blending; a chunk is 33.3
yd square and 64 by 64 texels.
Positions are the world's: x north, y west, z up, in yards.
",
        models = thousands(inv.models.len() as u64),
        doodads = thousands((inv.models.len() - buildings) as u64),
        buildings = thousands(buildings as u64),
        grounds = thousands(inv.grounds.len() as u64),
        zones = thousands(inv.zones.len() as u64),
    )
}
