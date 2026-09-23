//! Bounds-checked little-endian reads and the chunk walk the WoW file formats share.

/// Little-endian reads at an offset, `None` when the read would run past the end.
pub trait ByteExt {
    fn u8_at(&self, o: usize) -> Option<u8>;
    fn u16_at(&self, o: usize) -> Option<u16>;
    fn u32_at(&self, o: usize) -> Option<u32>;
    fn i32_at(&self, o: usize) -> Option<i32>;
    fn f32_at(&self, o: usize) -> Option<f32>;
    fn bytes_at(&self, o: usize, n: usize) -> Option<&[u8]>;
}

impl ByteExt for [u8] {
    #[inline]
    fn u8_at(&self, o: usize) -> Option<u8> {
        self.get(o).copied()
    }

    #[inline]
    fn u16_at(&self, o: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            self.get(o..o.checked_add(2)?)?.try_into().ok()?,
        ))
    }

    #[inline]
    fn u32_at(&self, o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.get(o..o.checked_add(4)?)?.try_into().ok()?,
        ))
    }

    #[inline]
    fn i32_at(&self, o: usize) -> Option<i32> {
        self.u32_at(o).map(|v| v as i32)
    }

    #[inline]
    fn f32_at(&self, o: usize) -> Option<f32> {
        self.u32_at(o).map(f32::from_bits)
    }

    #[inline]
    fn bytes_at(&self, o: usize, n: usize) -> Option<&[u8]> {
        self.get(o..o.checked_add(n)?)
    }
}

/// Walks `(magic, payload)` chunks, with the magic as stored: reversed, so `MOHD` reads `b"DHOM"`.
/// Stops at a header cut short, and clamps a payload that claims more than the buffer holds,
/// which some shipped files do.
pub fn chunks(b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos.checked_add(8)? > b.len() {
            return None;
        }
        let magic = [b[pos], b[pos + 1], b[pos + 2], b[pos + 3]];
        let size = b.u32_at(pos + 4)? as usize;
        let start = pos + 8;
        let end = start.saturating_add(size).min(b.len());
        pos = end;
        Some((magic, &b[start..end]))
    })
}

/// How many `elem_size`-byte items to reserve for a declared `count` when `avail` bytes remain,
/// so a corrupt count reserves no more than the input could hold.
pub fn capped(count: usize, elem_size: usize, avail: usize) -> usize {
    count.min(avail / elem_size.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_in_bounds() {
        let b: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(b.u8_at(4), Some(0x05));
        assert_eq!(b.u16_at(0), Some(0x0201));
        assert_eq!(b.u32_at(1), Some(0x0504_0302));
        assert_eq!(b.i32_at(0), Some(0x0403_0201));
        assert_eq!(b.f32_at(0), Some(f32::from_bits(0x0403_0201)));
        assert_eq!(b.bytes_at(2, 3), Some(&b[2..5]));
    }

    #[test]
    fn reads_out_of_bounds_are_none() {
        let b: &[u8] = &[0x01, 0x02, 0x03];
        assert_eq!(b.u8_at(3), None);
        assert_eq!(b.u16_at(2), None);
        assert_eq!(b.u32_at(0), None);
        assert_eq!(b.bytes_at(1, 3), None);
        assert_eq!(b.u32_at(usize::MAX - 1), None);
        assert_eq!(b.bytes_at(usize::MAX, 8), None);
    }

    #[test]
    fn walks_well_formed_chunks() {
        let mut b = Vec::new();
        b.extend_from_slice(b"ABCD");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&[0xAA, 0xBB]);
        b.extend_from_slice(b"EFGH");
        b.extend_from_slice(&0u32.to_le_bytes());
        let got: Vec<_> = chunks(&b).collect();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].0, got[0].1), (*b"ABCD", &[0xAA, 0xBB][..]));
        assert_eq!((got[1].0, got[1].1), (*b"EFGH", &[][..]));
    }

    #[test]
    fn clamps_and_stops_on_short_chunks() {
        let mut b = Vec::new();
        b.extend_from_slice(b"ABCD");
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&[0x01]);
        let got: Vec<_> = chunks(&b).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, &[0x01][..]);
        assert_eq!(chunks(&b[..7]).count(), 0);
        assert_eq!(chunks(&[]).count(), 0);
    }

    #[test]
    fn caps_reservations() {
        assert_eq!(capped(10, 4, 1000), 10);
        assert_eq!(capped(u32::MAX as usize, 48, 96), 2);
        assert_eq!(capped(5, 0, 16), 5);
        assert_eq!(capped(5, 4, 0), 0);
    }
}
