//! Analysis of vcd files as a series of state, for each clock cycle.

// FIXME: extract only top-level I/O.

use crate::error::{CompError, CompErrorKind};
use anyhow::{bail, Result};
use fnv::FnvHashMap as HashMap;
use std::borrow::Borrow;

/// State of a circuit at one clock cycle.
pub type State = Vec<VarState>;

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

/// Id of a variable (for lookup into State)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarId(usize);

/// States of a circuit over time.
#[derive(Debug)]
pub struct VcdStates {
    header: vcd::Header,
    states: Vec<State>,
    cache_ids: std::cell::RefCell<CacheNameIds>,
    idcode2var_id: HashMap<vcd::IdCode, VarId>,
}

/// Cache for lookups of path -> ids.
/// (to improve performance, since vcd::find_var uses linear probing)
#[derive(Debug, Default)]
struct CacheNameIds {
    id: usize,
    scopes: HashMap<String, CacheNameIds>,
}
impl CacheNameIds {
    fn new(id: usize) -> Self {
        Self {
            id,
            scopes: HashMap::default(),
        }
    }
}

impl VcdStates {
    /// Create VcdStates from a reader of a vcd file and the path of the clock signal.
    pub fn new(r: &mut impl std::io::BufRead, clock: &[impl Borrow<str>]) -> Result<Self> {
        let mut parser = vcd::Parser::new(r);
        let Ok(header) = parser.parse_header() else {
            bail!("TODO format {:?}", CompError::no_mod(CompErrorKind::Vcd));
        };
        let clock = header
            .find_var(clock)
            .ok_or_else(|| {
                CompError::no_mod(CompErrorKind::Other(format!(
                    "Error: Did not find clock {:?} in vcd file.",
                    clock.join(".")
                )))
            })?
            .code;
        let (idcode2var_id, vars) = list_vars(&header);
        let states = clocked_states(
            &idcode2var_id,
            vars.as_slice(),
            clock,
            parser.map(|cmd| {
                cmd.map_err(|_| {
                    anyhow::Error::msg(format!(
                        "TODO format {:?}",
                        CompError::no_mod(CompErrorKind::Vcd)
                    ))
                })
            }),
        )?;
        let cache_ids = std::cell::RefCell::new(CacheNameIds::default());
        Ok(Self {
            header,
            states,
            cache_ids,
            idcode2var_id,
        })
    }

    fn code2var(&self, code: vcd::IdCode) -> VarId {
        self.idcode2var_id[&code]
    }

