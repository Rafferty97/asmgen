#[derive(Clone, Copy, Debug)]
pub struct Ratio {
    pub num: i32,
    pub dem: i32,
}

impl Ord for Ratio {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.num * other.dem).cmp(&(other.num * self.dem))
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Ratio {}

impl PartialEq for Ratio {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Ratio {
    pub fn new(num: i32, dem: i32) -> Self {
        Self { num, dem }
    }
}
