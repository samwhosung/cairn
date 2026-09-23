#[derive(Clone, Copy)]
pub(crate) enum HashType {
    TableOffset,
    NameA,
    NameB,
    FileKey,
}

const CIPHER: usize = 4;

static TABLE: [[u32; 0x100]; 5] = crypt_table();

const fn crypt_table() -> [[u32; 0x100]; 5] {
    let mut table = [[0u32; 0x100]; 5];
    let mut seed: u32 = 0x0010_0001;
    let mut i = 0;
    while i < 0x100 {
        let mut section = 0;
        while section < 5 {
            seed = (seed * 125 + 3) % 0x2A_AAAB;
            let high = (seed & 0xFFFF) << 16;
            seed = (seed * 125 + 3) % 0x2A_AAAB;
            table[section][i] = high | (seed & 0xFFFF);
            section += 1;
        }
        i += 1;
    }
    table
}

pub(crate) fn canonical(byte: u8) -> u8 {
    if byte == b'/' {
        b'\\'
    } else {
        byte.to_ascii_uppercase()
    }
}

pub(crate) fn hash_string(name: &str, kind: HashType) -> u32 {
    let mut seed1: u32 = 0x7FED_7FED;
    let mut seed2: u32 = 0xEEEE_EEEE;
    for byte in name.bytes().map(canonical) {
        seed1 = TABLE[kind as usize][usize::from(byte)] ^ seed1.wrapping_add(seed2);
        seed2 = u32::from(byte)
            .wrapping_add(seed1)
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
    }
    seed1
}

pub(crate) fn decrypt(data: &mut [u32], mut key: u32) {
    let mut seed: u32 = 0xEEEE_EEEE;
    for word in data {
        seed = seed.wrapping_add(TABLE[CIPHER][(key & 0xFF) as usize]);
        *word ^= key.wrapping_add(seed);
        key = (!key << 21).wrapping_add(0x1111_1111) | (key >> 11);
        seed = word
            .wrapping_add(seed)
            .wrapping_add(seed << 5)
            .wrapping_add(3);
    }
}

#[cfg(test)]
pub(crate) fn encrypt(data: &mut [u32], mut key: u32) {
    let mut seed: u32 = 0xEEEE_EEEE;
    for word in data {
        seed = seed.wrapping_add(TABLE[CIPHER][(key & 0xFF) as usize]);
        let plain = *word;
        *word ^= key.wrapping_add(seed);
        key = (!key << 21).wrapping_add(0x1111_1111) | (key >> 11);
        seed = plain
            .wrapping_add(seed)
            .wrapping_add(seed << 5)
            .wrapping_add(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_known_values() {
        assert_eq!(hash_string("(hash table)", HashType::FileKey), 0xC3AF_3770);
        assert_eq!(hash_string("(block table)", HashType::FileKey), 0xEC83_B3A3);
        assert_eq!(hash_string("file.txt", HashType::TableOffset), 0x3EA9_8D7A);
    }

    #[test]
    fn hash_ignores_case_and_slash_direction() {
        assert_eq!(
            hash_string("path\\to\\FILE.blp", HashType::TableOffset),
            hash_string("path/to/file.blp", HashType::TableOffset)
        );
    }

    #[test]
    fn decrypt_inverts_encrypt() {
        let key = hash_string("(hash table)", HashType::FileKey);
        let original = [0x1234_5678u32, 0x9ABC_DEF0, 0x0F0F_0F0F, 42];
        let mut words = original;
        encrypt(&mut words, key);
        assert_ne!(words, original);
        decrypt(&mut words, key);
        assert_eq!(words, original);
    }
}
