// Simulator
//
// instance interface:
// - eval_out: request to evaluate some output, required input values are provided
// - eval_all: request to complete evaluation
//
// eval_out implementation:
//     - in combinational wire DAG of module with reversed edges, make a DfsPostOrder from the output to be computed, evaluate the wire
//     - this is recursive over module inclusion
//     - better to pre-evaluate this, and cache the evaluation order as a list of lists, given known query order
//     - use a single DfsPostOrder, keep visited and use move_to for each output.
//
// eval_all implementation:
//     - eval all non-evaluated nodes in toposort order of combinational wire DAG
//     - can also be pre-calculated
//     - for the pre-calculation: iterate over wires, and eval them using the DfsPostOrder
//
use super::fv_cells::{CombBinary, CombUnitary, Gate};
use super::gadget::{InputRole, Latency, LatencyVec, Slatency};
use super::module::{
    ConnectionId, InputId, InputVec, InstanceType, InstanceVec, OutputId, WireId, WireVec,
};
use super::netlist::Netlist;
use super::simulation::WireState;
use super::top_sim::GlobSimulationState;
use super::{ModuleId, WireValue};
use crate::type_utils::new_id;
use crate::utils::ShareSet;
use anyhow::{anyhow, bail, Context, Error, Result};
use itertools::izip;

// Globally-unique instance ID.
new_id!(GlobInstId, GlobInstVec, GlobInstSlice);
// Non-sharewise pipeline gadget instances
new_id!(NspgiId, NspgiVec, NspgiSlice);
// Registers (used to account for randomness storage)
new_id!(RegInstId, RegInstVec, RegInstSlice);

/// Half-open range
#[derive(Debug, Copy, Clone)]
struct Range<T> {
    start: T,
    end: T,
}

#[derive(Debug, Clone)]
pub struct EvalInstanceIds {
    nspgi: Range<NspgiId>,
    regs: Range<RegInstId>,
    insts: Range<GlobInstId>,
}

#[enum_dispatch::enum_dispatch]
pub trait Evaluator {
    fn uninit_state(&self, netlist: &Netlist) -> EvaluatorState;
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState;
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState;
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> EvaluatorState;
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    );
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState;
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    );
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()>;
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()>;
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()>;
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String>;
}

#[enum_dispatch::enum_dispatch(Evaluator)]
#[derive(Debug, Clone)]
pub enum InstanceEvaluator {
    Gate(GateEvaluator),
    Tie(TieEvaluator),
    Module(ModuleEvaluator),
    Gadget(PipelineGadgetEvaluator),
}

#[derive(Debug, Clone)]
pub struct ModuleEvaluator {
    module_id: ModuleId,
    instance_evaluators: InstanceVec<Option<InstanceEvaluator>>,
    ginst_id: GlobInstId,
    inst_ids: InstanceVec<EvalInstanceIds>,
    // The Option<ConnectionId> is only there for an assert.
    eval_order: Vec<(ConnectionId, Vec<WireId>)>,
    eval_finish: Vec<WireId>,
}

#[derive(Debug, Clone)]
struct GateEvaluator {
    gate: super::fv_cells::Gate,
    inst_id: GlobInstId,
    reg_id: Option<RegInstId>,
}

#[derive(Debug, Clone)]
struct TieEvaluator {
    value: WireValue,
}

#[derive(Debug, Clone)]
struct PipelineGadgetEvaluator {
    module_id: ModuleId,
    nspgi_id: NspgiId,
    module_evaluator: ModuleEvaluator,
}

#[derive(Debug, Clone)]
pub enum EvaluatorState {
    Gate(GateState),
    Module(ModuleState),
    PipelineGadget(PipelineGadgetState),
    Tie,
}

