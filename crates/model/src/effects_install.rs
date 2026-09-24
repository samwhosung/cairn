use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::particles::{EMITTERS, KeyBudget, emitters_within};
use crate::ribbons::{RIBBONS, ribbons_within};
use crate::{le_u32, parse_m2_particle_emitters, parse_m2_ribbon_emitters};

const EMITTER_SIZE: usize = 0x1f8;
const EMITTER_RATE_TRACK: usize = 0xdc;
const RIBBON_SIZE: usize = 0xdc;
const RIBBON_VISIBILITY_TRACK: usize = 0xc0;
const SEQUENCES: usize = 0x1c;
const SEQUENCE_SIZE: usize = 0x44;
const TRACK_TIMES: usize = 0x0c;
const TRACK_VALUES: usize = 0x14;
const MAX_KEYS_BAKED_PER_BYTE: f64 = 3.0;

struct ModelFile {
    name: String,
    bytes: Vec<u8>,
}

fn counts_emitters_or_ribbons(b: &[u8]) -> bool {
    b.len() >= EMITTERS + 8 && (le_u32(b, RIBBONS) | le_u32(b, EMITTERS)) != 0
}

fn models() -> Option<&'static [ModelFile]> {
    static MODELS: OnceLock<Option<Vec<ModelFile>>> = OnceLock::new();
    MODELS
        .get_or_init(|| {
            let Some(data) = std::env::var_os("WOW_DATA") else {
                eprintln!("skipped: WOW_DATA is not set");
                return None;
            };
            let chain = mpq::Chain::open(data).expect("open the chain");
            Some(
                chain
                    .list()
                    .into_iter()
                    .map(|e| e.name)
                    .filter(|n| n.to_ascii_lowercase().ends_with(".m2"))
                    .filter_map(|name| {
                        Some(ModelFile {
                            bytes: chain.read(&name).ok()?,
                            name,
                        })
                    })
                    .filter(|m| counts_emitters_or_ribbons(&m.bytes))
                    .collect(),
            )
        })
        .as_deref()
}

#[test]
fn shipped_models_read_within_their_key_budget() {
    let Some(models) = models() else {
        return;
    };
    let mut most = (0.0f64, "");
    for m in models {
        let emitters = KeyBudget::one_key_per_byte(&m.bytes);
        let ribbons = KeyBudget::one_key_per_byte(&m.bytes);
        emitters_within(&m.bytes, &emitters);
        ribbons_within(&m.bytes, &ribbons);
        assert!(
            !emitters.refused() && !ribbons.refused(),
            "{} ran out of keys",
            m.name
        );
        for b in [&emitters, &ribbons] {
            let spent = (m.bytes.len() - b.left()) as f64 / m.bytes.len() as f64;
            if spent > most.0 {
                most = (spent, &m.name);
            }
        }
    }
    eprintln!(
        "{} models; the most of its budget one spent: {:.3} ({})",
        models.len(),
        most.0,
        most.1
    );
    assert!(models.len() > 1800);
}

fn keys_baked_sampling(bytes: &[u8]) -> usize {
    const TIMES: [f32; 5] = [0.0, 0.1, 1.0, 7.5, -1.0];
    let mut keys = 0;
    for e in parse_m2_particle_emitters(bytes) {
        for seq in [None, Some(0), Some(1), Some(usize::MAX)] {
            for t in TIMES {
                let now = e.params.sample(seq, t, f64::from(t));
                let _ = (
                    now.lifespan,
                    e.timing.rate(seq, t, 0.0),
                    e.timing.emitting(seq, t, 0.0),
                );
            }
        }
        for u in [0.0, 0.3, 0.5, 1.0, 2.0] {
            let _ = (e.over_life.sample(u), e.twinkle(u));
        }
        if let Some(s) = &e.spline {
            let _ = (s.eval(0.5), s.tangent(0.5));
        }
        let _ = (
            e.params.peak_lifespan(),
            e.timing.peak_rate(),
            e.follow_line(),
        );
        keys += e
            .timing
            .slot_views()
            .iter()
            .map(|(_, r, g)| r.map_or(0, <[_]>::len) + g.map_or(0, <[_]>::len))
            .sum::<usize>();
    }
    for r in parse_m2_ribbon_emitters(bytes) {
        for ms in TIMES.map(|t| t * 1000.0) {
            let _ = (
                r.color.sample_ms(ms),
                r.alpha.sampled_ms(ms),
                r.height_above.sample_ms(ms),
                r.height_below.step_ms(ms),
            );
        }
        if let Some(v) = &r.visible {
            let _ = (v.at(0, 0.5), v.at(153, 1.0));
        }
        keys += r.color.keys.len() + r.alpha.keys.len();
        keys += r.height_above.keys.len() + r.height_below.keys.len();
    }
    keys
}

struct Xorshift(u32);

impl Xorshift {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as usize
    }
}

