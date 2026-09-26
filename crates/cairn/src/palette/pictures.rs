use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

/// The bytes the pictures may hold on the GPU; beyond it, those shown least lately go.
const BUDGET: usize = 32 << 20;
/// Pictures decoded at once.
const AT_ONCE: usize = 8;

/// The catalog's pictures the grid shows, each read and shrunk off the frame to the side it is drawn
/// at, and held on the GPU alone.
#[derive(Resource, Default)]
pub struct Pictures {
    side: u32,
    wanted_side: u32,
    held: BTreeMap<usize, Held>,
    decoding: BTreeMap<usize, Task<Result<Decoded, String>>>,
    wanted: Vec<(usize, PathBuf)>,
    failed: BTreeSet<usize>,
    frame: u64,
}

struct Held {
    image: Handle<Image>,
    id: egui::TextureId,
    bytes: usize,
    shown: u64,
}

pub(super) struct Decoded {
    pub(super) rgba: Vec<u8>,
    pub(super) side: u32,
}

/// What a picture is, once asked for.
pub enum Shown {
    Picture(egui::TextureId),
    Coming,
    Missing,
}

impl Pictures {
    /// Pictures `side` pixels across from the next frame on; those of another side are let go.
    pub fn at_side(&mut self, side: u32) {
        self.wanted_side = side;
    }

    /// Forgets what the last pass of the grid asked for.
    pub fn begin_pass(&mut self) {
        self.wanted.clear();
    }

    /// The picture of `item`, read from `path` if it isn't held yet.
    pub fn show(&mut self, item: usize, path: impl FnOnce() -> PathBuf) -> Shown {
        let this_side = self.side == self.wanted_side;
        if let Some(held) = self.held.get_mut(&item).filter(|_| this_side) {
            held.shown = self.frame;
            return Shown::Picture(held.id);
        }
        if self.failed.contains(&item) {
            return Shown::Missing;
        }
        if !self.wanted.iter().any(|(i, _)| *i == item) {
            self.wanted.push((item, path()));
        }
        Shown::Coming
    }

    /// Whether every picture the grid asked for is held or missing.
    pub fn settled(&self) -> bool {
        self.side == self.wanted_side
            && self.decoding.is_empty()
            && self
                .wanted
                .iter()
                .all(|(i, _)| self.held.contains_key(i) || self.failed.contains(i))
    }

    pub fn held(&self) -> (usize, usize) {
        (self.held.len(), self.held.values().map(|h| h.bytes).sum())
    }

    pub fn side(&self) -> u32 {
        self.side
    }
}

/// Starts reading what the grid asked for, takes what has been read onto the GPU, and lets go of
/// what was shown least lately beyond the budget.
pub fn load(
    mut pictures: ResMut<'_, Pictures>,
    mut images: ResMut<'_, Assets<Image>>,
    mut textures: ResMut<'_, EguiUserTextures>,
) {
    let pictures = &mut *pictures;
    if pictures.wanted_side != pictures.side {
        pictures.side = pictures.wanted_side;
        pictures.decoding.clear();
        pictures.failed.clear();
        for (_, held) in std::mem::take(&mut pictures.held) {
            textures.remove_image(&held.image);
            images.remove(&held.image);
        }
    }
    let finished: Vec<usize> = pictures
        .decoding
        .iter_mut()
        .filter_map(|(&item, task)| block_on(poll_once(task)).map(|done| (item, done)))
        .map(|(item, done)| {
            match done {
                Ok(decoded) => {
                    let image = images.add(texture(decoded.rgba, decoded.side));
                    let id = textures.add_image(EguiTextureHandle::Strong(image.clone()));
                    let bytes = (decoded.side * decoded.side * 4) as usize;
                    let shown = pictures.frame;
                    pictures.held.insert(
                        item,
                        Held {
                            image,
                            id,
                            bytes,
                            shown,
                        },
                    );
                }
                Err(e) => {
                    warn!("the palette's picture: {e}");
                    pictures.failed.insert(item);
                }
            }
            item
        })
        .collect();
    for item in finished {
        pictures.decoding.remove(&item);
    }
    let side = pictures.side;
    let pool = AsyncComputeTaskPool::get();
    for (item, path) in &pictures.wanted {
        let known = pictures.held.contains_key(item) || pictures.failed.contains(item);
        if known || pictures.decoding.contains_key(item) {
            continue;
        }
        if pictures.decoding.len() >= AT_ONCE {
            break;
        }
        let path = path.clone();
        let task = pool.spawn(async move { decode(&path, side) });
        pictures.decoding.insert(*item, task);
    }
    let mut by_age: Vec<(u64, usize)> = pictures.held.iter().map(|(&i, h)| (h.shown, i)).collect();
    by_age.sort_unstable();
    let mut bytes: usize = pictures.held.values().map(|h| h.bytes).sum();
    for (shown, item) in by_age {
        if bytes <= BUDGET || shown >= pictures.frame {
            break;
        }
        if let Some(held) = pictures.held.remove(&item) {
            textures.remove_image(&held.image);
            images.remove(&held.image);
            bytes -= held.bytes;
        }
    }
    pictures.frame += 1;
}

fn texture(rgba: Vec<u8>, side: u32) -> Image {
    Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// A square picture shrunk to `side` pixels across, or kept at its own side when smaller.
pub(super) fn decode(path: &std::path::Path, side: u32) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let picture = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgba8();
    let own = picture.width().min(picture.height());
    let side = side.clamp(1, own.max(1));
    let rgba = if picture.width() == side && picture.height() == side {
        picture
    } else {
        image::imageops::thumbnail(&picture, side, side)
    };
    Ok(Decoded {
        rgba: rgba.into_raw(),
        side,
    })
}