impl EvaluatorState {
    pub fn module(&self) -> &ModuleState {
        if let Self::Module(res) = self {
            res
        } else {
            panic!("{:?} is not a module state", self);
        }
    }
    pub fn module_mut(&mut self) -> &mut ModuleState {
        if let Self::Module(res) = self {
            res
        } else {
            panic!("{:?} is not a module state", self);
        }
    }
    pub fn module_any(&self) -> Option<&ModuleState> {
        match self {
            Self::Module(res) => Some(res),
            Self::PipelineGadget(res) => Some(&res.module_state),
            _ => None,
        }
    }
    pub fn expect_module(self) -> ModuleState {
        if let Self::Module(res) = self {
            res
        } else {
            panic!("{:?} is not a module state", self);
        }
    }
    fn gate_mut(&mut self) -> &mut GateState {
        if let Self::Gate(res) = self {
            res
        } else {
            panic!("{:?} is not a gate state", self);
        }
    }
    fn gate(&self) -> &GateState {
        if let Self::Gate(res) = self {
            res
        } else {
            panic!("{:?} is not a gate state", self);
        }
    }
    pub fn pipeline_gadget(&self) -> &PipelineGadgetState {
        if let Self::PipelineGadget(res) = self {
            res
        } else {
            panic!("{:?} is not a pipeline gadget state", self);
        }
    }
    pub fn pipeline_gadget_mut(&mut self) -> &mut PipelineGadgetState {
        if let Self::PipelineGadget(res) = self {
            res
        } else {
            panic!("{:?} is not a pipeline gadget state", self);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleState {
    pub instance_states: InstanceVec<Option<EvaluatorState>>,
    pub wire_states: WireVec<Option<WireState>>, // None for "uninit", allows to check correct evaluation order.
    next_eval_query_id: usize,                   // index in eval_order
    next_check_query_id: usize,                  // index in eval_order
}

#[derive(Debug, Clone)]
pub struct GateState {
    prev_inputs: InputVec<Option<WireState>>,
    inputs: InputVec<Option<WireState>>,
    //output: Option<WireState>,
}
#[derive(Debug, Clone)]

pub struct PipelineGadgetState {
    inputs: LatencyVec<InputVec<Option<WireState>>>,
    valid_output_latencies: LatencyVec<Option<bool>>,
    module_state: ModuleState,
}

impl Evaluator for ModuleEvaluator {
    fn uninit_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Module(self.uninit_state_inner(netlist))
    }
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState {
        let state = prev_state.module();
        EvaluatorState::Module(self.init_next_inner(prev_state.module(), netlist))
    }
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Module(self.x_state_inner(netlist))
    }
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> EvaluatorState {
        EvaluatorState::Module(self.state_from_vcd_inner(vcd, netlist))
    }
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        self.set_input_inner(state.module_mut(), input, input_state, netlist);
    }
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState {
        self.eval_output_inner(out, state.module_mut(), sim_state, netlist)
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        self.eval_finish_inner(state.module_mut(), sim_state, netlist);
    }
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let state = state.module_mut();
        self.check_safe_out_inner(out, state, sim_state, netlist)
    }
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let state = state.module_mut();
        self.check_safe_finish_inner(state, sim_state, netlist)
    }
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String> {
        let module = &netlist[self.module_id];
        let inst_id = self
            .inst_ids
            .binary_search_by(|eval_instance| eval_instance.insts.compare(&ginst).reverse())
            .unwrap();
        if let Some(evaluator) = &self.instance_evaluators[inst_id] {
            if let Some(path) = evaluator.glob_inst2path(ginst, netlist) {
                return Some(format!("{}.{}", module.instances[inst_id].name, path));
            }
        }
        Some(module.instances[inst_id].name.clone())
    }
}

