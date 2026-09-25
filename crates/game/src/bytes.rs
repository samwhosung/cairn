/// How a row's sent or saved fields travel: little-endian numbers, a bool as one byte and an
/// option as a byte and then its value, a tuple field by field.
pub trait Bytes: Sized {
    fn put(&self, out: &mut Vec<u8>);

    /// Reads one value off the front of `input`; `None` when it does not hold one.
    fn take(input: &mut &[u8]) -> Option<Self>;

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.put(&mut out);
        out
    }

    /// `None` unless `bytes` hold exactly one value.
    fn from_bytes(mut bytes: &[u8]) -> Option<Self> {
        let value = Self::take(&mut bytes)?;
        bytes.is_empty().then_some(value)
    }
}

macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl Bytes for $t {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }

            fn take(input: &mut &[u8]) -> Option<Self> {
                let (head, rest) = input.split_first_chunk()?;
                *input = rest;
                Some(Self::from_le_bytes(*head))
            }
        }
    )*};
}

numbers!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

impl Bytes for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }

    fn take(input: &mut &[u8]) -> Option<Self> {
        match u8::take(input)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl<T: Bytes> Bytes for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.put(out);
            }
        }
    }

    fn take(input: &mut &[u8]) -> Option<Self> {
        if bool::take(input)? {
            T::take(input).map(Some)
        } else {
            Some(None)
        }
    }
}

impl<T: Bytes, const N: usize> Bytes for [T; N] {
    fn put(&self, out: &mut Vec<u8>) {
        for v in self {
            v.put(out);
        }
    }

    fn take(input: &mut &[u8]) -> Option<Self> {
        let mut all = Vec::with_capacity(N);
        for _ in 0..N {
            all.push(T::take(input)?);
        }
        all.try_into().ok()
    }
}

impl Bytes for crate::Id {
    fn put(&self, out: &mut Vec<u8>) {
        (self.kind, self.n).put(out);
    }

    fn take(input: &mut &[u8]) -> Option<Self> {
        let (kind, n) = Bytes::take(input)?;
        Some(Self { kind, n })
    }
}

impl Bytes for crate::Spot {
    fn put(&self, out: &mut Vec<u8>) {
        (self.pos, self.facing).put(out);
    }

    fn take(input: &mut &[u8]) -> Option<Self> {
        let (pos, facing) = Bytes::take(input)?;
        Some(Self { pos, facing })
    }
}

macro_rules! tuples {
    ($(($($t:ident $v:ident),*)),*) => {$(
        impl<$($t: Bytes),*> Bytes for ($($t,)*) {
            #[allow(unused_variables)]
            fn put(&self, out: &mut Vec<u8>) {
                let ($($v,)*) = self;
                $($v.put(out);)*
            }

            #[allow(unused_variables)]
            fn take(input: &mut &[u8]) -> Option<Self> {
                Some(($($t::take(input)?,)*))
            }
        }
    )*};
}

tuples!(
    (),
    (A a),
    (A a, B b),
    (A a, B b, C c),
    (A a, B b, C c, D d),
    (A a, B b, C c, D d, E e),
    (A a, B b, C c, D d, E e, F f),
    (A a, B b, C c, D d, E e, F f, G g),
    (A a, B b, C c, D d, E e, F f, G g, H h)
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_come_back_as_they_went_and_a_short_or_long_input_is_refused() {
        let v = (
            7u32,
            -3i32,
            true,
            Some(2.5f32),
            None::<u16>,
            [1u8, 2, 3],
            crate::Id::player(9),
        );
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), 4 + 4 + 1 + 5 + 1 + 3 + 6);
        assert_eq!(Bytes::from_bytes(&bytes), Some(v));
        assert_eq!(<(u32, bool)>::from_bytes(&bytes[..4]), None);
        assert_eq!(u32::from_bytes(&bytes[..5]), None, "a byte left over");
        assert_eq!(bool::from_bytes(&[2]), None);
        assert_eq!(<()>::from_bytes(&[]), Some(()));
    }
}
