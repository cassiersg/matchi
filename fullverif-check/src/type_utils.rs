#[macro_export]
macro_rules! new_id {
    ($it:ident, $vt:ident, $st: ident) => {
        index_vec::define_index_type! {
            pub struct $it = u32;
            DISPLAY_FORMAT = "{}";
        }
        #[allow(dead_code)]
        pub type $vt<T> = index_vec::IndexVec<$it, T>;
        #[allow(dead_code)]
        pub type $st<T> = index_vec::IndexSlice<$it, [T]>;
        impl $it {
            pub fn idx(self) -> usize {
                <Self as Into<usize>>::into(self)
            }
            pub fn from_idx(x: usize) -> Self {
                <Self as From<usize>>::from(x)
            }
        }
    };
}
pub(crate) use new_id;

pub trait ExtendIdx {
    type Index: Copy;
    type Value: Clone;
    fn extend_idx(&mut self, idx: Self::Index, value: Self::Value);
}

impl<I: index_vec::Idx, T: Clone> ExtendIdx for index_vec::IndexVec<I, T> {
    type Index = I;
    type Value = T;
    fn extend_idx(&mut self, idx: Self::Index, value: Self::Value) {
        if self.len() <= idx.index() {
            self.extend(std::iter::repeat(value).take((self.len() + 1) - idx.index()));
        }
    }
}
