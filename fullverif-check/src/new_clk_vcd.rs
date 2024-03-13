use crate::error::{CompError, CompErrorKind};
use crate::type_utils::new_id;
use anyhow::{bail, Result};
use fnv::FnvHashMap as HashMap;
use std::borrow::Borrow;

/// Id of a variable (for lookup into State)
new_id!(VarId, VarVec, VarSlice);

/// State of a circuit at one clock cycle.
pub type State = VarVec<VarState>;

/// State of a variable at one clock cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VarState {
    Scalar(vcd::Value),
    Vector(Vec<vcd::Value>),
    Uninit,
}
impl VarState {
    pub fn to_bool(&self) -> Option<bool> {
        match self {
            VarState::Scalar(vcd::Value::V0) => Some(false),
            VarState::Scalar(vcd::Value::V1) => Some(true),
            _ => None,
        }
    }
}

/// States of a circuit over time.
#[derive(Debug)]
pub struct VcdStates {
    header: vcd::Header,
    states: Vec<State>,
    cache_ids: std::cell::RefCell<CacheNameIds>,
    idcode2var_id: HashMap<vcd::IdCode, VarId>,
}
