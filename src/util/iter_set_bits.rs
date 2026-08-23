/// Returns an iterator that yeilds the index of each set bit in the `mask`,
/// in ascending order (least significant to most significant).
pub fn iter_set_bits(mask: u64) -> impl Iterator<Item = u8> {
    let mut mask = mask;
    std::iter::from_fn(move || match mask {
        0 => None,
        bits => {
            mask &= mask - 1;
            Some(bits.trailing_zeros() as u8)
        }
    })
}

#[cfg(test)]
mod test {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_iter_set_bits() {
        let iter = iter_set_bits(0);
        assert!(iter.collect_vec().is_empty());

        let iter = iter_set_bits(0b10010100110);
        assert_eq!(iter.collect_vec(), &[1, 2, 5, 7, 10]);

        let iter = iter_set_bits(0b10010111);
        assert_eq!(iter.collect_vec(), &[0, 1, 2, 4, 7]);

        let iter = iter_set_bits(u64::MAX);
        assert_eq!(iter.count(), 64);
    }
}
