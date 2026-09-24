use server::Spawn;

use crate::ground::Ground;

const ELWYNN_AREA_ID: u32 = 12;
const GOLDSHIRE: [f32; 2] = [-9439.1, 51.2];

#[derive(Clone)]
pub struct XorShift64Star(u64);

impl XorShift64Star {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32)
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.range(0.0, 1.0) < p
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Region {
    Disk {
        centre: [f32; 2],
        radius: f32,
    },
    /// Anywhere in the zone `area_id`, which must lie inside the box `lo..hi`.
    Zone {
        area_id: u32,
        lo: [f32; 2],
        hi: [f32; 2],
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub name: &'static str,
    pub region: Region,
    pub tiles_x: (u32, u32),
    pub tiles_y: (u32, u32),
    pub leg_reach: f32,
}

/// The square within `reach` yards of `centre` on each axis.
#[derive(Clone, Copy, Debug)]
pub struct Around {
    pub centre: [f32; 2],
    pub reach: f32,
}

pub fn scenario(name: &str) -> Option<Scenario> {
    match name {
        "goldshire" => Some(Scenario {
            name: "goldshire",
            region: Region::Disk {
                centre: GOLDSHIRE,
                radius: 50.0,
            },
            tiles_x: (30, 32),
            tiles_y: (48, 50),
            leg_reach: 60.0,
        }),
        "elwynn" => Some(Scenario {
            name: "elwynn",
            region: Region::Zone {
                area_id: ELWYNN_AREA_ID,
                lo: [-10_200.0, -1734.0],
                hi: [-8000.0, 1067.0],
            },
            tiles_x: (30, 35),
            tiles_y: (47, 51),
            leg_reach: 150.0,
        }),
        _ => None,
    }
}

impl Region {
    pub fn contains(&self, ground: &Ground, p: [f32; 2]) -> bool {
        let inside = match *self {
            Self::Disk { centre, radius } => (p[0] - centre[0]).hypot(p[1] - centre[1]) <= radius,
            Self::Zone { area_id, .. } => ground.zone(p[0], p[1]) == Some(area_id),
        };
        inside && ground.height(p[0], p[1]).is_some()
    }

    pub fn sample(
        &self,
        ground: &Ground,
        rng: &mut XorShift64Star,
        around: Option<Around>,
    ) -> Option<[f32; 2]> {
        let (lo, hi) = match (*self, around) {
            (
                _,
                Some(Around {
                    centre: c,
                    reach: r,
                }),
            ) => ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
            (Self::Disk { centre, radius }, None) => (
                [centre[0] - radius, centre[1] - radius],
                [centre[0] + radius, centre[1] + radius],
            ),
            (Self::Zone { lo, hi, .. }, None) => (lo, hi),
        };
        (0..200).find_map(|_| {
            let p = [rng.range(lo[0], hi[0]), rng.range(lo[1], hi[1])];
            self.contains(ground, p).then_some(p)
        })
    }
}

pub fn spawns(
    s: &Scenario,
    ground: &Ground,
    count: usize,
    seed: u64,
) -> Result<Vec<Spawn>, String> {
    let mut rng = XorShift64Star::new(seed);
    (0..count)
        .map(|_| {
            let p = s
                .region
                .sample(ground, &mut rng, None)
                .ok_or(format!("no ground found in {}", s.name))?;
            Ok(Spawn {
                pos: [p[0], p[1], ground.height(p[0], p[1]).unwrap_or(0.0)],
                facing: rng.range(0.0, std::f32::consts::TAU),
            })
        })
        .collect()
}
