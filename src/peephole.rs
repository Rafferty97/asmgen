pub fn run_peephole<const N: usize, T, F>(ops: &[T], f: F, nop: T) -> Vec<T>
where
    T: Copy + Eq,
    F: Fn([T; N]) -> [T; N],
{
    let mut output = vec![];
    let mut window = std::array::from_fn(|i| ops.get(i).copied().unwrap_or(nop));
    for i in 0..ops.len() {
        window = f(window);
        if window[0] != nop {
            output.push(window[0]);
        }
        window.rotate_left(1);
        window[N - 1] = ops.get(i + N).copied().unwrap_or(nop);
    }
    output
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn basic_test() {
        // Merge runs of the same integer, and treat `0` as the nop
        let input = vec![1, 2, 2, 3, 3, 3, 2, 0, 2, 2, 0, 0, 1, 1, 1];

        let result = run_peephole(&input, |[a, b]| if a == b { [0, a] } else { [a, b] }, 0);
        assert_eq!(&result, &[1, 2, 3, 2, 2, 1]);

        let result = run_peephole(&result, |[a, b]| if a == b { [0, a] } else { [a, b] }, 0);
        assert_eq!(&result, &[1, 2, 3, 2, 1]);
    }
}
