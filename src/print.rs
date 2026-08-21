use std::fmt::{self, Display, Formatter};

use crate::bits::PartialBits;

pub struct PrintBits<T>(pub T);

impl Display for PrintBits<u64> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        #[derive(Clone, Copy, PartialEq, Eq)]
        struct Part(u64);

        impl Display for Part {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:04b}", self.0)
            }
        }

        let parts = (0..64)
            .step_by(4)
            .rev()
            .map(|i| Part((self.0 >> i) & 0b1111));

        Self::write_parts(f, parts)
    }
}

impl Display for PrintBits<PartialBits> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[derive(Clone, Copy, PartialEq, Eq)]
        struct Part(u64, u64);

        impl Display for Part {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for i in (0..4).rev() {
                    let mask = 1 << i;
                    match (self.0 & mask != 0, self.1 & mask != 0) {
                        (_, false) => write!(f, "*")?,
                        (false, true) => write!(f, "0")?,
                        (true, true) => write!(f, "1")?,
                    }
                }
                Ok(())
            }
        }

        let parts = (0..64)
            .step_by(4)
            .rev()
            .map(|i| Part((self.0.bits() >> i) & 0b1111, (self.0.used() >> i) & 0b1111))
            .skip_while(|&Part(_, u)| u == 0);

        Self::write_parts(f, parts)
    }
}

impl<T> PrintBits<T> {
    fn write_parts<I, P>(f: &mut Formatter<'_>, mut parts: I) -> fmt::Result
    where
        I: Iterator<Item = P>,
        P: Copy + Eq + Display,
    {
        let Some(part) = parts.next() else {
            return write!(f, "[]");
        };

        write!(f, "[{part}")?;
        let (mut prev, mut cnt) = (part, 0);

        for bits in parts {
            if bits == prev {
                cnt += 1;
            } else {
                Self::write_part(f, prev, cnt)?;
                Self::write_part(f, prev, cnt)?;
                prev = bits;
                cnt = 0;
            }
        }

        Self::write_part(f, prev, cnt)?;
        write!(f, "]")
    }

    fn write_part<P>(f: &mut Formatter<'_>, part: P, cnt: usize) -> fmt::Result
    where
        P: Display,
    {
        match (part, cnt) {
            (_, 0) => Ok(()),
            (part, 1) => write!(f, " {part}"),
            (part, 2) => write!(f, " .... {part}"),
            (part, cnt) => write!(f, " ...({})... {part}", 4 * (cnt - 1)),
        }
    }
}
