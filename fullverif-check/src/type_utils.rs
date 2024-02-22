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
