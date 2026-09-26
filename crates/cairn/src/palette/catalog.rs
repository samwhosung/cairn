use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fits::{MODEL_KINDS, Tables};

use crate::catalog::what_fits;

pub const GROUND: usize = MODEL_KINDS.len();
const UNDER_A_TENTH_OF_A_PERCENT: f64 = 0.0005;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Where it stands in [`MODEL_KINDS`], or [`GROUND`].
    pub kind: usize,
    /// As the install names it.
    pub path: String,
    pub key: String,
    /// Its picture, relative to the catalog.
    pub picture: String,
    /// A model's size, as the catalog gives it.
    pub size: String,
    /// How often the maps place a model, or how many chunks paint a ground texture.
    pub placed: u64,
    pub painted: Option<Painted>,
    words: Words,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Painted {
    /// What the texture's name says it is, as the catalog puts it: grass, road, rock... A search
    /// finds it by this word too.
    pub kind: String,
    /// The share of each zone's ground it paints.
    pub in_zones: Vec<Share>,
    /// The ground textures painted in the same chunks as it, with the share of its own ground there.
    pub beside: Vec<Share>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub name: String,
    /// A fraction.
    pub share: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Words {
    name: String,
    path_and_kinds: String,
    zones: Vec<String>,
}

/// How a search matched a thing, the closest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    /// Every word is in its file's name.
    Name,
    /// Every word is in its path or its kind.
    Path,
    /// Some word only names a zone that places it.
    Zone,
}

pub struct Catalog {
    pub dir: PathBuf,
    pub items: Vec<Item>,
    /// How many of the items are models; the ground textures follow them.
    pub models: usize,
    pub tables: Tables,
    by_key: BTreeMap<String, usize>,
    item_of_model: Vec<Option<usize>>,
}

impl Catalog {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let tables = fits::read(&dir.join(fits::IN_CATALOG)).map_err(|e| {
            format!(
                "{e}\nthe catalog in {} has no tables of what fits: `cairn catalog` writes them",
                dir.display()
            )
        })?;
        let mut items = rows(dir, "models.tsv", model)?;
        let models = items.len();
        items.extend(rows(dir, "ground.tsv", ground)?);
        let by_key: BTreeMap<String, usize> = items
            .iter()
            .enumerate()
            .map(|(i, item)| (kind_scoped_key(item.kind, &item.key), i))
            .collect();
        let item_of_model = tables
            .models
            .iter()
            .map(|m| by_key.get(&survey::key(&m.path)).copied())
            .collect();
        Ok(Self {
            dir: dir.to_path_buf(),
            items,
            models,
            tables,
            by_key,
            item_of_model,
        })
    }

    /// The item a named list's line names: a model, or else a ground texture, at an install path in
    /// any case, with `/` or `\` and any extension.
    pub fn find(&self, path: &str) -> Option<usize> {
        let key = survey::key(path.trim());
        self.by_key
            .get(&key)
            .or_else(|| self.by_key.get(&kind_scoped_key(GROUND, &key)))
            .copied()
    }

    pub fn item_of_model(&self, model: usize) -> Option<usize> {
        self.item_of_model.get(model).copied().flatten()
    }

    pub fn model_of_item(&self, item: usize) -> Option<usize> {
        (item < self.models)
            .then(|| self.tables.model(&self.items[item].path))
            .flatten()
    }

    /// The items of `kind`, or every model for `None`, most placed first, then by key.
    pub fn plain(&self, kind: Option<usize>) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.items.len())
            .filter(|&i| match kind {
                Some(k) => self.items[i].kind == k,
                None => i < self.models,
            })
            .collect();
        order.sort_by(|&a, &b| self.plainly(a, b));
        order
    }

    pub fn plainly(&self, a: usize, b: usize) -> Ordering {
        let (a, b) = (&self.items[a], &self.items[b]);
        b.placed.cmp(&a.placed).then_with(|| a.key.cmp(&b.key))
    }
}

impl Item {
    pub fn name(&self) -> &str {
        what_fits::stem(&self.path)
    }

