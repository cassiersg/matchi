#[macro_export]
macro_rules! new_id {
    ($it:ident) => {
        index_vec::define_index_type! {
            pub struct $it = u32;
            DISPLAY_FORMAT = "{}";
        }
    };
    ($it:ident, $vt:ident) => {
        $crate::type_utils::new_id!($it);
        #[allow(dead_code)]
        pub type $vt<T> = index_vec::IndexVec<$it, T>;
    };
    ($it:ident, $vt:ident, $st: ident) => {
        $crate::type_utils::new_id!($it, $vt);
        #[allow(dead_code)]
        pub type $st<T> = index_vec::IndexSlice<$it, [T]>;
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
            self.extend(std::iter::repeat(value).take(idx.index() + 1 - self.len()));
        }
    }
}