impl Evaluator for GateEvaluator {
    fn uninit_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Gate(GateState {
            prev_inputs: InputVec::from_vec(vec![None; self.gate.input_ports().len()]),
            inputs: InputVec::from_vec(vec![None; self.gate.input_ports().len()]),
        })
    }
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState {
        let prev_state = prev_state.gate();
        EvaluatorState::Gate(GateState {
            prev_inputs: prev_state.inputs.clone(),
            inputs: InputVec::from_vec(vec![None; self.gate.input_ports().len()]),
        })
    }
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Gate(GateState {
            prev_inputs: InputVec::from_vec(vec![
                Some(WireState::control());
                self.gate.input_ports().len()
            ]),
            inputs: InputVec::from_vec(vec![
                Some(WireState::control());
                self.gate.input_ports().len()
            ]),
        })
    }
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> EvaluatorState {
        EvaluatorState::Gate(GateState {
            prev_inputs: InputVec::from_vec(vec![
                Some(WireState::control());
                self.gate.input_ports().len()
            ]),
            inputs: self
                .gate
                .input_port_names()
                .iter()
                .map(|name| {
                    Some(vs2ws(
                        vcd.lookup(vec![(*name).to_owned()], 0, 0).unwrap().unwrap(),
                    ))
                })
                .collect(),
            /*
            output: Some(vs2ws(
                vcd.lookup(vec![self.gate.output_port_name().to_owned()], 0, 0)
                    .unwrap()
                    .unwrap(),
            )),
            */
        })
    }
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let state = state.gate_mut();
        state.inputs[input] = Some(input_state);
    }
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState {
        let state = state.gate_mut();
        match self.gate {
            Gate::CombUnitary(ugate) => {
                let op = state.inputs[0].as_ref().unwrap();
                match ugate {
                    CombUnitary::Buf => op.clone(),
                    CombUnitary::Not => {
                        sim_state.leak_random(&op, self.inst_id);
                        op.negate()
                    }
                }
            }
            Gate::CombBinary(bgate) => {
                let op0 = state.inputs[0].as_ref().unwrap();
                let op1 = state.inputs[1].as_ref().unwrap();
                bgate.sim(op0, op1, sim_state, self.inst_id)
            }
            Gate::Mux => {
                let op0 = state.inputs[0].as_ref().unwrap();
                let op1 = state.inputs[1].as_ref().unwrap();
                let ops = state.inputs[2].as_ref().unwrap();
                super::simulation::sim_mux(op0, op1, ops, sim_state, self.inst_id)
            }
            Gate::Dff => {
                if let Some(wire_state) = state.inputs[1].as_ref() {
                    sim_state.store_random(wire_state);
                }
                state.prev_inputs[1].clone().unwrap_or(WireState::control())
            }
        }
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
    }
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let state = state.gate();
        let sensitive_current = state.inputs.iter().fold(ShareSet::empty(), |x, y| {
            x.union(y.as_ref().unwrap().sensitivity)
        });
        if sensitive_current.len() > 1 {
            bail!(
                "Gate has input sensitive in multiple shares (causes glitch leakage):\n\t{}",
                self.gate
                    .input_ports()
                    .iter_enumerated()
                    .map(|(input_id, input_name)| format!(
                        "Input {}, shares: {}",
                        input_name,
                        state.inputs[input_id].as_ref().unwrap().sensitivity,
                    ))
                    .collect::<Vec<_>>()
                    .join("\n\t")
            );
        }
        let sensitive_prev = state
            .inputs
            .iter()
            .map(|x| x.as_ref().unwrap().sensitivity)
            .fold(ShareSet::empty(), |x, y| x.union(y));
        let sensitive_transition = sensitive_current.union(sensitive_prev);
        if sensitive_transition.len() > 1 {
            bail!(
                "Gate has input sensitive in multiple shares over consecutive cycles (transition leakage):\n\t{}",
                self.gate
                    .input_ports()
                    .iter_enumerated()
                    .map(|(input_id, input_name)| format!(
                        "Input {}, shares: {}, shares previous cycle: {}",
                        input_name,
                        state.inputs[input_id].as_ref().unwrap().sensitivity,
                        state.prev_inputs[input_id].as_ref().unwrap().sensitivity,
                    ))
                    .collect::<Vec<_>>()
                    .join("\n\t")
            );
        }
        Ok(())
    }
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String> {
        None
    }
}

impl Evaluator for TieEvaluator {
    fn uninit_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        unreachable!("A Tie has no input.")
    }
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState {
        WireState::control().with_value(Some(self.value))
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
    }
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String> {
        None
    }
}

