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
use super::gadget::{Latency, LatencyVec, PortRole};
use super::module::gates::{CombUnitary, Gate};
use super::module::{
    ConnectionId, InputId, InputVec, InstanceId, InstanceType, InstanceVec, OutputId, WireId,
    WireVec,
};
use super::netlist::{ModList, Netlist};
use super::simulation::{NspgiDep, WireState};
use super::top_sim::GlobSimulationState;
use super::{ModuleId, WireValue};
use crate::share_set::ShareSet;
use crate::top_sim::GadgetExecCycle;
use crate::type_utils::new_id;
use anyhow::{anyhow, bail, Context, Result};
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
    insts: Range<GlobInstId>,
}

#[enum_dispatch::enum_dispatch]
pub trait Evaluator {
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState;
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState;
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
        // We use an Option<_> here to disable all state-setting operation when simulating the
        // ModuleState of a PipelineGadget.
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) -> WireState;
    #[allow(unused_variables)]
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        // We use an Option<_> here to disable all state-setting operation when simulating the
        // ModuleState of a PipelineGadget.
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
    }
    #[allow(unused_variables)]
    fn check_safe_input(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        input: InputId,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    #[allow(unused_variables)]
    fn check_safe_out(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    #[allow(unused_variables)]
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        Ok(())
    }
    #[allow(unused_variables)]
    fn glob_inst2path(&self, ginst: GlobInstId, netlist: &Netlist) -> Option<String> {
        None
    }
}

#[enum_dispatch::enum_dispatch(Evaluator)]
#[derive(Debug, Clone)]
enum InstanceEvaluator {
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
    gate: Gate,
    inst_id: GlobInstId,
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
    output_states: LatencyVec<Option<PipelineStageStatus>>,
    module_state: ModuleState,
}

#[derive(Debug, Clone)]
struct PipelineStageStatus {
    // All inputs are deterministic.
    deterministic: bool,
    // Any input is sensitive.
    sensitive: bool,
    // Any input at the same lat is glitch-sensitive.
    glitch_sensitive: bool,
    // All randomness input are random.
    randomness: bool,
    // NSPGI dependencies
    nspgi_dep: NspgiDep,
}

impl PipelineStageStatus {
    fn from_inputs<'a>(
        nspgi_id: NspgiId,
        lat: Option<GadgetExecCycle>,
        wires: impl Iterator<Item = (&'a PortRole, &'a WireState, bool)>,
    ) -> Self {
        let nspgi_dep = if let Some(lat) = lat {
            NspgiDep::single(nspgi_id, lat)
        } else {
            NspgiDep::empty()
        };
        let init = Self {
            deterministic: true,
            sensitive: false,
            glitch_sensitive: false,
            randomness: true,
            nspgi_dep,
        };
        wires.fold(init, |mut state, (input_role, wire_state, same_cycle)| {
            state.deterministic &= wire_state.deterministic;
            state.glitch_sensitive |=
                wire_state.sensitive() | (same_cycle & wire_state.glitch_sensitive());
            state.sensitive |= wire_state.sensitive();
            if matches!(input_role, PortRole::Random(_)) {
                state.randomness &= wire_state.random.is_some();
            }
            state.nspgi_dep = state.nspgi_dep.max(&wire_state.nspgi_dep);
            state
        })
    }
}

