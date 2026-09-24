use crate::Error;
use crate::reader::Reader;

/// What a character looks like: its `ChrRaces` id, 0 male or 1 female, the five customization
/// choices counted from 0 as character creation offers them, and the `ItemDisplayInfo` ids it
/// wears by body slot (head, shoulder, shirt, chest, belt, pants, boots, wrist, gloves, tabard),
/// 0 for an empty slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub equipment: [u32; 10],
}

impl Appearance {
    pub const ENCODED_LEN: usize = 47;

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[
            self.race,
            self.sex,
            self.skin,
            self.face,
            self.hair_style,
            self.hair_color,
            self.facial_hair,
        ]);
        for id in self.equipment {
            out.extend_from_slice(&id.to_le_bytes());
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let dials = r.bytes(7)?;
        let mut equipment = [0; 10];
        for slot in &mut equipment {
            *slot = r.u32()?;
        }
        Ok(Self {
            race: dials[0],
            sex: dials[1],
            skin: dials[2],
            face: dials[3],
            hair_style: dials[4],
            hair_color: dials[5],
            facial_hair: dials[6],
            equipment,
        })
    }
}
