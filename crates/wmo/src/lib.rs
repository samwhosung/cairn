//! Reads World of Warcraft 1.12.1 WMO world objects: the root file and its group files.

mod error;
mod group;
mod liquid;
mod record;
mod root;

pub use error::Error;
pub use group::{Batch, Color, MopyEntry, Uv, Vec3, WmoGroup};
pub use liquid::WmoLiquid;
pub use root::{Material, WmoRoot};

use wowfile::chunks;

/// A WMO file: the root, or one of its groups.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedWmo {
    Root(WmoRoot),
    Group(WmoGroup),
}

/// Reads a root or a group file, whichever the first `MOHD` or `MOGP` chunk says it is.
pub fn parse_wmo(bytes: &[u8]) -> Result<ParsedWmo, Error> {
    for (magic, payload) in chunks(bytes) {
        match &magic {
            b"PGOM" => return group::parse_group(payload).map(ParsedWmo::Group),
            b"DHOM" => return root::parse_root(bytes).map(ParsedWmo::Root),
            _ => {}
        }
    }
    Err(Error::NotWmo)
}

#[cfg(test)]
mod tests;
