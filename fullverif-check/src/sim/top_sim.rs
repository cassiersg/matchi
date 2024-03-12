use super::gadget::{Latency, PortRole, RndPortVec, Slatency};
use super::module::{InputId, InputVec};
use super::netlist::ModList;
use super::recsim::{
    EvalInstanceIds, Evaluator, EvaluatorState, GlobInstId, ModuleEvaluator, ModuleState, NspgiId,
    NspgiVec,
};
use super::simulation::WireState;
use super::{ModuleId, Netlist};
use crate::type_utils::ExtendIdx;
use anyhow::{anyhow, bail, Result};
use itertools::izip;
use std::collections::VecDeque;
use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct Simulator {
    module_id: ModuleId,
    evaluator: ModuleEvaluator,
    vcd_states: super::clk_vcd::VcdParsedStates,
    input_vcd_ids: InputVec<super::clk_vcd::VarId>,
}

#[derive(Debug, Clone, Default)]
pub struct RndStatus {
    fresh_uses: Vec<(GlobInstId, Latency)>,
    leaks: Vec<(GlobInstId, Latency)>,
    last_stored: Option<Latency>,
}

/// Conceptually equivalent to LatencyVec<RndStatus>,
/// but is able to throw away outdated elements at the front of the vec.
/// An element is outdated if its last_stored is too old.
/// but behaves as a Deque that auto-prunes
#[derive(Debug, Clone)]
struct RndTracker {
    offset: Latency,
    states: VecDeque<RndStatus>,
}

