// TODO:
// 1. Get simulation running:
// - initalize from vcd state
// - check run at top-level
// 2. Add basic security verification (no glitches, no transitions)
// - Get gadget structure
// - "Reset" valid gadget output sharings.
// - Get a notion of "gadget sensitivity"/"gadget validity", and mark randomness as used.
// 3. Add glitches (glitch_sensitivity: ShareSet in WireState ?)
// 4. Add transitions for shares: revisit how formal model of gadget composition works.
// - Overlapping structural gadgets.
// - Relax correctness requirement for gadgets.
// - Allow gadgets that take only part of the input sharing.
// - For each "out-of-gadget" combinational gate, cluster consecutive executions with the same
// share in one structural gadget. Require bubble between executions of different structural
// gadgets. That way, we are still O-PINI, pipeline, and iterated transition-robust within one
// structural gadget. Different structural gadgets are non-adjacent (notion to be defined ? means
// use the same structural gate consecutively).
// 4. Add transitions for randomness.

use super::fv_cells::{CombBinary, CombUnitary, Gate};
use super::gadget::{Latency, RndPortId, Slatency};
use super::module::WireValue;
use super::recsim::{GlobInstId, NspgiId, NspgiVec};
use super::top_sim::GlobSimulationState;
use crate::utils::{ShareId, ShareSet};
use anyhow::{bail, Result};
use itertools::izip;
use std::ops::Deref;
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

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct NspgiDep(Rc<NspgiVec<Option<Slatency>>>);

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
    pub fn random(port: RndPortId, lat: Latency) -> Self {
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
        if self.random.is_some() {
            // FIXME: enable when we get random validity back.
            //assert!(self.value.is_some());
        }
        if self.deterministic {
            assert!(self.sensitivity.is_empty());
            assert!(self.random.is_none());
        }
        if self.glitch_deterministic() {
            assert!(self.nspgi_dep.is_empty());
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
    pub lat: Latency,
}

impl RandomSource {
    fn new(port: RndPortId, lat: Latency) -> Self {
        RandomSource { port, lat }
    }
}

/*
impl Gate {
    pub fn eval(
        &self,
        mut operands: impl Iterator<Item = (Option<WireState>, Option<WireState>)>,
    ) -> WireState {
        match self {
            Gate::CombUnitary(ugate) => {
                let op = operands.next().unwrap().1.unwrap();
                match ugate {
                    CombUnitary::Buf => op,
                    CombUnitary::Not => op.negate(),
                }
            }
            Gate::CombBinary(bgate) => {
                let op0 = operands.next().unwrap().1.unwrap();
                let op1 = operands.next().unwrap().1.unwrap();
                bgate.sim(op0, op1)
            }
            Gate::Mux => {
                let op0 = operands.next().unwrap().1.unwrap();
                let op1 = operands.next().unwrap().1.unwrap();
                let ops = operands.next().unwrap().1.unwrap();
                sim_mux(op0, op1, ops)
            }
            Gate::Dff => {
                let _ = operands.next().unwrap(); // C
                let prev = operands.next().unwrap().0.unwrap(); // D
                prev
            }
        }
    }
}
*/

impl CombBinary {
    pub fn sim(
        &self,
        op0: &WireState,
        op1: &WireState,
        sim_state: &mut GlobSimulationState,
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
            sim_state.leak_random(op0, inst_id);
            sim_state.leak_random(op1, inst_id);
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
        // FIXME: handle gating with stable deterministic value ?
        res.glitch_sensitivity = op0.glitch_sensitivity.union(op1.glitch_sensitivity);
        res
    }
}
pub fn sim_mux(
    op0: &WireState,
    op1: &WireState,
    ops: &WireState,
    sim_state: &mut GlobSimulationState,
    inst_id: GlobInstId,
) -> WireState {
    // FIXME: handle gating with stable deterministic value ?
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
        sim_state.leak_random(op0, inst_id);
        sim_state.leak_random(op1, inst_id);
        sim_state.leak_random(ops, inst_id);
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
    fn max(&self, other: &Self) -> Self {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() {
            self.clone()
        } else {
            Self(Rc::new(NspgiVec::from_vec(
                izip!(self.0.iter().copied(), other.0.iter().copied())
                    .map(|(d0, d1)| d0.max(d1))
                    .collect(),
            )))
        }
    }
    pub fn with_dep(&self, nspgi_id: NspgiId, lat: Slatency) -> Self {
        let mut res = self.0.deref().clone();
        res.extend((res.len()..=(nspgi_id + 1).index()).map(|_| None));
        res[nspgi_id] = Some(lat);
        Self(Rc::new(res))
    }
    pub fn last(&self, nspgi_id: NspgiId) -> Option<Slatency> {
        *self.0.get(nspgi_id)?
    }
}
