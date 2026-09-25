use std::collections::HashMap;
use std::sync::Arc;

use mpq::Chain;

use crate::raster::{Axis, EyeFrame, Grid, Vert, clip_near, twice_signed_area};
use crate::{Error, M2PortraitCamera, ModelBlend, RenderSubmesh};

/// How far a scene's art paints around its camera, as half-extents in tan units (the space of
/// `tan(fov / 2)`).
///
/// `half_w` is how far every row across the authored 4:3 vertical opening stays painted on both
/// sides of the axis, and `half_h` the same for the columns across the horizontal opening. `0.0`
/// when a scanline is unpainted at the axis itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtExtent {
    pub half_w: f32,
    pub half_h: f32,
}

/// The aspect the glue scenes are framed for.
pub const GLUE_AUTHORED_ASPECT: f32 = 4.0 / 3.0;

/// The client's alpha-key reference: an `AlphaTest` texel passes at `alpha >= 224`.
pub const ALPHA_KEY_REF: u8 = 224;

const DEG: f32 = std::f32::consts::PI / 180.0;

/// A shipped glue scene: its token (`UI_<token>.m2`), camera 0's field of view, and its measured
/// [`ArtExtent`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShippedGlueScene {
    pub token: &'static str,
    pub fov: f32,
    pub art: ArtExtent,
}

/// The seven shipped glue scenes, as [`glue_art_extent`] measures them with a [`CoverageReader`].
pub const SHIPPED_GLUE_SCENES: [ShippedGlueScene; 7] = [
    ShippedGlueScene {
        token: "MainMenu",
        fov: 86.0 * DEG,
        art: ArtExtent {
            half_w: 0.7431,
            half_h: 0.5067,
        },
    },
    ShippedGlueScene {
        token: "Human",
        fov: 80.0 * DEG,
        art: ArtExtent {
            half_w: 0.6548,
            half_h: 0.4866,
        },
    },
    ShippedGlueScene {
        token: "Orc",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5573,
            half_h: 0.3954,
        },
    },
    ShippedGlueScene {
        token: "Dwarf",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5040,
            half_h: 0.4272,
        },
    },
    ShippedGlueScene {
        token: "NightElf",
        fov: 60.0 * DEG,
        art: ArtExtent {
            half_w: 0.4262,
            half_h: 0.1221,
        },
    },
    ShippedGlueScene {
        token: "Scourge",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5542,
            half_h: 0.4776,
        },
    },
    ShippedGlueScene {
        token: "Tauren",
        fov: 65.0 * DEG,
        art: ArtExtent {
            half_w: 0.5177,
            half_h: 0.4066,
        },
    },
];

/// The measured [`ArtExtent`] of a shipped scene by token; `None` for any other token.
pub fn shipped_glue_art_extent(token: &str) -> Option<ArtExtent> {
    SHIPPED_GLUE_SCENES
        .iter()
        .find(|s| s.token == token)
        .map(|s| s.art)
}

/// The vertical half-extent (tan units) of a glue camera's diagonal `fov` at 4:3:
/// `tan(fovy / 2)` with `fovy = fov / √((4/3)² + 1)`.
pub fn authored_half_height(fov: f32) -> f32 {
    (fov / (GLUE_AUTHORED_ASPECT * GLUE_AUTHORED_ASPECT + 1.0).sqrt() * 0.5).tan()
}

/// How a batch paints the pixels it covers.
#[derive(Clone, Debug)]
pub enum Coverage {
    /// Every covered pixel.
    Full,
    /// Where the texture's alpha, sampled through the batch's UVs, is at least [`ALPHA_KEY_REF`].
    Alpha(Arc<AlphaMap>),
}

