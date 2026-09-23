use std::collections::HashMap;

use dbc::{FieldType, Record, Schema, SchemaField};
use mpq::Chain;

use crate::Error;
use crate::table::{f32_at, read, u32_at};

const INT_BAND: &str = "DBFilesClient\\LightIntBand.dbc";
const FLOAT_BAND: &str = "DBFilesClient\\LightFloatBand.dbc";

pub(crate) const HALF_MINUTES_PER_DAY: u32 = 2880;

const MAX_KEYS: usize = 16;
const KEY_COUNT_COLUMN: usize = 1;
const FIRST_TIME_COLUMN: usize = 2;
const FIRST_VALUE_COLUMN: usize = FIRST_TIME_COLUMN + MAX_KEYS;

pub(crate) struct Band<T> {
    times: Vec<u32>,
    values: Vec<T>,
}

fn band_schema(name: &str, value: FieldType) -> Schema {
    let mut s = Schema::new(name);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("num", FieldType::UInt32));
    for i in 0..MAX_KEYS {
        s.add_field(SchemaField::new(format!("time{i}"), FieldType::UInt32));
    }
    for i in 0..MAX_KEYS {
        s.add_field(SchemaField::new(format!("value{i}"), value));
    }
    s
}

fn decode_color(v: u32) -> [f32; 3] {
    [
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    ]
}

struct Segment {
    from: usize,
    to: usize,
    frac: f32,
}

fn segment(times: &[u32], t: u32) -> Segment {
    let n = times.len();
    if n == 1 {
        return Segment {
            from: 0,
            to: 0,
            frac: 0.0,
        };
    }
    if t < times[0] || t >= times[n - 1] {
        let span = times[0] + HALF_MINUTES_PER_DAY - times[n - 1];
        if span == 0 {
            return Segment {
                from: n - 1,
                to: 0,
                frac: 0.0,
            };
        }
        let into = if t < times[0] {
            t + HALF_MINUTES_PER_DAY
        } else {
            t
        } - times[n - 1];
        return Segment {
            from: n - 1,
            to: 0,
            frac: into as f32 / span as f32,
        };
    }
    for i in 0..n - 1 {
        if t <= times[i + 1] {
            let span = times[i + 1] - times[i];
            let frac = if span == 0 {
                0.0
            } else {
                (t - times[i]) as f32 / span as f32
            };
            return Segment {
                from: i,
                to: i + 1,
                frac,
            };
        }
    }
    Segment {
        from: n - 1,
        to: n - 1,
        frac: 0.0,
    }
}

pub(crate) fn sample_float(b: &Band<f32>, t: u32) -> Option<f32> {
    if b.values.is_empty() {
        return None;
    }
    let s = segment(&b.times, t);
    let (v0, v1) = (b.values[s.from], b.values[s.to]);
    Some(v0 + (v1 - v0) * s.frac)
}

pub(crate) fn sample_color(b: &Band<u32>, t: u32) -> Option<[f32; 3]> {
    if b.values.is_empty() {
        return None;
    }
    let s = segment(&b.times, t);
    let (c0, c1) = (decode_color(b.values[s.from]), decode_color(b.values[s.to]));
    Some([
        c0[0] + (c1[0] - c0[0]) * s.frac,
        c0[1] + (c1[1] - c0[1]) * s.frac,
        c0[2] + (c1[2] - c0[2]) * s.frac,
    ])
}

fn load<T>(
    chain: &Chain,
    table: &'static str,
    schema: Schema,
    value_at: fn(&Record, usize) -> Option<T>,
) -> Result<HashMap<u32, Band<T>>, Error> {
    let rs = read(chain, table, schema)?;
    let mut m = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(num)) = (u32_at(r, 0), u32_at(r, KEY_COUNT_COLUMN)) else {
            continue;
        };
        let num = (num as usize).min(MAX_KEYS);
        let times = (0..num)
            .filter_map(|i| u32_at(r, FIRST_TIME_COLUMN + i))
            .collect();
        let values = (0..num)
            .filter_map(|i| value_at(r, FIRST_VALUE_COLUMN + i))
            .collect();
        m.insert(id, Band { times, values });
    }
    Ok(m)
}

pub(crate) fn load_int_bands(chain: &Chain) -> Result<HashMap<u32, Band<u32>>, Error> {
    load(
        chain,
        INT_BAND,
        band_schema("LightIntBand", FieldType::UInt32),
        u32_at,
    )
}

pub(crate) fn load_float_bands(chain: &Chain) -> Result<HashMap<u32, Band<f32>>, Error> {
    load(
        chain,
        FLOAT_BAND,
        band_schema("LightFloatBand", FieldType::Float32),
        f32_at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_wraps_across_midnight() {
        let s = segment(&[720, 2160], 360);
        assert_eq!((s.from, s.to), (1, 0));
        let span = 720 + HALF_MINUTES_PER_DAY - 2160;
        let into = 360 + HALF_MINUTES_PER_DAY - 2160;
        assert!((s.frac - into as f32 / span as f32).abs() < 1e-6);
    }

    #[test]
    fn keys_interpolate_linearly_between_them() {
        let band = Band {
            times: vec![0, 1440],
            values: vec![0.0f32, 1.0],
        };
        assert_eq!(sample_float(&band, 720), Some(0.5));
        assert_eq!(sample_float(&band, 2160), Some(0.5));
        let empty: Band<f32> = Band {
            times: vec![],
            values: vec![],
        };
        assert_eq!(sample_float(&empty, 720), None);
    }

    #[test]
    fn colours_are_rgb_with_blue_low() {
        let c = decode_color(0x00ff_8000).map(|v| (v * 255.0).round() as u8);
        assert_eq!(c, [255, 128, 0]);
    }
}
