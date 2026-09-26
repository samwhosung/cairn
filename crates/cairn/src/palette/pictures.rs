use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

const GPU_BUDGET_BYTES: usize = 32 << 20;
const DECODING_AT_ONCE: usize = 8;

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

    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    pub fn held_bytes(&self) -> usize {
        self.held.values().map(|h| h.bytes).sum()
    }

    pub fn side(&self) -> u32 {
        self.side
    }

    fn switch_side(&mut self, images: &mut Assets<Image>, textures: &mut EguiUserTextures) {
        if self.wanted_side == self.side {
            return;
        }
        self.side = self.wanted_side;
        self.decoding.clear();
        self.failed.clear();
        for (_, held) in std::mem::take(&mut self.held) {
            let_go(&held, images, textures);
        }
    }

    fn take_decoded(&mut self, images: &mut Assets<Image>, textures: &mut EguiUserTextures) {
        let done: Vec<(usize, Result<Decoded, String>)> = self
            .decoding
            .iter_mut()
            .filter_map(|(&item, task)| block_on(poll_once(task)).map(|done| (item, done)))
            .collect();
        for (item, decoded) in done {
            self.decoding.remove(&item);
            match decoded {
                Ok(decoded) => {
                    let image = images.add(texture(decoded.rgba, decoded.side));
                    let id = textures.add_image(EguiTextureHandle::Strong(image.clone()));
                    let bytes = (decoded.side * decoded.side * 4) as usize;
                    let shown = self.frame;
                    let held = Held {
                        image,
                        id,
                        bytes,
                        shown,
                    };
                    self.held.insert(item, held);
                }
                Err(e) => {
                    warn!("the palette's picture: {e}");
                    self.failed.insert(item);
                }
            }
        }
    }

    fn start_decoding(&mut self) {
        let side = self.side;
        let pool = AsyncComputeTaskPool::get();
        for (item, path) in &self.wanted {
            let known = self.held.contains_key(item) || self.failed.contains(item);
            if known || self.decoding.contains_key(item) {
                continue;
            }
            if self.decoding.len() >= DECODING_AT_ONCE {
                break;
            }
            let path = path.clone();
            let task = pool.spawn(async move { decode(&path, side) });
            self.decoding.insert(*item, task);
        }
    }

    fn evict(&mut self, images: &mut Assets<Image>, textures: &mut EguiUserTextures) {
        let mut by_age: Vec<(u64, usize)> = self.held.iter().map(|(&i, h)| (h.shown, i)).collect();
        by_age.sort_unstable();
        let mut bytes = self.held_bytes();
        for (shown, item) in by_age {
            if bytes <= GPU_BUDGET_BYTES || shown >= self.frame {
                break;
            }
            if let Some(held) = self.held.remove(&item) {
                let_go(&held, images, textures);
                bytes -= held.bytes;
            }
        }
    }
}

pub fn load(
    mut pictures: ResMut<'_, Pictures>,
    mut images: ResMut<'_, Assets<Image>>,
    mut textures: ResMut<'_, EguiUserTextures>,
) {
    let (images, textures) = (&mut *images, &mut *textures);
    pictures.switch_side(images, textures);
    pictures.take_decoded(images, textures);
    pictures.start_decoding();
    pictures.evict(images, textures);
    pictures.frame += 1;
}

fn let_go(held: &Held, images: &mut Assets<Image>, textures: &mut EguiUserTextures) {
    textures.remove_image(&held.image);
    images.remove(&held.image);
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