/// A texture's alpha channel, row-major, one byte per texel.
#[derive(Clone, Debug)]
pub struct AlphaMap {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

impl AlphaMap {
    /// The texel's alpha at `(u, v)`, the texture repeating along an axis that wraps and clamped
    /// along one that does not.
    pub fn sample(&self, u: f32, v: f32, wrap_x: bool, wrap_y: bool) -> u8 {
        let axis = |t: f32, n: u32, wrap: bool| -> u32 {
            let t = if wrap {
                t - t.floor()
            } else {
                t.clamp(0.0, 1.0)
            };
            ((t * n as f32) as u32).min(n - 1)
        };
        if self.width == 0 || self.height == 0 {
            return 0;
        }
        let x = axis(u, self.width, wrap_x);
        let y = axis(v, self.height, wrap_y);
        self.alpha[(y * self.width + x) as usize]
    }
}

fn read_texture_rgba(chain: &Chain, path: &str) -> Result<(u32, u32, Vec<u8>), Error> {
    let bytes = chain.read(&path.replace('/', "\\")).map_err(Error::Chain)?;
    let level0 = blp::decode(&bytes)
        .map_err(Error::Blp)?
        .mips
        .into_iter()
        .next()
        .expect("a decoded texture has level 0");
    Ok((level0.width, level0.height, level0.rgba))
}

/// Finds each batch's [`Coverage`] on the chain, reading each texture once: `Opaque` paints in
/// full, `Blend` and `AlphaTest` by their texture's alpha (nothing when untextured), and `Mod`
/// and `Mod2x` paint nothing.
pub struct CoverageReader<'c> {
    chain: &'c Chain,
    cache: HashMap<String, Option<Coverage>>,
}

impl<'c> CoverageReader<'c> {
    pub fn new(chain: &'c Chain) -> Self {
        Self {
            chain,
            cache: HashMap::new(),
        }
    }

    /// How `sub` paints; `None` when it paints nothing, as over a texture wholly below the key.
    pub fn coverage(&mut self, sub: &RenderSubmesh) -> Result<Option<Coverage>, Error> {
        match sub.blend {
            ModelBlend::Opaque => Ok(Some(Coverage::Full)),
            ModelBlend::Mod | ModelBlend::Mod2x => Ok(None),
            ModelBlend::AlphaTest | ModelBlend::Blend => {
                let Some(path) = sub.texture.as_deref() else {
                    return Ok(None);
                };
                if let Some(known) = self.cache.get(path) {
                    return Ok(known.clone());
                }
                let (width, height, rgba) = read_texture_rgba(self.chain, path)?;
                let alpha: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|px| px[3]).collect();
                let cov = if alpha.iter().all(|&a| a >= ALPHA_KEY_REF) {
                    Some(Coverage::Full)
                } else if alpha.iter().all(|&a| a < ALPHA_KEY_REF) {
                    None
                } else {
                    Some(Coverage::Alpha(Arc::new(AlphaMap {
                        width,
                        height,
                        alpha,
                    })))
                };
                self.cache.insert(path.to_string(), cov.clone());
                Ok(cov)
            }
        }
    }
}

/// Measures a scene's [`ArtExtent`] from its batches, its camera, and how each batch paints.
/// Triangles are clipped at the camera's near plane, and the back faces (clockwise on screen) of a
/// single-sided batch paint nothing.
pub fn glue_art_extent<'a>(
    subs: impl IntoIterator<Item = &'a RenderSubmesh>,
    cam: &M2PortraitCamera,
    mut coverage: impl FnMut(&RenderSubmesh) -> Option<Coverage>,
) -> ArtExtent {
    let t0 = authored_half_height(cam.fov);
    let h0 = t0 * GLUE_AUTHORED_ASPECT;
    let mut grid = Grid::new(t0);
    if let Some(frame) = EyeFrame::of(cam) {
        for s in subs {
            if let Some(cov) = coverage(s) {
                grid.paint_batch(s, &frame, cam.near_clip.max(1e-3), &cov);
            }
        }
    }
    ArtExtent {
        half_w: grid.half_extent(Axis::X, t0),
        half_h: grid.half_extent(Axis::Y, h0),
    }
}

