// XXH32 implementation for parity with CExoString::GetHash().
// C++ uses XXH32(str, len, 0) XOR XXH32("", 0, 0).

const PRIME32_1: u32 = 0x9E3779B1;
const PRIME32_2: u32 = 0x85EBCA77;
const PRIME32_3: u32 = 0xC2B2AE3D;
const PRIME32_4: u32 = 0x27D4EB2F;
const PRIME32_5: u32 = 0x165667B1;

fn round(seed: u32, input: u32) -> u32 {
    let mut seed = seed.wrapping_add(input.wrapping_mul(PRIME32_2));
    seed = seed.rotate_left(13);
    seed.wrapping_mul(PRIME32_1)
}

fn avalanche(mut h32: u32) -> u32 {
    h32 ^= h32 >> 15;
    h32 = h32.wrapping_mul(PRIME32_2);
    h32 ^= h32 >> 13;
    h32 = h32.wrapping_mul(PRIME32_3);
    h32 ^= h32 >> 16;
    h32
}

pub fn xxh32(data: &[u8], seed: u32) -> u32 {
    let len = data.len();
    let mut idx = 0usize;
    let mut h32: u32;

    if len >= 16 {
        let mut v1 = seed.wrapping_add(PRIME32_1).wrapping_add(PRIME32_2);
        let mut v2 = seed.wrapping_add(PRIME32_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME32_1);
        while len - idx >= 16 {
            let w1 = u32::from_le_bytes(data[idx..idx + 4].try_into().unwrap());
            let w2 = u32::from_le_bytes(data[idx + 4..idx + 8].try_into().unwrap());
            let w3 = u32::from_le_bytes(data[idx + 8..idx + 12].try_into().unwrap());
            let w4 = u32::from_le_bytes(data[idx + 12..idx + 16].try_into().unwrap());
            v1 = round(v1, w1);
            v2 = round(v2, w2);
            v3 = round(v3, w3);
            v4 = round(v4, w4);
            idx += 16;
        }
        h32 = v1.rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
    } else {
        h32 = seed.wrapping_add(PRIME32_5);
    }

    h32 = h32.wrapping_add(len as u32);

    while len - idx >= 4 {
        let w = u32::from_le_bytes(data[idx..idx + 4].try_into().unwrap());
        h32 = h32.wrapping_add(w.wrapping_mul(PRIME32_3));
        h32 = h32.rotate_left(17).wrapping_mul(PRIME32_4);
        idx += 4;
    }
    while idx < len {
        h32 = h32.wrapping_add((data[idx] as u32).wrapping_mul(PRIME32_5));
        h32 = h32.rotate_left(11).wrapping_mul(PRIME32_1);
        idx += 1;
    }

    avalanche(h32)
}

/// Mirror of `CExoString::GetHash()` — XXH32(str, 0) XOR XXH32("", 0).
pub fn cexo_string_hash(s: &str) -> i32 {
    let null_hash = xxh32(&[], 0);
    let str_hash = xxh32(s.as_bytes(), 0);
    (str_hash ^ null_hash) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxh32_empty() {
        // XXH32("", 0, 0) is documented as 0x02CC5D05
        assert_eq!(xxh32(&[], 0), 0x02CC5D05);
    }

    #[test]
    fn xxh32_short() {
        // XXH32("abc", 0) → 0x32D153FF
        assert_eq!(xxh32(b"abc", 0), 0x32D153FF);
    }

    #[test]
    fn cexo_hash_empty_zero() {
        // Per CExoString::GetHash() comment, hash of "" is 0.
        assert_eq!(cexo_string_hash(""), 0);
    }
}
