pub fn is_aarch64_logical_immediate(value: u64) -> bool {
    if matches!(value, 0 | u64::MAX) {
        return false;
    }
    for len in [2, 4, 8, 16, 32, 64] {
        if is_tiled(value, len) && is_rotated_run(value, len) {
            return true;
        }
    }
    false
}

pub fn is_tiled(value: u64, len: u8) -> bool {
    value == value.rotate_right(len as u32)
}

/// Given a value that is a tiling of length `len`,
/// determines whether this repeated pattern can be derived by
/// rotating a contiguous run of one bits.
pub fn is_rotated_run(value: u64, len: u8) -> bool {
    // Clone the high bit to all bit positions
    let high_bit_mask = (value as i64) >> 63;

    // Overright all but the lowest copy of the tiled value
    // with a copy of the highest bit
    let shifted = (value as i64) >> (64 - len);

    // If the high-bit is set, flip all bits
    let value = shifted ^ high_bit_mask;

    // Shift out the lowest run on zeros
    let value = value.rotate_right(value.trailing_zeros());

    // A sequences of zeros followed by a sequence of ones is one less than a power of two
    value.wrapping_add(1).count_ones() == 1
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_is_rotated_run() {
        for len in [2, 4, 8, 16] {
            for value in 0u64..(1 << len) {
                println!("test case: {value:0>len$b}", len = len as usize);
                let value = tile_value(value, len);
                let expected = is_rotated_run_naive(value, len);
                let actual = is_rotated_run(value, len);
                assert_eq!(expected, actual);
            }
        }

        for value in 0..(0x400 * 0x400) {
            let value = {
                let left_side = value >> 10;
                let right_side = value & 0x3ff;
                (left_side << 54) | right_side
            };
            let expected = is_rotated_run(value, 64);
            let actual = is_rotated_run_naive(value, 64);
            assert_eq!(expected, actual);
        }
    }

    fn is_rotated_run_naive(value: u64, len: u8) -> bool {
        // Mask off the unused bits
        let mask = u64::MAX >> (64 - len);
        let val = value & mask;

        // All zeros or all ones within the n-bit window: accept.
        if val == 0 || val == mask {
            return true;
        }

        // A contiguous run (possibly wrapping around the n-bit ring) has exactly
        // one 0->1 transition when scanned cyclically. Count rising edges.
        let mut transitions = 0;
        for i in 0..len {
            let cur = (val >> i) & 1;
            let nxt = (val >> ((i + 1) % len)) & 1;
            if cur == 0 && nxt == 1 {
                transitions += 1;
            }
        }
        transitions == 1
    }

    fn tile_value(value: u64, len: u8) -> u64 {
        let mut value = value & ((1 << len) - 1);
        loop {
            let next_value = (value << len) | value;
            if next_value == value {
                return value;
            }
            value = next_value;
        }
    }
}