/// Where one batch lands in the camera's tan space, counted in triangles cut at the near plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchFootprint {
    /// Triangles facing the camera, or of a two-sided batch.
    pub front: usize,
    /// Triangles of a single-sided batch facing away.
    pub back: usize,
    /// Triangles wholly behind the near plane.
    pub clipped: usize,
    /// The front triangles' `x` range; `None` without one.
    pub x: Option<(f32, f32)>,
    /// The front triangles' `y` range; `None` without one.
    pub y: Option<(f32, f32)>,
}

/// One batch's [`BatchFootprint`] under `cam`, however it paints.
pub fn batch_footprint(sub: &RenderSubmesh, cam: &M2PortraitCamera) -> BatchFootprint {
    let mut fp = BatchFootprint {
        front: 0,
        back: 0,
        clipped: 0,
        x: None,
        y: None,
    };
    let Some(frame) = EyeFrame::of(cam) else {
        return fp;
    };
    let near = cam.near_clip.max(1e-3);
    for tri in sub.indices.as_chunks::<3>().0 {
        let Some(eye) = tri
            .iter()
            .map(|&i| {
                sub.positions
                    .get(i as usize)
                    .map(|&p| (frame.to_eye(p), [0.0; 2]))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let pieces = clip_near(&eye, near);
        if pieces.is_empty() {
            fp.clipped += 1;
            continue;
        }
        for piece in pieces {
            let proj: [Vert; 3] = piece.map(|((x, y, z), _)| Vert {
                x: x / z,
                y: y / z,
                inv_z: 1.0 / z,
                u_z: 0.0,
                v_z: 0.0,
            });
            if !sub.two_sided && twice_signed_area(&proj) <= 0.0 {
                fp.back += 1;
                continue;
            }
            fp.front += 1;
            for p in proj {
                fp.x = Some(fp.x.map_or((p.x, p.x), |(lo, hi)| (lo.min(p.x), hi.max(p.x))));
                fp.y = Some(fp.y.map_or((p.y, p.y), |(lo, hi)| (lo.min(p.y), hi.max(p.y))));
            }
        }
    }
    fp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::{CELLS, X_SPAN};

    fn cam(fov: f32) -> M2PortraitCamera {
        M2PortraitCamera {
            fov,
            far_clip: 100.0,
            near_clip: 0.1,
            position: [0.0, 0.0, 0.0],
            target: [1.0, 0.0, 0.0],
            roll: 0.0,
        }
    }

    fn card(d: f32, w: f32, h: f32, blend: ModelBlend, two_sided: bool) -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![[d, w, -h], [d, -w, -h], [d, -w, h], [d, w, h]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            blend,
            two_sided,
            ..Default::default()
        }
    }

    const FOV: f32 = 1.0;
    const TOL: f32 = 2.0 * 2.0 * X_SPAN * 0.31 / CELLS as f32;

    #[allow(clippy::unnecessary_wraps)]
    fn full(_: &RenderSubmesh) -> Option<Coverage> {
        Some(Coverage::Full)
    }

    fn band(lo: f32, hi: f32) -> Arc<AlphaMap> {
        let (w, h) = (64u32, 16u32);
        let alpha = (0..w * h)
            .map(|i| {
                let u = (i % w) as f32 / w as f32;
                if (lo..hi).contains(&u) {
                    255
                } else {
                    ALPHA_KEY_REF - 1
                }
            })
            .collect();
        Arc::new(AlphaMap {
            width: w,
            height: h,
            alpha,
        })
    }

    #[test]
    fn a_wide_card_reports_its_own_half_extents() {
        let sub = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        assert!((ext.half_w - 0.5).abs() < TOL, "half_w {}", ext.half_w);
        assert!((ext.half_h - 0.4).abs() < TOL, "half_h {}", ext.half_h);
    }

    #[test]
    fn a_card_narrower_than_the_authored_box_reports_zero() {
        let sub = card(10.0, 2.0, 2.0, ModelBlend::Opaque, false);
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        assert_eq!(
            ext,
            ArtExtent {
                half_w: 0.0,
                half_h: 0.0
            }
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn backfaces_are_not_coverage_unless_two_sided() {
        let mut back = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        back.indices.reverse();
        assert_eq!(glue_art_extent([&back], &cam(FOV), full).half_w, 0.0);
        back.two_sided = true;
        assert!((glue_art_extent([&back], &cam(FOV), full).half_w - 0.5).abs() < TOL);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_batch_that_counts_for_nothing_paints_nothing() {
        let sub = card(10.0, 5.0, 4.0, ModelBlend::Blend, false);
        assert_eq!(glue_art_extent([&sub], &cam(FOV), |_| None).half_w, 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn an_alpha_texture_paints_only_where_it_passes_the_key() {
        let sub = card(10.0, 5.0, 4.0, ModelBlend::AlphaTest, false);
        let map = band(0.2, 0.8);
        let ext = glue_art_extent([&sub], &cam(FOV), |_| Some(Coverage::Alpha(map.clone())));
        assert!(
            (ext.half_w - 0.3).abs() < TOL + 1.0 / 64.0,
            "half_w {}",
            ext.half_w
        );
        assert_eq!(ext.half_h, 0.0, "a column inside the box is unpainted");
        let map = band(0.05, 0.95);
        let ext = glue_art_extent([&sub], &cam(FOV), |_| Some(Coverage::Alpha(map.clone())));
        assert!((ext.half_h - 0.4).abs() < TOL, "half_h {}", ext.half_h);
        assert!(
            (ext.half_w - 0.45).abs() < TOL + 1.0 / 64.0,
            "half_w {}",
            ext.half_w
        );
    }

    #[test]
    fn the_narrowest_row_wins() {
        let mut sub = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        sub.positions[2] = [10.0, -2.5, 4.0];
        sub.positions[3] = [10.0, 2.5, 4.0];
        let ext = glue_art_extent([&sub], &cam(FOV), full);
        let t0 = authored_half_height(FOV);
        let expect = (5.0 - 2.5 * (t0 * 10.0 + 4.0) / 8.0) / 10.0;
        assert!(
            (ext.half_w - expect).abs() < 3e-3,
            "{} vs {expect}",
            ext.half_w
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_card_behind_the_camera_is_clipped_away_and_one_straddling_it_is_clipped_to_near() {
        let behind = card(-10.0, 5.0, 4.0, ModelBlend::Opaque, true);
        assert_eq!(glue_art_extent([&behind], &cam(FOV), full).half_w, 0.0);
        let mut ground = card(0.0, 50.0, 0.0, ModelBlend::Opaque, true);
        ground.positions = vec![
            [-5.0, 50.0, -1.0],
            [-5.0, -50.0, -1.0],
            [50.0, -50.0, -1.0],
            [50.0, 50.0, -1.0],
        ];
        let ext = glue_art_extent([&ground], &cam(FOV), full);
        assert!(ext.half_w.is_finite() && ext.half_h.is_finite());
        assert_eq!(ext.half_w, 0.0);
    }

    #[test]
    fn adjacent_cards_paint_one_run_across_their_shared_edge() {
        let mut left = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        left.positions = vec![
            [10.0, 5.0, -4.0],
            [10.0, 0.0, -4.0],
            [10.0, 0.0, 4.0],
            [10.0, 5.0, 4.0],
        ];
        let mut right = card(10.0, 5.0, 4.0, ModelBlend::Opaque, false);
        right.positions = vec![
            [10.0, 0.0, -4.0],
            [10.0, -5.0, -4.0],
            [10.0, -5.0, 4.0],
            [10.0, 0.0, 4.0],
        ];
        let ext = glue_art_extent([&left, &right], &cam(FOV), full);
        assert!((ext.half_w - 0.5).abs() < TOL, "half_w {}", ext.half_w);
    }
}