    pub fn found(&self, words: &[String]) -> Option<Match> {
        let w = &self.words;
        if words.iter().all(|word| w.name.contains(word.as_str())) {
            return Some(Match::Name);
        }
        if words
            .iter()
            .all(|word| w.path_and_kinds.contains(word.as_str()))
        {
            return Some(Match::Path);
        }
        let zone = |word: &str| w.zones.iter().any(|z| z == word);
        words
            .iter()
            .all(|word| w.path_and_kinds.contains(word.as_str()) || zone(word))
            .then_some(Match::Zone)
    }

    fn with_words(mut self, zones: &[String]) -> Self {
        let kind = kind_name(self.kind);
        let painted = self.painted.as_ref().map_or("", |p| p.kind.as_str());
        let path_and_kinds = format!(
            "{} {kind} {kind}s {painted}",
            self.path.to_ascii_lowercase()
        );
        let zones = zones
            .iter()
            .flat_map(|zone| zone.split([' ', '\'', '-']))
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        self.words = Words {
            name: self.name().to_ascii_lowercase(),
            path_and_kinds: path_and_kinds.replace('\\', "/"),
            zones,
        };
        self
    }
}

pub fn kind_name(kind: usize) -> &'static str {
    MODEL_KINDS.get(kind).copied().unwrap_or("ground")
}

fn kind_scoped_key(kind: usize, key: &str) -> String {
    if kind == GROUND {
        format!("ground:{key}")
    } else {
        key.to_owned()
    }
}

pub fn words(search: &str) -> Vec<String> {
    search
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect()
}

fn rows(dir: &Path, name: &str, item: fn(&[&str]) -> Option<Item>) -> Result<Vec<Item>, String> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .skip(1)
        .enumerate()
        .map(|(i, line)| {
            let fields: Vec<&str> = line.split('\t').collect();
            item(&fields)
                .ok_or_else(|| format!("{}:{}: not a row of the catalog", path.display(), i + 2))
        })
        .collect()
}

fn model(f: &[&str]) -> Option<Item> {
    let &[kind, path, size, placed, _scales, zones, picture] = f else {
        return None;
    };
    let item = Item {
        kind: MODEL_KINDS.iter().position(|k| *k == kind)?,
        path: path.to_owned(),
        key: survey::key(path),
        picture: picture.to_owned(),
        size: size.to_owned(),
        placed: count(placed)?,
        painted: None,
        words: Words::default(),
    };
    Some(item.with_words(&zones_placing(zones)))
}

fn ground(f: &[&str]) -> Option<Item> {
    let &[kind, path, chunks, zones, beside, swatch] = f else {
        return None;
    };
    let in_zones = shares(zones);
    let names: Vec<String> = in_zones.iter().map(|s| s.name.clone()).collect();
    let item = Item {
        kind: GROUND,
        path: path.to_owned(),
        key: survey::key(path),
        picture: swatch.to_owned(),
        size: String::new(),
        placed: count(chunks)?,
        painted: Some(Painted {
            kind: kind.to_owned(),
            in_zones,
            beside: shares(beside),
        }),
        words: Words::default(),
    };
    Some(item.with_words(&names))
}

fn count(text: &str) -> Option<u64> {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn zones_placing(text: &str) -> Vec<String> {
    text.split(", ")
        .filter_map(|entry| {
            let digits = word_starts(entry)
                .find(|&i| entry[i..].starts_with(|c: char| c.is_ascii_digit()))?;
            Some(entry[..digits].trim_end().to_owned())
        })
        .collect()
}

fn shares(text: &str) -> Vec<Share> {
    text.split(", ").filter_map(share).collect()
}

fn share(entry: &str) -> Option<Share> {
    let (name, share) = if let Some((name, _)) = entry.rsplit_once(" under ") {
        (name, UNDER_A_TENTH_OF_A_PERCENT)
    } else {
        let (name, percent) = entry.rsplit_once(' ')?;
        (
            name,
            percent.strip_suffix('%')?.parse::<f64>().ok()? / 100.0,
        )
    };
    Some(Share {
        name: name.trim_end().to_owned(),
        share,
    })
}

fn word_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.match_indices(' ').map(|(i, _)| i + 1))
}