impl Evaluator for PipelineGadgetEvaluator {
    fn uninit_state(&self, netlist: &Netlist) -> EvaluatorState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        self.state_from(
            LatencyVec::from_vec(vec![
                InputVec::from_vec(vec![None; gadget.input_roles.len()]);
                (gadget.max_latency + 1usize).into()
            ]),
            self.module_evaluator.uninit_state_inner(netlist),
            netlist,
        )
    }
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        let prev_state = prev_state.pipeline_gadget();
        self.state_from(
            std::iter::once(InputVec::from_vec(vec![None; gadget.input_roles.len()]))
                .chain(prev_state.inputs[..gadget.max_latency].iter().cloned())
                .collect(),
            self.module_evaluator
                .init_next_inner(&prev_state.module_state, netlist),
            netlist,
        )
    }
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        self.state_from(
            LatencyVec::from_vec(vec![
                InputVec::from_vec(vec![
                    Some(WireState::control());
                    gadget.input_roles.len()
                ]);
                (gadget.max_latency + 1usize).into()
            ]),
            self.module_evaluator.x_state_inner(netlist),
            netlist,
        )
    }
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> EvaluatorState {
        todo!()
    }
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let gadget = netlist.gadget(self.module_id).unwrap();
        let module = &netlist[self.module_id];
        let state = state.pipeline_gadget_mut();
        state.inputs[Latency::from_raw(0)][input] = Some(input_state.clone());
        self.module_evaluator
            .set_input_inner(&mut state.module_state, input, input_state, netlist);
        assert_eq!(
            state.module_state.wire_states[module.connection_wires[module.input_ports[input]]],
            state.inputs[0][input]
        );
    }
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()> {
        let state = state.pipeline_gadget_mut();
        let module = &netlist[self.module_id];
        let gadget = netlist.gadget(self.module_id).unwrap();
        let wire_state = state.inputs[0][input].as_ref().unwrap();
        match &gadget.input_roles[input] {
            InputRole::Share(share_id) => {
                if !wire_state.sensitivity.subset_of(ShareSet::from(*share_id)) {
                    Err(anyhow!(
                        "Input share index {} is sensitive for shares {}",
                        share_id,
                        wire_state.sensitivity
                    ))
                } else if !wire_state.sensitivity.subset_of(ShareSet::from(*share_id)) {
                    Err(anyhow!(
                        "Input share index {} is glitch-sensitive for shares {}",
                        share_id,
                        wire_state.glitch_sensitivity
                    ))
                } else {
                    Ok(())
                }
            }
            InputRole::Random(_) => {
                if !wire_state.glitch_sensitivity.is_empty() {
                    Err(anyhow!(
                        "Randomness input is (glitch-)sensitive for shares {}",
                        wire_state.glitch_sensitivity
                    ))
                } else {
                    Ok(())
                }
            }
            InputRole::Control => {
                if !wire_state.deterministic {
                    Err(anyhow!(
                        "Control input is not a deterministic value (it is share- or random-dependent)",
                    ))
                } else if !wire_state.glitch_sensitivity.is_empty() {
                    Err(anyhow!(
                        "Control input depends of share glitches."
                    ))
                } else {
                    Ok(())
                }
            }
        }
        .with_context(|| {
            format!(
                "Unsafe state for input {}",
                module.ports[module.input_ports[input]],
            )
        })?;
        if wire_state
            .nspgi_dep
            .last(self.nspgi_id)
            .is_some_and(|lat| sim_state.last_det_exec[self.nspgi_id] < lat)
        {
            bail!("Input {} depends on a previous execution of this gadget, there was no pipeline bubble since then.",
                module.ports[module.input_ports[input]],
        );
        }
        Ok(())
    }
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState {
        let state = state.pipeline_gadget_mut();
        let gadget = netlist.gadget(self.module_id).unwrap();
        let module = &netlist[self.module_id];
        let out_lats = &gadget.valid_latencies[module.output_ports[out]];
        assert_eq!(out_lats.len(), Latency::from_raw(1));
        let out_lat = out_lats[0];
        if out_lat == Latency::from_raw(0) {
            assert!(
                state.inputs[0].iter().all(|x| x.is_some()),
                "gadget {}, inputs: {:?}",
                module.name,
                state.inputs[0]
            );
        }
        let super::gadget::OutputRole::Share(share_id) = gadget.output_roles[out] else {
            panic!("Output {out} is not a share.")
        };
        // Here we use the module simulator to find out if the input is sensitive, and how the
        // randomness is used. Should be done without looking at the internals.
        let mut res = self.module_evaluator.eval_output_inner(
            out,
            &mut state.module_state,
            sim_state,
            netlist,
        );
        // We set sensitivity to the output share. Only exception: when we are deterministic.
        // We can be sensitive, even if we originally weren't (e.g. refresh gadget).
        if res.glitch_deterministic() {
            // Do not change anything here.
        } else {
            res.glitch_sensitivity = ShareSet::from(share_id);
            res.nspgi_dep = res.nspgi_dep.with_dep(
                self.nspgi_id,
                Slatency::from(sim_state.cur_lat()) - Slatency::from(out_lat),
            );
            if !res.deterministic {
                res.sensitivity = ShareSet::from(share_id);
            }
        }
        // Value is un-touched, as well as "deterministic"
        // FIXME: should we restrict "random"-ness?
        res.random = None;
        // FIXME:need to check annotations
        // Module imulation can say output is valid (e.g. MSKcst non-share 0 output), while gadget
        // doesn't.
        res.consistency_check();
        res
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        let state = state.pipeline_gadget_mut();
        self.module_evaluator
            .eval_finish_inner(&mut state.module_state, sim_state, netlist);
        // use the fresh randomness
        // We look back max_input_latency cycles, so we have all required inputs available.
        let module = &netlist[self.module_id];
        let gadget = netlist.gadget(self.module_id).unwrap();
        let exec_deterministic = module
            .input_ports
            .iter_enumerated()
            .any(|(input_id, con_id)| {
                Some(*con_id) == module.clock
                    || state.inputs[gadget.max_input_latency - gadget.valid_latencies[*con_id][0]]
                        [input_id]
                        .as_ref()
                        .unwrap()
                        .glitch_deterministic()
            });
        if exec_deterministic {
            sim_state.nspgi_det_exec(self.nspgi_id);
        } else {
            for (input_id, input_role) in gadget.input_roles.iter_enumerated() {
                if let InputRole::Random(_) = input_role {
                    let lat = gadget.max_input_latency
                        - gadget.valid_latencies[module.input_ports[input_id]][0];
                    sim_state.use_random(
                        &state.inputs[lat][input_id].as_ref().unwrap(),
                        self.module_evaluator.ginst_id,
                        lat,
                    );
                }
            }
        }
    }
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        // FIXME: make this empty only for "assumed" gadgets.
        Ok(())
        /*
        self.module_evaluator.check_safe_out_inner(
            out,
            &mut state.pipeline_gadget_mut().module_state,
            netlist,
        )
            */
    }
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        // FIXME: make this empty only for "assumed" gadgets.
        Ok(())
        /*
        self.module_evaluator
            .check_safe_finish_inner(&mut state.pipeline_gadget_mut().module_state, netlist)
            */
    }
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String> {
        self.module_evaluator.glob_inst2path(ginst, netlist)
    }
}

