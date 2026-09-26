use crate::zone::{Id, Thing, Water, Z};

pub fn two_places(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

pub fn centi(v: f64) -> i64 {
    (v * 100.0).round() as i64
}

pub fn from_centi(v: i64) -> String {
    two_places(v as f64 / 100.0)
}

pub fn scale_u16(v: f64) -> Result<u16, String> {
    let s = (v * 1024.0).round();
    if (1.0..=65535.0).contains(&s) {
        Ok(s as u16)
    } else {
        Err(format!(
            "scale {v} is outside what the ADT holds (1/1024 to 64)"
        ))
    }
}

/// A scale printed so that it reads back to the same 1/1024.
pub fn scale_text(s: u16) -> String {
    for d in 2..=5 {
        let t = format!("{:.*}", d, f64::from(s) / 1024.0);
        if t.parse().ok().and_then(|v| scale_u16(v).ok()) == Some(s) {
            return t.trim_end_matches('0').trim_end_matches('.').to_owned();
        }
    }
    format!("{}", f64::from(s) / 1024.0)
}

pub fn number<T: std::str::FromStr>(s: &str, what: &str) -> Result<T, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{what}: {s:?} is not a number"))
}

/// An id as `AUTHOR.N`.
pub fn parse_id(s: &str) -> Result<Id, String> {
    let s = s.trim_start_matches('#');
    let (author, n) = s
        .rsplit_once('.')
        .ok_or_else(|| format!("{s:?} is not an id: AUTHOR.N, as `things` lists them"))?;
    crate::zone::check_author(author)?;
    let n: u32 = number(n, "an id's count")?;
    if n == 0 {
        return Err(format!("{s:?} is not an id: counts start at 1"));
    }
    Ok(Id {
        author: author.to_owned(),
        n,
    })
}

/// A thing's line in `things.txt`, after its id.
pub fn thing_text(t: &Thing) -> String {
    let z = match t.z {
        Z::Ground(d) => format!("{}{}", if d < 0 { "" } else { "+" }, from_centi(d)),
        Z::At(h) => format!("={}", from_centi(h)),
    };
    let set = t.set.map_or("-".to_owned(), |v| v.to_string());
    let scattered = t.scattered.map_or("-".to_owned(), |s| s.to_string());
    format!(
        "{} {} {z} {} {} {set} {} {scattered} {}",
        from_centi(t.x),
        from_centi(t.y),
        from_centi(t.facing),
        scale_text(t.scale),
        if t.lean { LEANING } else { UPRIGHT },
        t.model
    )
}

const UPRIGHT: &str = "upright";
const LEANING: &str = "leaning";

pub fn parse_thing(l: &str) -> Result<Thing, String> {
    let mut f = l.splitn(9, char::is_whitespace);
    let mut next = || {
        f.next()
            .ok_or("want `x y z facing scale set stands scattered model`")
    };
    let x = centi(number(next()?, "x")?);
    let y = centi(number(next()?, "y")?);
    let zt = next()?;
    let z = match zt.strip_prefix('=') {
        Some(h) => Z::At(centi(number(h, "z")?)),
        None => Z::Ground(centi(number(zt.trim_start_matches('+'), "z")?)),
    };
    let facing = centi(number(next()?, "facing")?);
    let scale = scale_u16(number(next()?, "scale")?)?;
    let set = match next()? {
        "-" => None,
        s => Some(number(s, "set")?),
    };
    let lean = match next()? {
        UPRIGHT => false,
        LEANING => true,
        s => return Err(format!("{s:?}: a thing stands {UPRIGHT} or {LEANING}")),
    };
    let scattered = match next()? {
        "-" => None,
        s => Some(number(s, "a scatter's seed")?),
    };
    let model = next()?.trim().to_owned();
    if model.is_empty() {
        return Err("no model".into());
    }
    Ok(Thing {
        model,
        x,
        y,
        z,
        facing,
        scale,
        set,
        lean,
        scattered,
    })
}

/// A body of water's line in `water.txt`, after its id.
pub fn water_text(w: &Water) -> String {
    format!("water {} {}", from_centi(w.level), w.shape.describe())
}

pub fn parse_water(l: &str) -> Result<Water, String> {
    let f: Vec<String> = l.split_whitespace().map(str::to_owned).collect();
    if f.len() < 3 || f[0] != "water" {
        return Err("want `water level outline`".into());
    }
    Ok(Water {
        level: centi(number(&f[1], "level")?),
        shape: crate::grammar::shape(&f[2..])?,
    })
}

/// Lines with their numbers, blank lines and `#` notes left out.
pub fn lines(s: &str) -> impl Iterator<Item = (usize, &str)> {
    s.lines().enumerate().filter_map(|(n, l)| {
        let l = l.trim();
        (!l.is_empty() && !l.starts_with('#')).then_some((n + 1, l))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_print_back_to_themselves() {
        for s in 1..=4096u16 {
            let back = scale_text(s).parse().ok().and_then(|v| scale_u16(v).ok());
            assert_eq!(back, Some(s));
        }
        assert_eq!(scale_text(1024), "1");
        assert_eq!(scale_text(1126), "1.1");
    }

    #[test]
    fn things_and_ids_read_back() {
        let t = Thing {
            model: "World\\A B\\Tree.m2".into(),
            x: 12_345,
            y: -5,
            z: Z::Ground(-40),
            facing: 35_999,
            scale: 1126,
            set: None,
            lean: true,
            scattered: Some(u64::MAX),
        };
        assert_eq!(parse_thing(&thing_text(&t)), Ok(t.clone()));
        let placed = Thing {
            lean: false,
            scattered: None,
            ..t
        };
        assert_eq!(parse_thing(&thing_text(&placed)), Ok(placed));
        let id = parse_id("#sam.12").expect("an id");
        assert_eq!(id.to_string(), "sam.12");
        assert!(parse_id("sam.0").is_err() && parse_id("12").is_err());
    }
}