impl RndTracker {
    fn new() -> Self {
        Self {
            offset: Latency::from_usize(0),
            states: VecDeque::new(),
        }
    }
    fn index_of(&mut self, lat: Latency) -> usize {
        assert!(
            lat >= self.offset,
            "lat: {}, self.offset: {}",
            lat,
            self.offset
        );
        let index = (lat - self.offset).index();
        while index >= self.states.len() {
            self.states.push_back(RndStatus::default());
        }
        index
    }
    fn get_mut(&mut self, lat: Latency) -> &mut RndStatus {
        let index = self.index_of(lat);
        &mut self.states[index]
    }
    fn iter_enumerated(&self) -> impl Iterator<Item = (Latency, &RndStatus)> + '_ {
        self.states
            .iter()
            .enumerate()
            .map(|(i, rnd_status)| (self.offset + Latency::from_usize(i), rnd_status))
    }
    /// Remove states whose last_stored is less than last_cycle
    fn prune(&mut self, last_cycle: Latency) {
        while self
            .states
            .front()
            .is_some_and(|state| !state.last_stored.is_some_and(|last| last >= last_cycle))
        {
            self.offset += 1;
            self.states.pop_front();
        }
        if self.states.is_empty() {
            self.offset = last_cycle;
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlobSimulationState {
    /// For every top-level random port and every input latency, track where the corresponding
    /// random value is leaked, used and stored.
    random_status: RndPortVec<RndTracker>,
    /// Current simulated clock cycle.
    current_cycle: Option<Latency>,
    /// Last deterministic execution "pipeline bubble".
    // FIXME: how is this working, when we actually do not consider a bubble if randomness is
    // fresh? (implied by the "fully deterministic" requirement)
    pub last_det_exec: NspgiVec<Slatency>,
}

#[derive(Debug, Clone)]
pub struct SimulationState {
    eval_state: EvaluatorState,
}

impl Simulator {
    pub fn new(
        netlist: &Netlist,
        vcd_parser: vcd::Parser<impl std::io::BufRead>,
        dut_path: &[String],
    ) -> Result<Self> {
        let module_id = netlist.top_gadget.module_id;
        let module = netlist.module(module_id);
        let mut clk_path = dut_path.to_owned();
        clk_path.push(
            module
                .clock
                .as_ref()
                .ok_or_else(|| anyhow!("Top-level gadget must have a clock."))?
                .name()
                .to_owned(),
        );
        let mut vcd_parsed_header = super::clk_vcd::VcdParsedHeader::new(vcd_parser, &clk_path)?;
        let input_vcd_ids = module
            .input_ports
            .iter()
            .map(|con_id| {
                let mut path = dut_path.to_owned();
                path.push(module.ports[*con_id].name().to_owned());
                vcd_parsed_header.add_var(&path)
            })
            .collect::<Result<InputVec<_>>>()?;
        let vcd_states = vcd_parsed_header.get_states()?;
        Ok(Self {
            module_id,
            evaluator: ModuleEvaluator::new(
                module_id,
                netlist,
                vec![],
                &mut EvalInstanceIds::new(),
            ),
            vcd_states,
            input_vcd_ids,
        })
    }
    fn gadget_vcd_input_sequence<'a>(
        &'a self,
        netlist: &'a Netlist,
        n_cycles: usize,
    ) -> impl Iterator<Item = InputVec<WireState>> + 'a {
        let module = netlist.module(self.module_id);
        (0..n_cycles).map(move |t| {
            module
                .input_ports
                .indices()
                .map(|input_id| self.gadget_vcd_input(input_id, netlist, t))
                .collect()
        })
    }
    fn gadget_vcd_input(&self, input_id: InputId, netlist: &Netlist, cycle: usize) -> WireState {
        let gadget = &netlist.top_gadget;
        let value: Option<super::WireValue> = self
            .vcd_states
            .get_var(self.input_vcd_ids[input_id], cycle)
            .to_bool()
            .map(|v| v.into());
        match &gadget.input_roles[input_id] {
            PortRole::Share(id) => WireState::share(*id),
            // FIXME: randomness freshness/validity
            PortRole::Random(rnd_id) => WireState::random(*rnd_id, Latency::from_usize(cycle)),
            //.valid(time_offset >= start_exec_offset),
            PortRole::Control => WireState::control(),
        }
        .with_value(value)
    }
    pub fn simu<'s>(
        &'s self,
        netlist: &'s Netlist,
        n_cycles: usize,
    ) -> SimuIter<'s, impl Iterator<Item = InputVec<WireState>> + 's> {
        let simu_state = self.new_state(self.evaluator.x_state(netlist), netlist);
        let inputs = self.gadget_vcd_input_sequence(netlist, n_cycles);
        SimuIter::new(self, inputs, simu_state, netlist)
    }
    fn next(
        &self,
        prev_state: &SimulationState,
        glob_state: &mut GlobSimulationState,
        inputs: &InputVec<WireState>,
        netlist: &Netlist,
    ) -> Result<SimulationState> {
        if let Some(cur_lat) = glob_state.current_cycle {
            for rnd_tracker in glob_state.random_status.iter_mut() {
                rnd_tracker.prune(cur_lat);
            }
            glob_state.current_cycle = Some(cur_lat + 1);
        } else {
            glob_state.current_cycle = Some(Latency::from_raw(0));
        }

        let mut eval_state = self.evaluator.init_next(&prev_state.eval_state, netlist);
        for (input_id, input_state) in inputs.iter_enumerated() {
            self.evaluator
                .set_input(&mut eval_state, input_id, input_state.clone(), netlist);
        }
        self.evaluator
            .eval_finish(&mut eval_state, Some(glob_state), netlist);
        Ok(self.new_state(eval_state, netlist))
    }
    fn new_glob_state(&self, netlist: &Netlist) -> GlobSimulationState {
        let gadget = &netlist.top_gadget;
        GlobSimulationState {
            random_status: RndPortVec::from_vec(vec![RndTracker::new(); gadget.rnd_ports.len()]),
            current_cycle: None,
            last_det_exec: NspgiVec::new(),
        }
    }
    fn new_state(&self, eval_state: EvaluatorState, netlist: &Netlist) -> SimulationState {
        SimulationState { eval_state }
    }
    pub fn n_cycles(&self) -> usize {
        self.vcd_states.len()
    }
}

impl SimulationState {
    pub fn module(&self) -> &ModuleState {
        self.eval_state.module()
    }
}