impl Evaluator for ModuleEvaluator {
    fn init_next(&self, prev_state: &EvaluatorState, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Module(self.init_next_inner(prev_state.module(), netlist))
    }
    fn x_state(&self, netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Module(self.x_state_inner(netlist))
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
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) -> WireState {
        self.eval_output_inner(out, state.module_mut(), sim_state, netlist)
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
        self.eval_finish_inner(state.module_mut(), sim_state, netlist);
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
        let module = netlist.module(self.module_id);
        if ginst == self.ginst_id {
            return None;
        }
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
    fn init_next(&self, prev_state: &EvaluatorState, _netlist: &Netlist) -> EvaluatorState {
        let prev_state = prev_state.gate();
        EvaluatorState::Gate(GateState {
            prev_inputs: prev_state.inputs.clone(),
            inputs: InputVec::from_vec(vec![None; self.gate.input_ports().len()]),
        })
    }
    fn x_state(&self, _netlist: &Netlist) -> EvaluatorState {
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
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        _netlist: &Netlist,
    ) {
        let state = state.gate_mut();
        state.inputs[input] = Some(input_state);
    }
    fn eval_output(
        &self,
        _out: OutputId,
        state: &mut EvaluatorState,
        sim_state: Option<&mut GlobSimulationState>,
        _netlist: &Netlist,
    ) -> WireState {
        let state = state.gate_mut();
        match self.gate {
            Gate::CombUnitary(ugate) => {
                let op = state.inputs[0].as_ref().unwrap();
                match ugate {
                    CombUnitary::Buf => op.clone(),
                    CombUnitary::Not => {
                        if let Some(sim_state) = sim_state {
                            sim_state.leak_random(op, self.inst_id);
                        }
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
                    if let Some(sim_state) = sim_state {
                        sim_state.store_random(wire_state);
                    }
                }
                // TODO: only stop glitches for some specifically-marked gates?
                state.prev_inputs[1]
                    .clone()
                    .unwrap_or(WireState::control())
                    .stop_glitches()
            }
        }
    }
    fn check_safe_out(
        &self,
        _out: OutputId,
        state: &mut EvaluatorState,
        _sim_state: &mut GlobSimulationState,
        _netlist: &Netlist,
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
}

impl Evaluator for TieEvaluator {
    fn init_next(&self, _prev_state: &EvaluatorState, _netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn x_state(&self, _netlist: &Netlist) -> EvaluatorState {
        EvaluatorState::Tie
    }
    fn set_input(
        &self,
        _state: &mut EvaluatorState,
        _input: InputId,
        _input_state: WireState,
        _netlist: &Netlist,
    ) {
        unreachable!("A Tie has no input.")
    }
    fn eval_output(
        &self,
        _out: OutputId,
        _state: &mut EvaluatorState,
        _sim_state: Option<&mut GlobSimulationState>,
        _netlist: &Netlist,
    ) -> WireState {
        WireState::control().with_value(Some(self.value))
    }
}

impl Evaluator for PipelineGadgetEvaluator {
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
    fn set_input(
        &self,
        state: &mut EvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let module = netlist.module(self.module_id);
        /*
        eprintln!(
            "set_input {} {}/{}",
            module.name,
            input,
            module.input_ports.len()
        );
        */
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
        let module = netlist.module(self.module_id);
        let gadget = netlist.gadget(self.module_id).unwrap();
        let wire_state = state.inputs[0][input].as_ref().unwrap();
        match &gadget.input_roles[input] {
            PortRole::Share(share_id) => {
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
            PortRole::Random(_) => {
                if !wire_state.glitch_sensitivity.is_empty() {
                    Err(anyhow!(
                        "Randomness input is (glitch-)sensitive for shares {}",
                        wire_state.glitch_sensitivity
                    ))
                } else {
                    Ok(())
                }
            }
            PortRole::Control => {
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
        if gadget.prop.requires_bubble()
            && wire_state.nspgi_dep.last(self.nspgi_id).is_some_and(|dep| {
                sim_state.last_nonsensitive_exec[self.nspgi_id]
                    .map(|last| last < dep)
                    .unwrap_or(true)
            })
        {
            bail!("Input {} depends on a previous execution of this gadget, there was no pipeline bubble since then.",
                module.ports[module.input_ports[input]],
        );
        }
        Ok(())
    }
    fn check_safe_finish(
        &self,
        state: &mut EvaluatorState,
        _sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<()> {
        let module = netlist.module(self.module_id);
        let gadget = netlist.gadget(self.module_id).unwrap();
        let state = state.pipeline_gadget_mut();
        let all_in_status = state.output_states[gadget.max_input_latency]
            .as_ref()
            .unwrap();
        if all_in_status.sensitive {
            for input_id in &gadget.rnd_ports {
                let lat = gadget.input_maxrellat(*input_id, netlist);
                let random_wire_state = state.inputs[lat][*input_id].as_ref().unwrap();
                if random_wire_state.random.is_none() {
                    bail!("Gadget execution has at least one sensitive input but randomness wire {} is not a fresh random", module.ports[module.input_ports[*input_id]]);
                }
            }
        }
        Ok(())
    }
    fn eval_output(
        &self,
        out: OutputId,
        state: &mut EvaluatorState,
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) -> WireState {
        let state = state.pipeline_gadget_mut();
        let gadget = netlist.gadget(self.module_id).unwrap();
        let module = netlist.module(self.module_id);
        let out_lat = gadget.latency[module.output_ports[out]];
        if out_lat == Latency::from_raw(0) {
            assert!(
                state.inputs[0].iter().all(|x| x.is_some()),
                "gadget {}, inputs: {:?}",
                module.name,
                state.inputs[0],
            );
        }
        // We use the module simulator only for evaluating the value of the output.
        // Here we do not forward the sim_state: in module and gate, it is used only for leaking
        // randoms, which we want to handle at the border of the gadget.
        let res =
            self.module_evaluator
                .eval_output_inner(out, &mut state.module_state, None, netlist);
        // Let us evaluate the output based solely on the gadget annotations.
        let out_status = self.out_status(state, out_lat, sim_state.unwrap(), netlist);
        let share_id = gadget.output_share_id[out];
        let g_res = WireState {
            sensitivity: ShareSet::from(share_id).clear_if(!out_status.sensitive),
            glitch_sensitivity: ShareSet::from(share_id).clear_if(!out_status.glitch_sensitive),
            value: res.value, // Here we need actual eval.
            random: None,
            deterministic: out_status.deterministic,
            nspgi_dep: out_status.nspgi_dep.clone(),
        };
        if out_status.sensitive {
            assert!(out_status.glitch_sensitive);
        }
        /*
        eprintln!("module: {}", module.name);
        eprintln!("res: {:?}", res);
        eprintln!("g_res: {:?}", g_res);
        eprintln!("out_status: {:?}", out_status);
        eprintln!("state.inputs: {:?}", state.inputs);
        eprintln!("state.output_states: {:?}", state.output_states);
        */
        if !g_res.sensitive() {
            assert!(!res.sensitive());
        }
        if !g_res.glitch_sensitive() {
            assert!(!res.glitch_sensitive());
        }
        if g_res.deterministic {
            assert!(res.deterministic);
        }
        assert!(g_res.nspgi_dep.is_larger_than(&res.nspgi_dep));
        g_res.consistency_check();
        g_res
    }
    fn eval_finish(
        &self,
        state: &mut EvaluatorState,
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
        let state = state.pipeline_gadget_mut();
        let sim_state = sim_state.unwrap();
        self.module_evaluator
            .eval_finish_inner(&mut state.module_state, None, netlist);
        // use the fresh randomness
        // We look back max_input_latency cycles, so we have all required inputs available.
        let module = netlist.module(self.module_id);
        let gadget = netlist.gadget(self.module_id).unwrap();
        self.compute_output_state(state, gadget.max_input_latency, sim_state, netlist);
        let all_in_status = state.output_states[gadget.max_input_latency]
            .as_ref()
            .unwrap();
        // FIXME: are glitch-sensitive-only gadget counting as pipeline bubble ?
        if !all_in_status.glitch_sensitive {
            // Straight conversion of GlobSimCycle -> GadgetExecCycle because all_in_status is
            // tied to gadget.max_input_latency.
            sim_state.nspgi_det_exec(
                self.nspgi_id,
                GadgetExecCycle::from_global(sim_state.cur_lat()),
            );
        }
        // For random, it is not needed to be fresh if input is sensitive only in glitch domain:
        // glitches can remove randomness anyway. (Randomness is still "leaked").
        for input_id in &gadget.rnd_ports {
            let lat = gadget.max_input_latency - gadget.latency[module.input_ports[*input_id]];
            let random_wire_state = state.inputs[lat][*input_id].as_ref().unwrap();
            let ginst_id = self.module_evaluator.ginst_id;
            /*
            eprintln!(
                "use_random module: {}, ginst_id: {}, input_id: {}, wire: {:?}",
                module.name, ginst_id, input_id, random_wire_state
            );
            */
            if all_in_status.sensitive {
                sim_state.use_random(random_wire_state, ginst_id, lat);
            }
            sim_state.leak_random(random_wire_state, ginst_id);
            // We don't know what to do with the random until we are late enough, but until then we
            // have to say that the random is stored in the gadget, otherwise its state is not
            // tracked anymore.
            for store_lat in 0..lat.index() {
                /*
                eprintln!(
                    "store_lat: {} max_input_latency: {}, wire_state: {:?}",
                    store_lat,
                    gadget.max_input_latency,
                    state.inputs[store_lat][*input_id].as_ref().unwrap()
                );
                */
                sim_state.store_random(state.inputs[store_lat][*input_id].as_ref().unwrap());
            }
        }
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
        instance_path: Vec<String>,
    ) -> Self {
        let module = netlist.module(module_id);
        let wg = &netlist.module_comb_deps(module_id).comb_wire_dag;
        let ginst_id = used_ids.new_inst();
        /*
        eprintln!("new module {}, ginst_id: {:?}", module.name, ginst_id);
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
        let (instance_evaluators, inst_ids): (_, InstanceVec<_>) = module
            .instances
            .iter()
            .zip(instance_queries)
            .map(|(instance, queries)| {
                let mut inst_id_range = used_ids.start_new();
                /*
                eprintln!(
                    "module {}, before: used_ids: {:?}, inst_id_range: {:?}",
                    module.name, used_ids, inst_id_range
                );
                */
                let mut path = instance_path.clone();
                path.push(instance.name.clone());
                let res = InstanceEvaluator::new(
                    &instance.architecture,
                    queries,
                    netlist,
                    &mut inst_id_range,
                    path,
                );
                used_ids.copy_end(&inst_id_range);
                /*
                eprintln!(
                    "module {}, after: used_ids: {:?}, inst_id_range: {:?}",
                    module.name, used_ids, inst_id_range
                );
                */
                (res, inst_id_range)
            })
            .unzip();
        for (r0, r1) in std::iter::zip(&inst_ids[InstanceId::from_raw(1)..], &inst_ids) {
            assert_eq!(r0.insts.start, r1.insts.end);
        }
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
        mut sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
        for wire in wires {
            self.eval_wire(*wire, state, sim_state.as_deref_mut(), netlist);
        }
    }
    fn eval_wire(
        &self,
        wire: WireId,
        state: &mut ModuleState,
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
        let module = netlist.module(self.module_id);
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
        for (instance_id, input_id) in &netlist.module(self.module_id).wires[wire].sinks {
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
        for (instance_id, input_id) in &netlist.module(self.module_id).wires[wire].sinks {
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
    fn init_next_inner(&self, prev_state: &ModuleState, netlist: &Netlist) -> ModuleState {
        let instance_states = izip!(&self.instance_evaluators, &prev_state.instance_states)
            .map(|(eval, state)| {
                eval.as_ref()
                    .map(|eval| eval.init_next(state.as_ref().unwrap(), netlist))
            })
            .collect();
        let wire_states = WireVec::from_vec(vec![None; netlist.module(self.module_id).wires.len()]);
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
            netlist.module(self.module_id).wires.len()
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
    fn set_input_inner(
        &self,
        state: &mut ModuleState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let module = netlist.module(self.module_id);
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
        sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) -> WireState {
        let module = netlist.module(self.module_id);
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
        mut sim_state: Option<&mut GlobSimulationState>,
        netlist: &Netlist,
    ) {
        assert_eq!(state.next_eval_query_id, self.eval_order.len());
        self.eval_wires(
            self.eval_finish.as_slice(),
            state,
            sim_state.as_deref_mut(),
            netlist,
        );
        state.next_eval_query_id += 1;
        for (evaluator, state) in
            itertools::izip!(&self.instance_evaluators, &mut state.instance_states,)
        {
            if let (Some(evaluator), Some(state)) = (evaluator, state.as_mut()) {
                evaluator.eval_finish(state, sim_state.as_deref_mut(), netlist);
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
        let module = netlist.module(self.module_id);
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
        let module = netlist.module(self.module_id);
        assert_eq!(
            state.next_check_query_id,
            self.eval_order.len(),
            "{}",
            module.name
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
        let module = netlist.module(self.module_id);
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

impl PipelineGadgetEvaluator {
    fn new(
        module_id: ModuleId,
        netlist: &Netlist,
        queries: Vec<OutputId>,
        used_ids: &mut EvalInstanceIds,
        instance_path: Vec<String>,
    ) -> Self {
        let nspgi_id = used_ids.new_nspgi();
        //eprintln!("building evaluator for {instance_path:?}, new nspgi_id: {nspgi_id}");
        Self {
            module_id,
            nspgi_id,
            module_evaluator: ModuleEvaluator::new(
                module_id,
                netlist,
                queries,
                used_ids,
                instance_path,
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

        EvaluatorState::PipelineGadget(PipelineGadgetState {
            inputs,
            output_states: LatencyVec::from_vec(vec![None; gadget.max_latency.index() + 1]),
            module_state,
        })
    }
    fn compute_output_state(
        &self,
        state: &mut PipelineGadgetState,
        out_lat: Latency,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) {
        let gadget = netlist.gadget(self.module_id).unwrap();
        let module = netlist.module(self.module_id);
        // TODO: perf: re-use status from previous evaluation, and only update w.r.t. inputs of the
        // current cycle.
        /*
        eprintln!(
            "compute_output_state {} out_lat: {out_lat}, max_in_lat: {}, cur_lat: {}",
            module.name,
            gadget.max_input_latency,
            sim_state.cur_lat()
        );
        */
        state.output_states[out_lat] = Some(PipelineStageStatus::from_inputs(
            self.nspgi_id,
            // This is the "new" execution from out_lat cycles ago, shifted by the definitional
            // offset of GadgetExecCycle.
            (GadgetExecCycle::from_global(sim_state.cur_lat()) + gadget.max_input_latency)
                .checked_sub_lat(out_lat),
            module
                .input_ports
                .iter_enumerated()
                .filter_map(|(input_id, con_id)| {
                    let input_lat = &gadget.latency[*con_id];
                    /*
                    eprintln!(
                        "input_id: {}, input_lat: {}, lat: {}",
                        input_id, input_lat, lat
                    );
                    */
                    let lat_diff = out_lat.checked_sub(*input_lat)?;
                    Some((
                        &gadget.input_roles[input_id],
                        state.inputs[lat_diff][input_id]
                            .as_ref()
                            .expect("uninitialized input"),
                        lat_diff == Latency::from_raw(0),
                    ))
                }),
        ))
    }

    fn out_status<'a>(
        &self,
        state: &'a mut PipelineGadgetState,
        out_lat: Latency,
        sim_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> &'a PipelineStageStatus {
        if state.output_states[out_lat].is_none() {
            self.compute_output_state(state, out_lat, sim_state, netlist);
        }
        state.output_states[out_lat].as_ref().unwrap()
    }
}

impl InstanceEvaluator {
    fn new(
        architecture: &InstanceType,
        queries: Vec<OutputId>,
        netlist: &Netlist,
        used_ids: &mut EvalInstanceIds,
        instance_path: Vec<String>,
    ) -> Option<Self> {
        match architecture {
            InstanceType::Gate(gate) => {
                let inst_id = used_ids.new_inst();
                Some(InstanceEvaluator::Gate(GateEvaluator {
                    gate: *gate,
                    inst_id,
                }))
            }
            InstanceType::Module(submodule_id) => match netlist.gadget(*submodule_id) {
                Some(_gadget) => Some(InstanceEvaluator::Gadget(PipelineGadgetEvaluator::new(
                    *submodule_id,
                    netlist,
                    queries,
                    used_ids,
                    instance_path,
                ))),
                None => Some(InstanceEvaluator::Module(ModuleEvaluator::new(
                    *submodule_id,
                    netlist,
                    queries,
                    used_ids,
                    instance_path,
                ))),
            },
            InstanceType::Input(..) => None,
            InstanceType::Tie(value) => {
                Some(InstanceEvaluator::Tie(TieEvaluator { value: *value }))
            }
            InstanceType::Clock => Some(InstanceEvaluator::Tie(TieEvaluator {
                value: WireValue::_0,
            })),
        }
    }
}

impl EvalInstanceIds {
    pub fn new() -> Self {
        Self {
            nspgi: Range::new(0.into(), 0.into()),
            insts: Range::new(0.into(), 0.into()),
        }
    }
    fn start_new(&self) -> Self {
        Self {
            nspgi: Range::new(self.nspgi.end, self.nspgi.end),
            insts: Range::new(self.insts.end, self.insts.end),
        }
    }
    fn copy_end(&mut self, other: &Self) {
        self.nspgi.end = other.nspgi.end;
        self.insts.end = other.insts.end;
    }
    fn new_nspgi(&mut self) -> NspgiId {
        let res = self.nspgi.end;
        self.nspgi.end += 1;
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
