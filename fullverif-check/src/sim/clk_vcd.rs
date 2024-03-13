//! Analysis of vcd files as a series of state, for each clock cycle.

use super::WireValue;
use crate::type_utils::new_id;
use anyhow::{anyhow, bail, Result};
use fnv::FnvHashMap as HashMap;
use std::borrow::Borrow;

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
    pub fn scalar(&self) -> Option<vcd::Value> {
        if let VarState::Scalar(res) = self {
            Some(*res)
        } else {
            None
        }
    }
}

// Id of a variable.
new_id!(VarId, VarVec, VarSlice);

pub type IdCode = vcd::IdCode;

pub struct VcdParsedHeader<R: std::io::BufRead> {
    header: vcd::Header,
    parser: vcd::Parser<R>,
    used_vars: VarVec<vcd::Var>,
    clock: IdCode,
    var_names: HashMap<Vec<String>, (vcd::Var, Option<VarId>)>,
}

#[derive(Debug, Clone)]
pub struct VcdParsedStates {
    states: Vec<State>,
}

#[derive(Debug, Copy, Clone)]
pub struct VarOffsetId(VarId, usize);

impl<R: std::io::BufRead> VcdParsedHeader<R> {
    pub fn new(mut parser: vcd::Parser<R>, clock: &[impl Borrow<str>]) -> Result<Self> {
        let header = parser.parse_header()?;
        let clock = header
            .find_var(clock)
            .ok_or_else(|| anyhow!("Did not find clock {} in vcd file.", clock.join(".")))?
            .code;
        let var_names = var_names(&header);
        Ok(Self {
            header,
            parser,
            used_vars: VarVec::new(),
            clock,
            var_names,
        })
    }
    pub fn add_var(&mut self, path: &[String]) -> Result<VarId> {
        let res = self
            .var_names
            .get_mut(path)
            .ok_or_else(|| anyhow!("No variable {:?} in vcd.", path))?;
        Ok(*res
            .1
            .get_or_insert_with(|| self.used_vars.push(res.0.clone())))
    }
    pub fn add_var_offset(&mut self, path: &[String], offset: usize) -> Result<VarOffsetId> {
        let var_id = self.add_var(path)?;
        if offset >= self.used_vars[var_id].size as usize {
            bail!(
                "Cannot index variable {:?} at offset {}: not long enough",
                path,
                offset
            );
        }
        Ok(VarOffsetId(var_id, offset))
    }
    pub fn get_states(self) -> Result<VcdParsedStates> {
        let states = clocked_states2(
            &self.used_vars,
            self.clock,
            self.parser.map(|cmd| cmd.map_err(Into::into)),
        )?;
        Ok(VcdParsedStates { states })
    }
}
impl VcdParsedStates {
    pub fn get_var(&self, var_id: VarId, cycle: usize) -> &VarState {
        &self.states[cycle][var_id]
    }
    pub fn get_var_offset(&self, var_offset_id: VarOffsetId, cycle: usize) -> Option<WireValue> {
        match &self.states[cycle][var_offset_id.0] {
            VarState::Scalar(value) if var_offset_id.1 == 0 => WireValue::from_vcd(*value),
            VarState::Vector(values) => WireValue::from_vcd(values[var_offset_id.1]),
            VarState::Uninit => None,
            x @ _ => unreachable!("expected no offset, found {:?}, {:?}", var_offset_id, x),
        }
    }
    pub fn len(&self) -> usize {
        self.states.len()
    }
}

/// Maps the state of a vector signal from the vcd (truncated, BE) to the representation used in
/// the states (not trucated, LE).
fn pad_vec_and_reverse(vec: vcd::Vector, size: u32) -> Vec<vcd::Value> {
    let mut vec: Vec<vcd::Value> = vec.into();
    // We need to reverse order of bits since last one in binary writing is at offset 0.
    // Then we pad since leading '0', 'x' or 'z' are not always written.
    let padding_value = if vec[0] == vcd::Value::V1 {
        vcd::Value::V0
    } else {
        vec[0]
    };
    vec.reverse();
    vec.extend(std::iter::repeat(padding_value).take((size as usize) - vec.len()));
    vec
}

fn var_names(header: &vcd::Header) -> HashMap<Vec<String>, (vcd::Var, Option<VarId>)> {
    let mut res = HashMap::default();
    let mut remaining_items = header
        .items
        .iter()
        .map(|scope_item| (scope_item, vec![]))
        .collect::<Vec<_>>();
    while let Some((scope_item, mut path)) = remaining_items.pop() {
        match scope_item {
            vcd::ScopeItem::Scope(scope) => {
                remaining_items.extend(scope.items.iter().map(|scope_item| {
                    (
                        scope_item,
                        path.iter()
                            .cloned()
                            .chain(std::iter::once(normalize_name(&scope.identifier)))
                            .collect(),
                    )
                }));
            }
            vcd::ScopeItem::Var(var) => {
                path.push(normalize_name(&var.reference));
                res.insert(path, (var.clone(), None));
            }
            _ => {}
        }
    }
    res
}

fn normalize_name(name: &str) -> String {
    // Remove leading backslash, in case the vcd is encoded using the "escaped
    // identifier" syntax of verilog.
    // Also, we fix the duplication introduced by iverilog (seemingly
    // non-standard).
    name.strip_prefix('\\').unwrap_or(name).replace(r"\\", r"\")
}

fn clocked_states2(
    used_vars: &VarVec<vcd::Var>,
    clock: vcd::IdCode,
    commands: impl Iterator<Item = Result<vcd::Command>>,
) -> Result<Vec<State>> {
    let max_idcode = used_vars
        .iter()
        .map(|var| u64::from(var.code))
        .max()
        .unwrap_or(0);
    let mut idcode2var_id = vec![None; max_idcode as usize + 1];
    for (var_id, var) in used_vars.iter_enumerated() {
        idcode2var_id[u64::from(var.code) as usize] = Some(var_id);
    }
    let get_var_id = |idcode| {
        idcode2var_id
            .get(u64::from(idcode) as usize)
            .copied()
            .flatten()
    };
    let mut states = Vec::new();
    let mut current_state = VarVec::from_vec(vec![VarState::Uninit; used_vars.len()]);
    //let mut previous_state = current_state.clone();
    let mut previous_state = current_state.clone();
    let mut clk_state = vcd::Value::X;
    let mut started = false;
    for command in commands {
        match command? {
            vcd::Command::ChangeScalar(id_code, value) => {
                if id_code == clock {
                    match value {
                        vcd::Value::V1 if clk_state == vcd::Value::V0 => {
                            states.push(previous_state.clone());
                            clk_state = vcd::Value::V1;
                            started = true;
                        }
                        vcd::Value::V0 | vcd::Value::V1 => {
                            clk_state = value;
                            started = true;
                        }
                        vcd::Value::X | vcd::Value::Z => {
                            if started {
                                bail!(
                                    "Invalid value for the clock: {:?} (at cycle >= {}).",
                                    value,
                                    states.len()
                                );
                            }
                        }
                    }
                }
                if let Some(var_id) = get_var_id(id_code) {
                    current_state[var_id] = VarState::Scalar(value);
                }
            }
            vcd::Command::ChangeVector(id_code, value) => {
                if let Some(var_id) = get_var_id(id_code) {
                    current_state[var_id] =
                        VarState::Vector(pad_vec_and_reverse(value, used_vars[var_id].size));
                }
            }
            vcd::Command::Timestamp(_) => {
                previous_state = current_state.clone();
            }
            _ => {}
        }
    }
    states.push(current_state);
    Ok(states)
}