    /// VarId from the path (list of strings) of a variable
    pub fn get_var_id(&self, path: &[impl Borrow<str>]) -> Result<VarId> {
        let mut cache = self.cache_ids.borrow_mut();
        let mut dir: &mut CacheNameIds = &mut (*cache);
        let mut scope: &[vcd::ScopeItem] = &self.header.items;
        for (path_part, name) in path.iter().enumerate() {
            let n = name.borrow();
            if dir.scopes.contains_key(n) {
                dir = dir.scopes.get_mut(n).unwrap();
                match &scope[dir.id] {
                    vcd::ScopeItem::Scope(s) => {
                        scope = &s.items;
                    }
                    vcd::ScopeItem::Var(v) => {
                        if path_part == path.len() - 1 {
                            return Ok(self.code2var(v.code));
                        } else {
                            // error
                            break;
                        }
                    }
                    _ => {}
                }
            } else {
                fn scope_id(s: &vcd::ScopeItem) -> String {
                    let res = match s {
                        vcd::ScopeItem::Var(v) => &v.reference,
                        vcd::ScopeItem::Scope(s) => &s.identifier,
                        _ => {
                            unreachable!()
                        }
                    };
                    // Remove leading backslash, in case the vcd is encoded using the "escaped
                    // identifier" syntax of verilog.
                    let res = res.strip_prefix('\\').unwrap_or(res);
                    // Also, we fix the duplication introduced by iverilog (seemingly
                    // non-standard).
                    res.replace(r"\\", r"\")
                }
                match scope.iter().enumerate().find(|(_, s)| scope_id(s) == n) {
                    Some((i, s)) => {
                        dir = dir
                            .scopes
                            .entry(scope_id(s).to_owned())
                            .or_insert(CacheNameIds::new(i));
                        if let vcd::ScopeItem::Scope(s) = s {
                            scope = &s.items;
                        } else {
                            match &scope[dir.id] {
                                vcd::ScopeItem::Scope(s) => {
                                    scope = &s.items;
                                }
                                vcd::ScopeItem::Var(v) => {
                                    if path_part == path.len() - 1 {
                                        return Ok(self.code2var(v.code));
                                    } else {
                                        // error
                                        break;
                                    }
                                }
                                _ => {
                                    unreachable!()
                                }
                            }
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
        }
        //bail!("Error: Did not find signal {} in vcd file.", path.join("."));
        bail!(
            "Error: Did not find signal {:?} in vcd file.",
            path.join("|")
        );
    }

    /// State of a variable. Returns None if the cycle is too large compared to what was in the vcd.
    pub fn get_var(&self, var: VarId, cycle: usize) -> Option<&VarState> {
        //trace!("cycle: {}, n_cycles: {}", cycle, self.states.len());
        self.states.get(cycle).map(|state| &state[var.0 as usize])
    }

    /// State of a wire in a vector variable. Returns None if the cycle is too large compared to
    /// what was in the vcd.
    pub fn get_var_idx(&self, var: VarId, cycle: usize, offset: usize) -> Option<VarState> {
        self.get_var(var, cycle).map(|state| match state {
            res @ VarState::Scalar(_) => {
                assert_eq!(offset, 0);
                res.clone()
            }
            res @ VarState::Uninit => res.clone(),
            VarState::Vector(values) => VarState::Scalar(values[offset]),
        })
    }

    /// Number of clock cycles in the vcd file.
    pub fn len(&self) -> usize {
        self.states.len()
    }
}

// FIXME: could we replace the Vec<String> with a VarId for better efficiency ?
/// Records the path and the results of all the queries.
pub type StateLookups = HashMap<(Vec<String>, usize, usize), Option<VarState>>;

/// Query the control signals in a module.
/// Adds the following features on top of VcdStates:
/// * working in a submodule (i.e. prepends a prefix to all path queries)
/// * working from a cycle offset.
/// * recording all the queries (and their results)
#[derive(Debug, Clone)]
pub struct ModuleControls<'a> {
    vcd_states: &'a VcdStates,
    offset: usize,
    pub root_module: Vec<String>,
    accessed: StateLookups,
}

impl<'a> ModuleControls<'a> {
    pub fn new(vcd_states: &'a VcdStates, root_module: Vec<String>, offset: usize) -> Self {
        Self {
            vcd_states,
            offset,
            root_module,
            accessed: HashMap::default(),
        }
    }

    pub fn root_module(&self) -> &[String] {
        self.root_module.as_slice()
    }

    /// Create a ModulesControls for the given root_module, with cycle 0 set to the first cycle
    /// where tne enable signal is asserted.
    /// The enable signal and root_module paths start at the vcd root.
    pub fn from_enable<'b>(
        vcd_states: &'a VcdStates,
        root_module: Vec<String>,
        enable: &[impl Borrow<str>],
    ) -> Result<Self> {
        let enable_code = vcd_states.get_var_id(enable)?;
        let Some(offset) = (0..vcd_states.len()).find(|i| {
            vcd_states.get_var(enable_code, *i).unwrap() == &VarState::Scalar(vcd::Value::V1)
        }) else {
            bail!(
                "Error: Enable signal {:?} never asserted.",
                enable.join(".")
            )
        };
        debug!("ModuleControls offset: {}", offset);
        Ok(Self::new(vcd_states, root_module, offset))
    }

    /// Create a fresh ModuleControls, incrementing the cycle offset by time_offset from the
    /// current offset and selecting a sub-module path from the current one.
    /// The StateLookups state of the new ModuleControls is empty.
    pub fn submodule(&self, module: String, time_offset: usize) -> Self {
        let mut path = self.root_module.clone();
        path.push(module);
        Self {
            vcd_states: self.vcd_states,
            offset: self.offset + time_offset,
            root_module: path,
            accessed: StateLookups::default(),
        }
    }

    /// Lookup the value of the wire path[idx] at the given cycle.
    /// Returns None when the cycle to be looked up is after the end of the vcd file.
    pub fn lookup(
        &mut self,
        path: Vec<String>,
        cycle: usize,
        idx: usize,
    ) -> Result<Option<VarState>> {
        let mut p: Vec<String> = self.root_module.clone();
        p.extend(path.iter().map(|s| s.to_owned()));
        let var_id = self.vcd_states.get_var_id(&p)?;
        let vcd_states = &self.vcd_states;
        let accessed = &mut self.accessed;
        let offset = self.offset;
        Ok(accessed
            .entry((path, cycle, idx))
            .or_insert_with(|| vcd_states.get_var_idx(var_id, offset + cycle, idx))
            .clone())
    }

    /// Returns the list of the lookups.
    pub fn lookups(self) -> StateLookups {
        self.accessed
    }

    /// Number of cycles from the start of the module to the end of the vcd.
    pub fn len(&self) -> usize {
        self.vcd_states.len() - self.offset
    }

    pub fn first_asserted(&mut self, path: Vec<String>, idx: usize) -> Result<usize> {
        for i in 0..self.len() {
            if self.lookup(path.clone(), i, idx)?.unwrap() == VarState::Scalar(vcd::Value::V1) {
                return Ok(i);
            }
        }
        bail!("Error: Enable signal {:?} never asserted.", path.join("."))
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

/// Computes the state from the vcd reader, the clock and the list of variables.
fn clocked_states(
    code2var_id: &HashMap<vcd::IdCode, VarId>,
    vars: &[vcd::Var],
    clock: vcd::IdCode,
    commands: impl Iterator<Item = Result<vcd::Command>>,
) -> Result<Vec<State>> {
    let mut states = Vec::new();
    let mut current_state = vec![VarState::Uninit; vars.len()];
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
                current_state[code2var_id[&id_code].0] = VarState::Scalar(value);
            }
            vcd::Command::ChangeVector(id_code, value) => {
                let var_id = code2var_id[&id_code];
                current_state[var_id.0] =
                    VarState::Vector(pad_vec_and_reverse(value, vars[var_id.0].size));
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

/// List the variables in the vcd.
fn list_vars(header: &vcd::Header) -> (HashMap<vcd::IdCode, VarId>, Vec<vcd::Var>) {
    let mut res = HashMap::default();
    let mut remaining_items = header.items.iter().collect::<Vec<_>>();
    while let Some(scope_item) = remaining_items.pop() {
        match scope_item {
            vcd::ScopeItem::Scope(scope) => {
                remaining_items.extend(scope.items.iter());
            }
            vcd::ScopeItem::Var(var) => {
                res.insert(var.code, var.clone());
            }
            _ => {}
        }
    }
    let (id_codes, vars): (Vec<_>, Vec<_>) = res.into_iter().unzip();
    let code2var_id = id_codes
        .into_iter()
        .enumerate()
        .map(|(i, code)| (code, VarId(i)))
        .collect();
    (code2var_id, vars)
}
