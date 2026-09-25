use std::fmt::Debug;

/// One `key = value` line of a knobs file, and where it stands as `file:line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub key: String,
    pub value: String,
    pub at: String,
}

/// The lines of a knobs file's text, `#` starting a comment; a key set twice is an error.
pub fn lines(text: &str, file: &str) -> Result<Vec<Line>, String> {
    let mut out: Vec<Line> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let at = format!("{file}:{}", i + 1);
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{at}: want `key = value`"));
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() {
            return Err(format!("{at}: no key before `=`"));
        }
        if let Some(first) = out.iter().find(|l| l.key == key) {
            return Err(format!("{at}: `{key}` again: it was set at {}", first.at));
        }
        out.push(Line {
            key: key.into(),
            value: value.into(),
            at,
        });
    }
    Ok(out)
}

/// A game's knobs, as [`knobs!`](crate::knobs) declares them.
pub trait Knobs: Clone + Debug + Send + Sync + 'static {
    /// The knobs `base` sets, which must be every one, with `over` laid on them in order.
    fn read(base: &[Line], base_file: &str, over: &[Line]) -> Result<Self, String>;
}

/// A value a knob may hold.
pub trait Knob: Sized {
    fn parse(value: &str) -> Result<Self, String>;
}

macro_rules! whole {
    ($($t:ty),*) => {$(
        impl Knob for $t {
            fn parse(value: &str) -> Result<Self, String> {
                value.parse().map_err(|_| format!("`{value}` is not a whole number"))
            }
        }
    )*};
}

whole!(u8, u16, u32, u64, i32, i64);

impl Knob for f32 {
    fn parse(value: &str) -> Result<Self, String> {
        value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or(format!("`{value}` is not a number"))
    }
}

impl Knob for bool {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "yes" | "true" => Ok(true),
            "no" | "false" => Ok(false),
            _ => Err(format!("`{value}` is not yes or no")),
        }
    }
}

/// `never`, or a value.
impl<T: Knob> Knob for Option<T> {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "never" => Ok(None),
            v => T::parse(v).map(Some),
        }
    }
}

/// Declares a game's knobs: a struct whose fields are the keys of its knobs files.
#[macro_export]
macro_rules! knobs {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $($(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        $vis struct $name {
            $($(#[$fmeta])* $fvis $field: $ty),*
        }

        impl $crate::Knobs for $name {
            fn read(
                base: &[$crate::Line],
                base_file: &str,
                over: &[$crate::Line],
            ) -> ::std::result::Result<Self, ::std::string::String> {
                $(let mut $field: ::std::option::Option<$ty> = ::std::option::Option::None;)*
                for (lines, is_base) in [(base, true), (over, false)] {
                    for line in lines {
                        match line.key.as_str() {
                            $(::std::stringify!($field) => {
                                let v = <$ty as $crate::Knob>::parse(&line.value).map_err(|e| {
                                    ::std::format!("{}: `{}`: {e}", line.at, line.key)
                                })?;
                                $field = ::std::option::Option::Some(v);
                            })*
                            _ => {
                                return ::std::result::Result::Err(::std::format!(
                                    "{}: `{}` is not a knob of this game",
                                    line.at,
                                    line.key
                                ));
                            }
                        }
                    }
                    if is_base {
                        $(if $field.is_none() {
                            return ::std::result::Result::Err(::std::format!(
                                "{base_file}: sets no `{}`",
                                ::std::stringify!($field)
                            ));
                        })*
                    }
                }
                ::std::result::Result::Ok(Self {
                    $($field: $field.ok_or_else(|| ::std::stringify!($field).to_owned())?,)*
                })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::knobs! {
        struct Test {
            health: u32,
            reach: f32,
            rises_after_s: Option<u32>,
        }
    }

    fn read(base: &str, over: &str) -> Result<Test, String> {
        let base = lines(base, "base.knobs")?;
        let over = lines(over, "over.knobs")?;
        Test::read(&base, "base.knobs", &over)
    }

    #[test]
    fn an_overlay_changes_what_it_names_and_nothing_else() {
        let base = "health = 100 # full\nreach = 5\n\nrises_after_s = 10\n";
        let t = read(base, "rises_after_s = never\n").expect("knobs");
        assert_eq!(
            t,
            Test {
                health: 100,
                reach: 5.0,
                rises_after_s: None
            }
        );
    }

    #[test]
    fn a_fault_names_its_file_and_line() {
        let base = "health = 100\nreach = 5\nrises_after_s = 10\n";
        let fault = |base: &str, over: &str| read(base, over).expect_err("a fault");
        assert_eq!(
            fault(base, "\nspeed = 3\n"),
            "over.knobs:2: `speed` is not a knob of this game"
        );
        assert_eq!(
            fault(base, "reach = far\n"),
            "over.knobs:1: `reach`: `far` is not a number"
        );
        assert_eq!(fault("health = 1\n", ""), "base.knobs: sets no `reach`");
        assert_eq!(
            fault(base, "reach = 1\nreach = 2\n"),
            "over.knobs:2: `reach` again: it was set at over.knobs:1"
        );
        assert_eq!(fault(base, "reach 1\n"), "over.knobs:1: want `key = value`");
    }
}