fn cut_copies(bytes: &[u8]) -> impl Iterator<Item = Vec<u8>> + '_ {
    let len = bytes.len();
    [0, 4, EMITTERS, EMITTERS + 8, len / 4, len / 2, len - 1]
        .into_iter()
        .filter(move |&cut| cut < len)
        .map(|cut| bytes[..cut].to_vec())
}

fn bit_flipped_copies(bytes: &[u8], rng: &mut Xorshift) -> Vec<Vec<u8>> {
    let len = bytes.len();
    (0..8)
        .map(|i| {
            let mut b = bytes.to_vec();
            let span = if i % 2 == 0 {
                len.min(EMITTERS + 0x14)
            } else {
                len
            };
            for _ in 0..=i % 3 {
                let at = rng.next() % span;
                b[at] ^= 1 << (rng.next() % 8);
            }
            b
        })
        .collect()
}

struct Records {
    base: usize,
    count: usize,
    size: usize,
    stretched_track: usize,
}

fn records(bytes: &[u8]) -> Vec<Records> {
    [
        (EMITTERS, EMITTER_SIZE, EMITTER_RATE_TRACK),
        (RIBBONS, RIBBON_SIZE, RIBBON_VISIBILITY_TRACK),
    ]
    .into_iter()
    .filter_map(|(table, size, stretched_track)| {
        let (count, base) = (
            le_u32(bytes, table) as usize,
            le_u32(bytes, table + 4) as usize,
        );
        (count > 0 && base + count * size <= bytes.len()).then_some(Records {
            base,
            count,
            size,
            stretched_track,
        })
    })
    .collect()
}

fn count_bit_20_copies(bytes: &[u8], rng: &mut Xorshift) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for r in records(bytes) {
        for _ in 0..2 {
            let mut b = bytes.to_vec();
            let t = r.base + rng.next() % r.count * r.size + r.stretched_track;
            for word in [SEQUENCES, t + TRACK_TIMES, t + TRACK_VALUES] {
                b[word + 2] ^= 0x10;
            }
            out.push(b);
        }
    }
    out
}

fn stretched_count_copies(bytes: &[u8]) -> Vec<Vec<u8>> {
    let len = bytes.len();
    let fits =
        |at: usize, size: usize| (len.saturating_sub(le_u32(bytes, at) as usize) / size) as u32;
    records(bytes)
        .into_iter()
        .map(|r| {
            let mut b = bytes.to_vec();
            let t = r.base + r.stretched_track;
            let keys = fits(t + TRACK_TIMES + 4, 4).min(fits(t + TRACK_VALUES + 4, 4));
            let sequences = fits(SEQUENCES + 4, SEQUENCE_SIZE);
            for (at, n) in [
                (SEQUENCES, sequences),
                (t + TRACK_TIMES, keys),
                (t + TRACK_VALUES, keys),
            ] {
                b[at..at + 4].copy_from_slice(&n.to_le_bytes());
            }
            b
        })
        .collect()
}

fn damaged_copies(bytes: &[u8], seed: u32) -> Vec<Vec<u8>> {
    let mut rng = Xorshift(seed | 1);
    let mut out: Vec<Vec<u8>> = cut_copies(bytes).collect();
    out.extend(bit_flipped_copies(bytes, &mut rng));
    out.extend(count_bit_20_copies(bytes, &mut rng));
    out.extend(stretched_count_copies(bytes));
    out
}

#[test]
fn damaged_models_read_without_a_panic_or_a_runaway() {
    let Some(models) = models() else {
        return;
    };
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let (copies, most_keys, slowest) = std::thread::scope(|s| {
        let workers: Vec<_> = models
            .chunks(models.len().div_ceil(threads).max(1))
            .enumerate()
            .map(|(w, chunk)| {
                s.spawn(move || {
                    let (mut copies, mut most_keys, mut slowest) = (0, 0.0f64, Duration::ZERO);
                    for (i, m) in chunk.iter().enumerate() {
                        for b in damaged_copies(&m.bytes, (w * 7919 + i) as u32) {
                            let t = Instant::now();
                            let keys = keys_baked_sampling(&b);
                            slowest = slowest.max(t.elapsed());
                            most_keys = most_keys.max(keys as f64 / b.len().max(1) as f64);
                            copies += 1;
                        }
                    }
                    (copies, most_keys, slowest)
                })
            })
            .collect();
        workers
            .into_iter()
            .fold((0, 0.0f64, Duration::ZERO), |acc, w| {
                let (c, k, t) = w.join().expect("a reader panicked");
                (acc.0 + c, acc.1.max(k), acc.2.max(t))
            })
    });
    eprintln!(
        "{copies} damaged copies of {} models: the most keys baked per byte {most_keys:.3}, the \
         slowest read {slowest:?}",
        models.len()
    );
    assert!(copies > 25_000);
    assert!(
        most_keys <= MAX_KEYS_BAKED_PER_BYTE,
        "a copy baked more keys than its file could hold"
    );
    assert!(slowest < Duration::from_secs(10), "a read ran away");
}
