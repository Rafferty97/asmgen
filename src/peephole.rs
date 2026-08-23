pub fn run_peephole<const N: usize, T, F>(ops: &mut Vec<T>, f: F, nop: T)
where
    T: Copy + Eq,
    F: Fn([T; N]) -> [T; N],
{
    let mut window = [nop; N];
    let mut next_input = 0;
    let mut next_output = 0;

    // Fill the window
    for op in window.iter_mut().take(ops.len()) {
        *op = ops[next_input];
        next_input += 1;
    }

    // Main loop
    while next_input < ops.len() {
        window = f(window).into();
        if window[0] != nop {
            ops[next_output] = window[0];
            next_output += 1;
        }
        window.rotate_left(1);
        window[N - 1] = ops[next_input];
        next_input += 1;
    }

    // Flush the window
    for _ in 0..N {
        window = f(window).into();
        if window[0] != nop {
            ops[next_output] = window[0];
            next_output += 1;
        }
        window.rotate_left(1);
        window[N - 1] = nop;
    }

    ops.truncate(next_output);
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn basic_test() {
        // Merge runs of the same integer, and treat `0` as the nop
        let input = vec![1, 2, 2, 3, 3, 3, 2, 0, 2, 2, 0, 0, 1, 1, 1];
        let merge = |[a, b]: [i32; 2]| if a == b { [0, a] } else { [a, b] };

        let mut result = input.clone();
        run_peephole(&mut result, merge, 0);
        assert_eq!(&result, &[1, 2, 3, 2, 2, 1]);

        run_peephole(&mut result, merge, 0);
        assert_eq!(&result, &[1, 2, 3, 2, 1]);
    }

    #[test]
    fn short_input() {
        // Merge runs of the same integer, and treat `0` as the nop
        let input = vec![1];
        let merge = |[a, b, c]: [i32; 3]| if a == b { [0, a, c] } else { [a, b, c] };

        let mut result = input.clone();
        run_peephole(&mut result, merge, 0);
        assert_eq!(&result, &[1]);
    }
}
