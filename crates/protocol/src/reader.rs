use crate::Error;

#[derive(Clone)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Truncated)?;
        let out = self.bytes.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.bytes(N)?.try_into().map_err(|_| Error::Truncated)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        self.array().map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        self.array().map(u32::from_le_bytes)
    }

    pub(crate) fn f32(&mut self) -> Result<f32, Error> {
        self.array().map(f32::from_le_bytes)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }

    pub(crate) fn finish(&self) -> Result<(), Error> {
        match self.bytes.len().saturating_sub(self.at) {
            0 => Ok(()),
            n => Err(Error::Trailing(n)),
        }
    }
}