impl GlobSimulationState {
    pub fn leak_random(&mut self, wire: &WireState, inst: GlobInstId) {
        if let Some(rnd_source) = wire.random.as_ref() {
            let cur_lat = self.cur_lat();
            self.random_status[rnd_source.port]
                .get_mut(rnd_source.lat)
                .leak(inst, cur_lat);
        }
    }
    pub fn use_random(&mut self, wire: &WireState, inst: GlobInstId, cycle_offset: Latency) {
        if let Some(rnd_source) = wire.random.as_ref() {
            let cur_lat = self.cur_lat();
            self.random_status[rnd_source.port]
                .get_mut(rnd_source.lat)
                .fresh_use(inst, cur_lat - cycle_offset);
        }
    }
    pub fn store_random(&mut self, wire: &WireState) {
        if let Some(rnd_source) = wire.random.as_ref() {
            self.random_status[rnd_source.port]
                .get_mut(rnd_source.lat)
                .last_stored = Some(self.cur_lat());
        }
    }
    pub fn cur_lat(&self) -> Latency {
        self.current_cycle.unwrap()
    }
    pub fn nspgi_det_exec(&mut self, nspgi_id: NspgiId) {
        self.last_det_exec.extend_idx(nspgi_id, Slatency::MIN);
        self.last_det_exec[nspgi_id] = self.cur_lat().into();
    }
}

#[derive(Debug, Clone)]
pub struct SimuIter<'a, I> {
    simu_state: SimulationState,
    glob_state: GlobSimulationState,
    inputs: I,
    netlist: &'a Netlist,
    simulator: &'a Simulator,
}

impl<'a, I> SimuIter<'a, I>
where
    I: Iterator<Item = InputVec<WireState>> + 'a,
{
    fn new(
        simulator: &'a Simulator,
        inputs: I,
        simu_state: SimulationState,
        netlist: &'a Netlist,
    ) -> Self {
        let glob_state = simulator.new_glob_state(netlist);
        Self {
            simu_state,
            glob_state,
            inputs,
            netlist,
            simulator,
        }
    }
    pub fn next(mut self) -> Result<Option<Self>> {
        let Some(inputs) = self.inputs.next() else {
            return Ok(None);
        };
        self.simu_state = self.simulator.next(
            &self.simu_state,
            &mut self.glob_state,
            &inputs,
            self.netlist,
        )?;
        Ok(Some(self))
    }
    pub fn state(&self) -> &SimulationState {
        &self.simu_state
    }
    pub fn check(&mut self) -> Result<()> {
        self.simulator.evaluator.check_safe_finish(
            &mut self.simu_state.eval_state,
            Some(&mut self.glob_state),
            self.netlist,
        )?;
        self.check_random_uses()?;
        Ok(())
    }
    fn check_random_uses(&self) -> Result<()> {
        let module = self.netlist.module(self.simulator.module_id);
        let gadget = &self.netlist.top_gadget;
        for (rnd_port_id, rnd_uses) in self.glob_state.random_status.iter_enumerated() {
            for (lat, status) in rnd_uses.iter_enumerated() {
                if !status.fresh_uses.is_empty() && status.leaks.len() > 1 {
                    let wire_name =
                        &module.ports[module.input_ports[gadget.rnd_ports[rnd_port_id]]];
                    let mut use_string = "\n\tAs fresh randomness in:".to_owned();
                    let write_uses = |x: &Vec<(GlobInstId, Latency)>, s: &mut String| {
                        for (inst_id, inst_lat) in x {
                            write!(
                                s,
                                "\n\t\t{} (at cycle {})",
                                self.simulator
                                    .evaluator
                                    .glob_inst2path(*inst_id, self.netlist)
                                    .unwrap(),
                                inst_lat
                            )
                            .unwrap();
                        }
                    };
                    write_uses(&status.fresh_uses, &mut use_string);
                    if !status.leaks.is_empty() {
                        write!(use_string, "\n\tOther in:").unwrap();
                        write_uses(&status.leaks, &mut use_string);
                    }
                    bail!(
                        "Random input {} at cycle {} is used in multiple places:{}.",
                        wire_name,
                        lat,
                        use_string,
                    );
                }
            }
        }
        Ok(())
    }
}

impl RndStatus {
    fn leak(&mut self, inst: GlobInstId, lat: Latency) {
        self.leaks.push((inst, lat));
    }
    fn fresh_use(&mut self, inst: GlobInstId, lat: Latency) {
        self.fresh_uses.push((inst, lat));
    }
}
