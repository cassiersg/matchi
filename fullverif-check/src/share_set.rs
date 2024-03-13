use itertools::Itertools;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ShareId(u32);

impl ShareId {
    pub fn from_raw(x: u32) -> Self {
        ShareId(x)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareSet(u64);

impl ShareSet {
    pub fn empty() -> Self {
        Self(0)
    }
    pub fn from(x: ShareId) -> Self {
        assert!(x.0 < 64);
        Self(1 << x.0)
    }
    pub fn union(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
    pub fn intersection(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
    pub fn difference(self, rhs: Self) -> Self {
        Self(self.0 & !rhs.0)
    }
    pub fn subset_of(self, rhs: Self) -> bool {
        self.difference(rhs).is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    pub fn contains(&self, x: ShareId) -> bool {
        !self.intersection(Self::from(x)).is_empty()
    }
    pub fn len(&self) -> usize {
        self.0.count_ones() as usize
    }
    pub fn iter(&self) -> impl Iterator<Item = ShareId> {
        let mut x = self.0;
        let mut tot_shift = 0;
        std::iter::from_fn(move || {
            if x == 0 {
                None
            } else {
                let shift = x.trailing_zeros() + 1;
                tot_shift += shift;
                x >>= shift;
                Some(ShareId(tot_shift - 1))
            }
        })
    }
    pub fn clear_if(self, cond: bool) -> Self {
        if cond {
            Self::empty()
        } else {
            self
        }
    }
}

impl std::fmt::Display for ShareId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::fmt::Display for ShareSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ShareSet{{{}}}", self.iter().join(", "))
    }
}
