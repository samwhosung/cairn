use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fits::{MODEL_KINDS, Tables};

use crate::catalog::what_fits;

/// The kinds a tab can show: the catalog's model kinds, then ground textures.
pub const KINDS: [&str; 7] = [
    "tree", "shrub", "rock", "fence", "prop", "building", "ground",
];
pub const GROUND: usize = 6;
/// What the catalog's `under 0.1%` is taken to be.
const UNDER_A_TENTH_OF_A_PERCENT: f64 = 0.0005;

/// A model or ground texture of the catalog, with what its row says of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Where it stands in [`KINDS`].
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
    /// The zones that place a model, or that paint a ground texture with its share of their ground.
    pub zones: Vec<(String, f64)>,
    /// The ground textures painted in the same chunks as this one, with the share of its own.
    pub beside: Vec<(String, f64)>,
    /// What a ground texture's name says it is, as the catalog puts it: grass, road, rock...
    pub ground: String,
    words: String,
}

/// The catalog as the palette shows it: every model, then every ground texture.
pub struct Catalog {
    pub dir: PathBuf,
    pub items: Vec<Item>,
    /// How many of the items are models; the ground textures follow them.
    pub models: usize,
    pub tables: Tables,
    by_key: BTreeMap<String, usize>,
    /// The item of each of the tables' models.
    item_of_model: Vec<Option<usize>>,
}

impl Catalog {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let tables = fits::read(&dir.join(what_fits::TABLES)).map_err(|e| {
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
            .map(|(i, item)| (lookup(item.kind, &item.key), i))
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
            .or_else(|| self.by_key.get(&lookup(GROUND, &key)))
            .copied()
    }

    pub fn item_of_model(&self, model: usize) -> Option<usize> {
        self.item_of_model.get(model).copied().flatten()
    }

    /// The tables' model of a model item.
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

    /// Whether every word of a search is in its path, its kind or a zone that places it.
    pub fn matches(&self, words: &[String]) -> bool {
        words.iter().all(|w| self.words.contains(w.as_str()))
    }

    fn with_words(mut self) -> Self {
        let kind = KINDS[self.kind];
        let mut words = format!(
            "{} {kind} {kind}s {}",
            self.path.to_ascii_lowercase(),
            self.ground
        );
        for (zone, _) in &self.zones {
            words.push_str(" | ");
            words.push_str(&zone.to_ascii_lowercase());
        }
        self.words = words.replace('\\', "/");
        self
    }
}

/// A model and a ground texture may share a key.
fn lookup(kind: usize, key: &str) -> String {
    if kind == GROUND {
        format!("ground:{key}")
    } else {
        key.to_owned()
    }
}

/// The words of a search, lowercase.
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
                .map(Item::with_words)
                .ok_or_else(|| format!("{}:{}: not a row of the catalog", path.display(), i + 2))
        })
        .collect()
}

/// `kind path size placed scales zones picture`
fn model(f: &[&str]) -> Option<Item> {
    let &[kind, path, size, placed, _scales, zones, picture] = f else {
        return None;
    };
    Some(Item {
        kind: MODEL_KINDS.iter().position(|k| *k == kind)?,
        path: path.to_owned(),
        key: survey::key(path),
        picture: picture.to_owned(),
        size: size.to_owned(),
        placed: count(placed)?,
        zones: listed(zones, count_of),
        beside: Vec::new(),
        ground: String::new(),
        words: String::new(),
    })
}

/// `kind path chunks zones beside swatch`
fn ground(f: &[&str]) -> Option<Item> {
    let &[kind, path, chunks, zones, beside, swatch] = f else {
        return None;
    };
    Some(Item {
        kind: GROUND,
        path: path.to_owned(),
        key: survey::key(path),
        picture: swatch.to_owned(),
        size: String::new(),
        placed: count(chunks)?,
        zones: listed(zones, share_of),
        beside: listed(beside, share_of),
        ground: kind.to_owned(),
        words: String::new(),
    })
}

/// The first number in `text`, its thousands separated by commas.
fn count(text: &str) -> Option<u64> {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// A list of `name amount` separated by `, `, a name running up to its amount.
fn listed(text: &str, amount: fn(&str) -> Option<(usize, f64)>) -> Vec<(String, f64)> {
    text.split(", ")
        .filter_map(|entry| {
            let (at, value) = amount(entry)?;
            Some((entry[..at].trim_end().to_owned(), value))
        })
        .collect()
}

/// A zone's placements: where the first word that opens with a digit starts, and the number.
fn count_of(entry: &str) -> Option<(usize, f64)> {
    let at = word_starts(entry).find(|&i| entry[i..].starts_with(|c: char| c.is_ascii_digit()))?;
    Some((at, count(&entry[at..])? as f64))
}

/// A share of ground: `12%`, `6.6%` or `under 0.1%`, as a fraction.
fn share_of(entry: &str) -> Option<(usize, f64)> {
    if let Some(i) = entry.rfind(" under ") {
        return Some((i, UNDER_A_TENTH_OF_A_PERCENT));
    }
    let i = entry.rfind(' ')?;
    let percent = entry[i + 1..].strip_suffix('%')?.parse::<f64>().ok()?;
    Some((i, percent / 100.0))
}

fn word_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.match_indices(' ').map(|(i, _)| i + 1))
}
