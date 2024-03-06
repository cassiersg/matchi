use itertools::Itertools;

/// For generic functions over various int types.
pub trait Int:
    std::fmt::Display + std::cmp::Ord + Copy + std::cmp::Eq + std::ops::Add<Output = Self>
{
    fn one() -> Self;
}
impl Int for usize {
    fn one() -> Self {
        1
    }
}
impl Int for u32 {
    fn one() -> Self {
        1
    }
}

/// Represents the input set of integers as a series of closed intervals.
fn abstract_set<T: Int>(it: impl Iterator<Item = T>) -> impl Iterator<Item = (T, T)> {
    let mut vals = it.collect::<Vec<_>>();
    vals.sort_unstable();
    vals.into_iter()
        .dedup()
        .map(|val| (val, val))
        .coalesce(|(s1, e1), (s2, e2)| {
            if e1 + T::one() == s2 {
                Ok((s1, e2))
            } else {
                Err(((s1, e1), (s2, e2)))
            }
        })
}

/// Represents the input set of integers as a human-readable short string.
pub fn format_set<T: Int>(it: impl Iterator<Item = T>) -> String {
    let mut res = String::new();
    for (start, end) in abstract_set(it) {
        if !res.is_empty() {
            res.push_str(", ");
        }
        if start == end {
            res.push_str(&format!("{}", start));
        } else if start + T::one() == end {
            res.push_str(&format!("{}, {}", start, end));
        } else if start + T::one() + T::one() == end {
            res.push_str(&format!("{}, {}, {}", start, start + T::one(), end));
        } else {
            res.push_str(&format!(
                "{{{}, {}, ..., {}}}",
                start,
                start + T::one(),
                end
            ));
        }
    }
    if res.is_empty() {
        res.push_str("{}");
    }
    res
}

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
                let shift = x.trailing_zeros();
                tot_shift += shift;
                x >>= shift;
                Some(ShareId(tot_shift))
            }
        })
    }
}

impl std::fmt::Display for ShareId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::fmt::Display for ShareSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ShareSet({})", self.iter().join(", "))
    }
}