impl ModuleEvaluator {
    pub fn new(
        module_id: ModuleId,
        netlist: &Netlist,
        queries: Vec<OutputId>,
        used_ids: &mut EvalInstanceIds,
    ) -> Self {
        let module = &netlist[module_id];
        let wg = &module.comb_wire_dag;
        let ginst_id = used_ids.new_inst();
        /*
        eprintln!("module: {}", module.name);
        eprintln!("wg: {:?}", wg);
        eprintln!("queries: {:?}", queries);
        */
        let graph = petgraph::visit::Reversed(&wg.graph);
        let mut dfs = petgraph::visit::DfsPostOrder::empty(&graph);
        let mut eval_order = vec![];
        let mut instance_queries = InstanceVec::from_vec(vec![vec![]; module.instances.len()]);
        let mut eval_dfs = |output_wire| {
            //eprintln!("move dfs to {:?}", output_wire);
            dfs.move_to(wg.node_indices[output_wire]);
            std::iter::from_fn(|| {
                dfs.next(&graph).map(|node_wire_id| {
                    let wire_id = wg.graph[node_wire_id];
                    //eprintln!("found wire {:?}", wire_id);
                    let (instance_id, con_id) = module.wires[wire_id].source;
                    instance_queries[instance_id].push(con_id);
                    wire_id
                })
            })
            // Exclude inputs from wires to evaluate, since they are not computed with
            // eval_wires, but with eval_input.
            .filter(|wire| {
                !matches!(
                    module.instances[module.wires[*wire].source.0].architecture,
                    InstanceType::Input(_, _)
                )
            })
            .collect::<Vec<_>>()
        };
        for output_id in queries {
            let output_con = module.output_ports[output_id];
            let output_wire = module.connection_wires[output_con];
            let eval_wires = eval_dfs(output_wire);
            eval_order.push((output_con, eval_wires));
        }
        let eval_finish = module
            .wires
            .indices()
            .flat_map(|eval_wire_id| eval_dfs(eval_wire_id).into_iter())
            .collect();
        let (instance_evaluators, inst_ids) = module
            .instances
            .iter()
            .zip(instance_queries)
            .map(|(instance, queries)| {
                let mut inst_id_range = used_ids.start_new();
                let res =
                    InstanceEvaluator::new(&instance.architecture, queries, netlist, used_ids);
                inst_id_range.copy_end(used_ids);
                (res, inst_id_range)
            })
            .unzip();
        Self {
            module_id,
            instance_evaluators,
            eval_order,
            eval_finish,
            inst_ids,
            ginst_id,
        }
    }
    fn eval_wires(
        &self,
        wires: &[WireId],
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        for wire in wires {
            self.eval_wire(*wire, state, sim_state, netlist);
        }
    }
    fn eval_wire(
        &self,
        wire: WireId,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        let module = &netlist[self.module_id];
        /*
        eprintln!(
            "eval wire {:?} ({:?}) in module {}",
            wire, module.wire_names[*wire], module.name
        );
        */
        let (src_inst_id, src_con) = module.wires[wire].source;
        let instance = &module.instances[src_inst_id];
        if !matches!(instance.architecture, InstanceType::Input(..)) {
            let res = self.instance_evaluators[src_inst_id]
                .as_ref()
                .unwrap()
                .eval_output(
                    src_con,
                    state.instance_states[src_inst_id].as_mut().unwrap(),
                    sim_state,
                    netlist,
                );
            state.wire_states[wire] = Some(res);
        }
        self.eval_fanout(wire, state, netlist);
        //eprintln!("eval wire done");
    }
    fn eval_fanout(&self, wire: WireId, state: &mut ModuleState, netlist: &Netlist) {
        //eprintln!("eval fanout of wire {}", wire);
        for (instance_id, input_id) in &netlist[self.module_id].wires[wire].sinks {
            if let Some(sub_evaluator) = &self.instance_evaluators[*instance_id] {
                /*
                eprintln!(
                    "\tinstance {}, input {} (con: {})",
                    netlist[self.module_id].instances[*instance_id].name,
                    *input_id,
                    netlist[self.module_id].input_ports[*input_id]
                );
                */
                sub_evaluator.set_input(
                    state.instance_states[*instance_id].as_mut().unwrap(),
                    *input_id,
                    state.wire_states[wire].clone().unwrap(),
                    netlist,
                );
            }
        }
        //eprintln!("eval fanout done");
    }
    fn check_fanout(
        &self,
        wire: WireId,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        for (instance_id, input_id) in &netlist[self.module_id].wires[wire].sinks {
            if let Some(sub_evaluator) = &self.instance_evaluators[*instance_id] {
                sub_evaluator.check_safe_input(
                    state.instance_states[*instance_id].as_mut().unwrap(),
                    sim_state,
                    *input_id,
                    netlist,
                )?;
            }
        }
        Ok(())
    }
    pub fn vcd_inputs(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        time_offset: usize,
        netlist: &Netlist,
    ) -> Result<InputVec<WireState>> {
        let module = &netlist[self.module_id];
        Ok(module
            .input_ports
            .iter()
            .map(|con_id| {
                let wire_name = &module.ports[*con_id];
                let v = vcd
                    .lookup(vec![wire_name.name.clone()], time_offset, wire_name.offset)
                    .unwrap()
                    .unwrap_or_else(|| {
                        panic!("No value for {:?}, wire {}", vcd.root_module(), wire_name)
                    })
                    .to_bool();
                WireState::control().with_value(v.map(|v| v.into()))
            })
            .collect())
    }
    fn uninit_state_inner(&self, netlist: &Netlist) -> ModuleState {
        let instance_states = self
            .instance_evaluators
            .iter()
            .map(|eval| eval.as_ref().map(|eval| eval.uninit_state(netlist)))
            .collect();
        let wire_states = WireVec::from_vec(vec![None; netlist[self.module_id].wires.len()]);
        let next_eval_query_id = 0;
        let next_check_query_id = 0;
        ModuleState {
            instance_states,
            wire_states,
            next_eval_query_id,
            next_check_query_id,
        }
    }
    fn init_next_inner(&self, prev_state: &ModuleState, netlist: &Netlist) -> ModuleState {
        let instance_states = izip!(&self.instance_evaluators, &prev_state.instance_states)
            .map(|(eval, state)| {
                eval.as_ref()
                    .map(|eval| eval.init_next(state.as_ref().unwrap(), netlist))
            })
            .collect();
        let wire_states = WireVec::from_vec(vec![None; netlist[self.module_id].wires.len()]);
        let next_eval_query_id = 0;
        let next_check_query_id = 0;
        ModuleState {
            instance_states,
            wire_states,
            next_eval_query_id,
            next_check_query_id,
        }
    }
    fn x_state_inner(&self, netlist: &Netlist) -> ModuleState {
        let instance_states = self
            .instance_evaluators
            .iter()
            .map(|eval| eval.as_ref().map(|eval| eval.x_state(netlist)))
            .collect();
        let wire_states = WireVec::from_vec(vec![
            Some(WireState::control());
            netlist[self.module_id].wires.len()
        ]);
        let next_eval_query_id = self.eval_order.len() + 1;
        let next_check_query_id = 0;
        ModuleState {
            instance_states,
            wire_states,
            next_eval_query_id,
            next_check_query_id,
        }
    }
    fn state_from_vcd_inner(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> ModuleState {
        let module = &netlist[self.module_id];
        let instance_states = self
            .instance_evaluators
            .iter_enumerated()
            .map(|(instance_id, eval)| {
                eval.as_ref().map(|eval| {
                    let instance_name = module.instances[instance_id].name.clone();
                    eval.state_from_vcd(&mut vcd.submodule(instance_name, 0), netlist)
                })
            })
            .collect();
        let wire_states = module
            .wire_names
            .iter()
            .map(|opt_name| {
                let v = opt_name.as_ref().and_then(|wire_name| {
                    vcd.lookup(vec![wire_name.name.clone()], 0, wire_name.offset)
                        .unwrap()
                        .unwrap()
                        .to_bool()
                });
                Some(WireState::control().with_value(v.map(|v| v.into())))
            })
            .collect();
        let next_eval_query_id = self.eval_order.len() + 1;
        let next_check_query_id = 0;
        ModuleState {
            instance_states,
            wire_states,
            next_eval_query_id,
            next_check_query_id,
        }
    }
    fn set_input_inner(
        &self,
        state: &mut ModuleState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let module = &netlist[self.module_id];
        let con_id = module.input_ports[input];
        let wire_id = module.connection_wires[con_id];
        if input_state.random.is_some() {
            /*
            eprintln!(
                "eval_input {} {:?} in module {}",
                wire_id, module.ports[con_id], module.name,
            );
            */
        }
        assert!(state.wire_states[wire_id].is_none());
        state.wire_states[wire_id] = Some(input_state);
        self.eval_fanout(wire_id, state, netlist);
    }
    fn eval_output_inner(
        &self,
        out: OutputId,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> WireState {
        let module = &netlist[self.module_id];
        let (next_con_id, next_wires) = &self.eval_order[state.next_eval_query_id];
        let out = module.output_ports[out];
        assert_eq!(out, *next_con_id);
        self.eval_wires(next_wires.as_slice(), state, sim_state, netlist);
        state.next_eval_query_id += 1;
        state.wire_states[module.connection_wires[out]]
            .clone()
            .unwrap()
    }
    fn eval_finish_inner(
        &self,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        assert_eq!(state.next_eval_query_id, self.eval_order.len());
        self.eval_wires(self.eval_finish.as_slice(), state, sim_state, netlist);
        state.next_eval_query_id += 1;
        for (evaluator, state) in
            itertools::izip!(&self.instance_evaluators, &mut state.instance_states,)
        {
            if let (Some(evaluator), Some(state)) = (evaluator, state.as_mut()) {
                evaluator.eval_finish(state, sim_state, netlist);
            } else {
                assert!(evaluator.is_none() && state.is_none());
            }
        }
    }
    fn check_safe_out_inner(
        &self,
        out: OutputId,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let module = &netlist[self.module_id];
        let (next_con_id, next_wires) = &self.eval_order[state.next_check_query_id];
        let out = module.output_ports[out];
        assert_eq!(out, *next_con_id);
        for wire_id in next_wires {
            self.check_wire(*wire_id, state, sim_state, netlist)?;
        }
        state.next_check_query_id += 1;
        Ok(())
    }
    fn check_safe_finish_inner(
        &self,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let module = &netlist[self.module_id];
        assert_eq!(
            state.next_check_query_id,
            self.eval_order.len(),
            "{}",
            netlist[self.module_id].name
        );
        for wire in &self.eval_finish {
            self.check_wire(*wire, state, sim_state, netlist)?;
        }
        state.next_check_query_id += 1;
        for ((instance_id, evaluator), state) in itertools::izip!(
            self.instance_evaluators.iter_enumerated(),
            &mut state.instance_states,
        ) {
            if let (Some(evaluator), Some(state)) = (evaluator, state.as_mut()) {
                evaluator
                    .check_safe_finish(state, sim_state, netlist)
                    .with_context(|| {
                        format!(
                            "In module {}, checking instance {}.",
                            module.name, module.instances[instance_id].name
                        )
                    })?;
            } else {
                assert!(evaluator.is_none() && state.is_none());
            }
        }
        Ok(())
    }
    fn check_wire(
        &self,
        wire: WireId,
        state: &mut ModuleState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let module = &netlist[self.module_id];
        let (src_inst_id, src_con) = module.wires[wire].source;
        if let Some(evaluator) = self.instance_evaluators[src_inst_id].as_ref() {
            evaluator
                .check_safe_out(
                    src_con,
                    state.instance_states[src_inst_id].as_mut().unwrap(),
                    sim_state,
                    netlist,
                )
                .with_context(|| {
                    format!(
                        "In module {}, checking instance {}",
                        module.name, module.instances[src_inst_id].name
                    )
                })?;
        }
        state.wire_states[wire]
            .as_ref()
            .unwrap()
            .check_secure()
            .with_context(|| {
                format!(
                    "Checking wire {:?} in module {}",
                    module.wire_names[wire], module.name
                )
            })?;
        self.check_fanout(wire, state, sim_state, netlist)?;
        Ok(())
    }
}

fn vs2ws(vs: super::clk_vcd::VarState) -> WireState {
    WireState::control().with_value(vs.to_bool().map(|v| v.into()))
}

impl PipelineGadgetEvaluator {
    fn new(
        module_id: ModuleId,
        netlist: &Netlist,
        queries: Vec<OutputId>,
        used_ids: &mut EvalInstanceIds,
    ) -> Self {
        let nspgi_id = used_ids.new_nspgi();
        Self {
            module_id,
            nspgi_id,
            module_evaluator: ModuleEvaluator::new(
                module_id,
                netlist,
                queries,
                &mut EvalInstanceIds::new(),
            ),
        }
    }
    fn state_from(
        &self,
        inputs: LatencyVec<InputVec<Option<WireState>>>,
        module_state: ModuleState,
        netlist: &Netlist,
    ) -> EvaluatorState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        let valid_output_latencies =
            LatencyVec::from_vec(vec![None; gadget.max_latency.index() + 1]);
        assert_eq!(
            valid_output_latencies.len(),
            <usize as From<_>>::from(gadget.max_latency + 1)
        );
        EvaluatorState::PipelineGadget(PipelineGadgetState {
            inputs,
            valid_output_latencies,
            module_state,
        })
    }
    /*
    fn output_lat_valid(&self, lat: Latency, state: &mut PipelineGadgetState, netlist: &Netlist) {
        let gadget = netlist.gadget(self.module_id).unwrap();
        let module = &netlist[self.module_id];
        state.valid_output_latencies[lat] = Some(
            module
                .input_ports
                .iter_enumerated()
                .filter_map(|(input_id, con_id)| {
                    if module.clock == Some(*con_id) {
                        None
                    } else {
                        let input_lats = &gadget.valid_latencies[*con_id];
                        assert_eq!(input_lats.len(), 1);
                        let input_lat = input_lats[0];
                        let lat_diff = lat.checked_sub(input_lat)?;
                        Some(
                            state.inputs[lat_diff][input_id]
                                .expect("uninitialized input")
                                .valid,
                        )
                    }
                })
                .all(std::convert::identity),
        );
    }

    fn out_valid(
        &self,
        state: &mut PipelineGadgetState,
        out_lat: Latency,
        netlist: &Netlist,
    ) -> bool {
        if state.valid_output_latencies[out_lat].is_none() {
            self.output_lat_valid(out_lat, state, netlist);
        }
        state.valid_output_latencies[out_lat].unwrap()
    }
    */
}

impl InstanceEvaluator {
    fn new(
        architecture: &InstanceType,
        queries: Vec<OutputId>,
        netlist: &Netlist,
        used_ids: &mut EvalInstanceIds,
    ) -> Option<Self> {
        let inst_id = used_ids.new_inst();
        match architecture {
            InstanceType::Gate(gate) => {
                let reg_id = if *gate == Gate::Dff {
                    Some(used_ids.new_reg())
                } else {
                    None
                };
                Some(InstanceEvaluator::Gate(GateEvaluator {
                    gate: *gate,
                    inst_id,
                    reg_id,
                }))
            }
            InstanceType::Module(submodule_id) => match netlist.gadget(*submodule_id) {
                Some(gadget) if gadget.is_pipeline() => Some(InstanceEvaluator::Gadget(
                    PipelineGadgetEvaluator::new(*submodule_id, netlist, queries, used_ids),
                )),
                _ => Some(InstanceEvaluator::Module(ModuleEvaluator::new(
                    *submodule_id,
                    netlist,
                    queries,
                    used_ids,
                ))),
            },
            InstanceType::Input(..) => None,
            InstanceType::Tie(value) => {
                Some(InstanceEvaluator::Tie(TieEvaluator { value: *value }))
            }
        }
    }
}

impl EvalInstanceIds {
    pub fn new() -> Self {
        Self {
            nspgi: Range::new(0.into(), 0.into()),
            regs: Range::new(0.into(), 0.into()),
            insts: Range::new(0.into(), 0.into()),
        }
    }
    fn start_new(&self) -> Self {
        Self {
            nspgi: Range::new(self.nspgi.end, self.nspgi.end),
            regs: Range::new(self.regs.end, self.regs.end),
            insts: Range::new(self.insts.end, self.insts.end),
        }
    }
    fn copy_end(&mut self, other: &Self) {
        self.nspgi.end = other.nspgi.end;
        self.regs.end = other.regs.end;
        self.insts.end = other.insts.end;
    }
    fn new_nspgi(&mut self) -> NspgiId {
        let res = self.nspgi.end;
        self.nspgi.end += 1;
        res
    }
    fn new_reg(&mut self) -> RegInstId {
        let res = self.regs.end;
        self.regs.end += 1;
        res
    }
    fn new_inst(&mut self) -> GlobInstId {
        let res = self.insts.end;
        self.insts.end += 1;
        res
    }
}

impl<T> Range<T> {
    fn new(start: T, end: T) -> Self {
        Self { start, end }
    }
}
impl<T> Range<T>
where
    T: Ord,
{
    fn compare(&self, other: &T) -> std::cmp::Ordering {
        if other < &self.start {
            std::cmp::Ordering::Less
        } else if other < &self.end {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    }
}
