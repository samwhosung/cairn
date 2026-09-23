use crate::equipment::{LAYER_TORSO_LOWER, LAYER_TORSO_UPPER};

const GUILD_EMBLEM_DIR: &str = "Textures\\GuildEmblems";

const UNDESIGNED: i32 = -1;

pub(crate) fn emblem_half(layer: usize) -> Option<&'static str> {
    match layer {
        LAYER_TORSO_UPPER => Some("TU"),
        LAYER_TORSO_LOWER => Some("TL"),
        _ => None,
    }
}

/// A guild's tabard design: five indices into the files under `Textures\GuildEmblems`. An index
/// with no file paints nothing, so none is range-checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GuildEmblem {
    pub emblem_style: i32,
    pub emblem_color: i32,
    pub border_style: i32,
    pub border_color: i32,
    pub background_color: i32,
}

impl GuildEmblem {
    /// Whether the guild has designed a tabard: none of the five is `-1`. The client paints no
    /// crest for a guild that has not, keeping the tabard's own art, so pass it as no emblem.
    pub fn is_designed(&self) -> bool {
        [
            self.emblem_style,
            self.emblem_color,
            self.border_style,
            self.border_color,
            self.background_color,
        ]
        .iter()
        .all(|&i| i != UNDESIGNED)
    }
}

/// One of the three layers a guild tabard paints, in the order they are painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmblemLayer {
    Background,
    Border,
    Symbol,
}

impl EmblemLayer {
    pub const ALL: [EmblemLayer; 3] = [
        EmblemLayer::Background,
        EmblemLayer::Border,
        EmblemLayer::Symbol,
    ];

    /// The cell this part takes in each torso layer's row.
    pub fn column(self) -> i8 {
        match self {
            EmblemLayer::Background => 2,
            EmblemLayer::Border => 3,
            EmblemLayer::Symbol => 4,
        }
    }

    /// This layer's file for `emblem` and a tabard half, `"TU"` or `"TL"`. Indices are padded to
    /// two digits, never cut, and `-1` stays `-1`.
    pub fn path(self, emblem: &GuildEmblem, half: &str) -> String {
        match self {
            EmblemLayer::Background => format!(
                "{GUILD_EMBLEM_DIR}\\Background_{:02}_{half}_U.blp",
                emblem.background_color
            ),
            EmblemLayer::Border => format!(
                "{GUILD_EMBLEM_DIR}\\Border_{:02}_{:02}_{half}_U.blp",
                emblem.border_style, emblem.border_color
            ),
            EmblemLayer::Symbol => format!(
                "{GUILD_EMBLEM_DIR}\\Emblem_{:02}_{:02}_{half}_U.blp",
                emblem.emblem_style, emblem.emblem_color
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESIGNED: GuildEmblem = GuildEmblem {
        emblem_style: 7,
        emblem_color: 3,
        border_style: 1,
        border_color: 12,
        background_color: 5,
    };

    const UNDESIGNED: GuildEmblem = GuildEmblem {
        emblem_style: -1,
        emblem_color: -1,
        border_style: -1,
        border_color: -1,
        background_color: -1,
    };

    #[test]
    fn emblem_paths_pad_indices_to_two_digits() {
        assert_eq!(
            EmblemLayer::ALL.map(|p| p.path(&DESIGNED, "TU")),
            [
                "Textures\\GuildEmblems\\Background_05_TU_U.blp",
                "Textures\\GuildEmblems\\Border_01_12_TU_U.blp",
                "Textures\\GuildEmblems\\Emblem_07_03_TU_U.blp",
            ]
        );
        assert_eq!(
            EmblemLayer::Symbol.path(&DESIGNED, "TL"),
            "Textures\\GuildEmblems\\Emblem_07_03_TL_U.blp"
        );
        let wide = GuildEmblem {
            emblem_style: 169,
            emblem_color: 16,
            ..DESIGNED
        };
        assert_eq!(
            EmblemLayer::Symbol.path(&wide, "TU"),
            "Textures\\GuildEmblems\\Emblem_169_16_TU_U.blp"
        );
        assert_eq!(
            EmblemLayer::ALL.map(|p| p.path(&UNDESIGNED, "TU")),
            [
                "Textures\\GuildEmblems\\Background_-1_TU_U.blp",
                "Textures\\GuildEmblems\\Border_-1_-1_TU_U.blp",
                "Textures\\GuildEmblems\\Emblem_-1_-1_TU_U.blp",
            ]
        );
    }

    #[test]
    fn one_minus_one_is_enough_to_have_no_design() {
        assert!(DESIGNED.is_designed());
        assert!(GuildEmblem::default().is_designed());
        assert!(!UNDESIGNED.is_designed());
        for spoil in [
            GuildEmblem {
                emblem_style: -1,
                ..DESIGNED
            },
            GuildEmblem {
                emblem_color: -1,
                ..DESIGNED
            },
            GuildEmblem {
                border_style: -1,
                ..DESIGNED
            },
            GuildEmblem {
                border_color: -1,
                ..DESIGNED
            },
            GuildEmblem {
                background_color: -1,
                ..DESIGNED
            },
        ] {
            assert!(!spoil.is_designed(), "{spoil:?}");
        }
    }
}
