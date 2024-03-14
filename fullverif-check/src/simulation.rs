use super::fv_cells::CombBinary;
use super::gadget::RndPortId;
use super::recsim::{GlobInstId, NspgiId, NspgiVec};
use super::top_sim::{GadgetExecCycle, GlobSimCycle, GlobSimulationState};
use super::WireValue;
use crate::share_set::{ShareId, ShareSet};
use anyhow::{bail, Result};
use itertools::Itertools;
use std::rc::Rc;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WireState {
    /// Set of sensitive shares.
    pub sensitivity: ShareSet,
    // FIXME: what about glitches and transitions for randomness?
    /// Set of sensitive shares considering gliches.
    pub glitch_sensitivity: ShareSet,
    /// Value from the non-symbolic simulation. None represents 'x'.
    pub value: Option<WireValue>,
    /// Is a fresh random of known origin.
    pub random: Option<RandomSource>,
    /// Is constant across all possible executions.
    pub deterministic: bool,
    /// Last execution of each NSPGI this wire depends on.
    pub nspgi_dep: NspgiDep,
}

// The GlobSimCycle refers to the "gadget exec ref lat + max_input_lat" cycle.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct NspgiDep(Rc<NspgiVec<Option<GadgetExecCycle>>>);

impl WireState {
    fn nil() -> Self {
        Self {
            sensitivity: ShareSet::empty(),
            glitch_sensitivity: ShareSet::empty(),
            value: None,
            random: None,
            deterministic: false,
            nspgi_dep: Default::default(),
        }
    }
    pub fn glitch_deterministic(&self) -> bool {
        self.deterministic && self.glitch_sensitivity.is_empty()
    }
    pub fn sensitive(&self) -> bool {
        !self.sensitivity.is_empty()
    }
    pub fn glitch_sensitive(&self) -> bool {
        !self.glitch_sensitivity.is_empty()
    }
    pub fn is_control(&self, value: WireValue) -> bool {
        self.glitch_deterministic() && self.value == Some(value)
    }
    pub fn negate(&self) -> Self {
        self.clone().with_value(self.value.map(|v| !v))
    }
    pub fn share(id: ShareId) -> Self {
        let res = Self {
            sensitivity: ShareSet::from(id),
            glitch_sensitivity: ShareSet::from(id),
            ..Self::nil()
        };
        res.consistency_check();
        res
    }
    pub fn random(port: RndPortId, lat: GlobSimCycle) -> Self {
        let res = Self {
            random: Some(RandomSource::new(port, lat)),
            ..Self::nil()
        };
        res.consistency_check();
        res
    }
    pub fn with_value(mut self, value: Option<WireValue>) -> Self {
        self.value = value;
        self
    }
    pub fn stop_glitches(mut self) -> Self {
        self.glitch_sensitivity = self.sensitivity;
        self
    }
    pub fn with_glitches(mut self, glitches: ShareSet) -> Self {
        self.glitch_sensitivity = self.glitch_sensitivity.union(glitches);
        self
    }
    pub fn control() -> Self {
        let res = Self {
            deterministic: true,
            ..Self::nil()
        };
        res.consistency_check();
        res
    }
    pub fn consistency_check(&self) {
        // sensitivity is a subset of glitch-sensitivity
        assert_eq!(
            self.glitch_sensitivity,
            self.glitch_sensitivity.union(self.sensitivity),
        );
        if self.deterministic {
            assert!(self.sensitivity.is_empty());
            assert!(self.random.is_none());
        }
    }
    pub fn check_secure(&self) -> Result<()> {
        if self.sensitivity.len() > 1 {
            bail!(
                "Wire is sensitive for multiple shares: {}.",
                self.sensitivity
            );
        } else if self.glitch_sensitivity.len() > 1 {
            bail!(
                "Wire is glitch-sensitive for multiple shares: {}.",
                self.glitch_sensitivity
            );
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RandomSource {
    pub port: RndPortId,
    pub lat: GlobSimCycle,
}

impl RandomSource {
    fn new(port: RndPortId, lat: GlobSimCycle) -> Self {
        RandomSource { port, lat }
    }
}

impl CombBinary {
    pub fn sim(
        &self,
        op0: &WireState,
        op1: &WireState,
        sim_state: Option<&mut GlobSimulationState>,
        inst_id: GlobInstId,
    ) -> WireState {
        let mut res = if op0.is_control(self.neutral())
            || self.absorb().is_some_and(|v| op1.is_control(v))
        {
            op1.clone()
        } else if op1.is_control(self.neutral()) || self.absorb().is_some_and(|v| op0.is_control(v))
        {
            op0.clone()
        } else {
            let sensitivity = op0.sensitivity.union(op1.sensitivity);
            let glitch_sensitivity = op0.glitch_sensitivity.union(op1.glitch_sensitivity);
            let value = self.opx(op0.value, op1.value);
            if let Some(sim_state) = sim_state {
                sim_state.leak_random(op0, inst_id);
                sim_state.leak_random(op1, inst_id);
            }
            let random = None;
            let deterministic = op0.deterministic && op1.deterministic;
            let nspgi_dep = op0.nspgi_dep.max(&op1.nspgi_dep);
            let res = WireState {
                sensitivity,
                glitch_sensitivity,
                value,
                random,
                deterministic,
                nspgi_dep,
            };
            res.consistency_check();
            res
        };
        // FIXME: handle glitch gating with stable deterministic value ?
        res.glitch_sensitivity = op0.glitch_sensitivity.union(op1.glitch_sensitivity);
        res
    }
}
pub fn sim_mux(
    op0: &WireState,
    op1: &WireState,
    ops: &WireState,
    sim_state: Option<&mut GlobSimulationState>,
    inst_id: GlobInstId,
) -> WireState {
    // FIXME: handle glitch gating with stable deterministic value ?
    op0.consistency_check();
    op1.consistency_check();
    ops.consistency_check();
    let res = if ops.is_control(WireValue::_0) {
        op0.clone().with_glitches(op1.glitch_sensitivity)
    } else if ops.is_control(WireValue::_1) {
        op1.clone().with_glitches(op0.glitch_sensitivity)
    } else {
        // Here we are a bit pessimistic wrt randomness, some cases might not be leakage, but that
        // should not be an issue in practice: mux without deterministic control should not be used
        // with randomness (outside "assumed" gadgets).
        if let Some(sim_state) = sim_state {
            sim_state.leak_random(op0, inst_id);
            sim_state.leak_random(op1, inst_id);
            sim_state.leak_random(ops, inst_id);
        }
        WireState {
            sensitivity: op0
                .sensitivity
                .union(op1.sensitivity)
                .union(ops.sensitivity),
            glitch_sensitivity: op0
                .glitch_sensitivity
                .union(op1.glitch_sensitivity)
                .union(ops.glitch_sensitivity),
            value: ops.value.and_then(|s| match s {
                WireValue::_0 => op0.value,
                WireValue::_1 => op1.value,
            }),
            random: (op0.random == op1.random).then_some(op0.random).flatten(),
            deterministic: ops.deterministic && op0.deterministic && op1.deterministic,
            nspgi_dep: op0.nspgi_dep.max(&op1.nspgi_dep).max(&ops.nspgi_dep),
        }
    };
    res.consistency_check();
    res
}

impl NspgiDep {
    fn is_empty(&self) -> bool {
        self.0.iter().all(|dep| dep.is_none())
    }
    pub fn max(&self, other: &Self) -> Self {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() || std::rc::Rc::ptr_eq(&self.0, &other.0) || self == other {
            self.clone()
        } else {
            Self(Rc::new(NspgiVec::from_vec(
                self.0
                    .iter()
                    .copied()
                    .zip_longest(other.0.iter().copied())
                    .map(|pair| pair.reduce(|d0, d1| d0.max(d1)))
                    .collect(),
            )))
        }
    }
    pub fn is_larger_than(&self, other: &Self) -> bool {
        // TODO improve perf
        self == &self.max(other)
    }
    pub fn last(&self, nspgi_id: NspgiId) -> Option<GadgetExecCycle> {
        *self.0.get(nspgi_id)?
    }
    pub fn empty() -> Self {
        Self(Rc::new(NspgiVec::new()))
    }
    pub fn single(nspgi_id: NspgiId, cycle: GadgetExecCycle) -> Self {
        let mut res = NspgiVec::from_vec(vec![None; nspgi_id.index() + 1]);
        res[nspgi_id] = Some(cycle);
        Self(Rc::new(res))
    }
}
