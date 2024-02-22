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
use super::module::WireValue;
use crate::utils::ShareSet;

#[derive(Debug, Clone, Copy)]
pub struct WireState {
    pub sensitivity: ShareSet,
    /// Has a deterministic value.
    pub value: Option<WireValue>,
    /// Is a fresh random of known origin.
    random: Option<RandomSource>,
    /// Is a share of a known and named sharing.
    share_of: Option<SharingSource>,
    /// Is a 0/1 value in a simulation not a 'x'.
    pub valid: bool,
}
impl WireState {
    fn new(
        sensitivity: ShareSet,
        value: Option<WireValue>,
        random: Option<RandomSource>,
        share_of: Option<SharingSource>,
        valid: bool,
    ) -> Self {
        let res = Self {
            sensitivity,
            value,
            random,
            share_of,
            valid,
        };
        res.consistency_check();
        res
    }
}

impl WireState {
    fn negate(mut self) -> Self {
        self.value = self.value.map(|v| !v);
        self.random = None;
        self.share_of = None;
        self.consistency_check();
        self
    }
    pub fn constant(v: WireValue) -> Self {
        Self::new(ShareSet::empty(), Some(v), None, None, true)
    }
    pub fn from_x() -> Self {
        Self::new(ShareSet::empty(), None, None, None, false)
    }
    fn consistency_check(&self) {
        assert!(!(self.value.is_some() && self.random.is_some()));
        if !self.valid {
            assert!(self.value.is_none());
            assert!(self.random.is_none());
            assert!(self.share_of.is_none());
        }
        if self.value.is_some() {
            assert!(self.sensitivity.is_empty());
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct SharingSource {}
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct RandomSource {}

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

impl CombBinary {
    fn propagate_neutral<T>(
        &self,
        op0: &WireState,
        op1: &WireState,
        v0: Option<T>,
        v1: Option<T>,
    ) -> Option<T> {
        if op0.value == Some(self.neutral()) {
            v0
        } else if op1.value == Some(self.neutral()) {
            v1
        } else {
            None
        }
    }
    fn sim(&self, op0: WireState, op1: WireState) -> WireState {
        let sensitivity = op0.sensitivity.union(op1.sensitivity);
        let value = self.opx(op0.value, op1.value);
        let random = self.propagate_neutral(&op0, &op1, op0.random, op1.random);
        let share_of = self.propagate_neutral(&op0, &op1, op0.share_of, op1.share_of);
        let valid = (op0.valid && op1.valid) || value.is_some();
        let res = WireState::new(sensitivity, value, random, share_of, valid);
        res.consistency_check();
        res
    }
}
fn sim_mux(op0: WireState, op1: WireState, ops: WireState) -> WireState {
    op0.consistency_check();
    op1.consistency_check();
    ops.consistency_check();
    let res = match ops.value {
        Some(WireValue::_0) => op0,
        Some(WireValue::_1) => op1,
        None => WireState::new(
            op0.sensitivity
                .union(op1.sensitivity)
                .union(ops.sensitivity),
            ops.valid
                .then_some((op0.value == op1.value).then_some(op0.value).flatten())
                .flatten(),
            (op0.random == op1.random).then_some(op0.random).flatten(),
            None,
            op0.valid & op1.valid & ops.valid,
        ),
    };
    //dbg!((op0, op1, ops, res));
    res.consistency_check();
    res
}
