use super::gadget::{InputRole, Latency, LatencyVec, RndPortVec, Slatency};
use super::module::{
    ConnectionId, InputId, InputVec, InstanceType, InstanceVec, OutputId, WireId, WireVec,
};
use super::recsim::{
    EvalInstanceIds, Evaluator, EvaluatorState, GlobInstId, ModuleEvaluator, ModuleState, NspgiId,
    NspgiVec,
};
use super::simulation::{RandomSource, WireState};
use super::{ModuleId, Netlist};
use crate::type_utils::ExtendIdx;
use anyhow::{anyhow, bail, Context, Error, Result};
use itertools::izip;
use std::collections::VecDeque;
use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct Simulator {
    module_id: ModuleId,
    evaluator: ModuleEvaluator,
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
    fn get(&mut self, lat: Latency) -> &RndStatus {
        let index = self.index_of(lat);
        &self.states[index]
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
    fn prune(&mut self, up_to: Latency) {
        while self.offset < up_to && !self.states.is_empty() {
            self.offset += 1;
            self.states.pop_front();
        }
        if self.states.is_empty() {
            self.offset = up_to;
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlobSimulationState {
    // FIXME: instead of LatencyVec, uses something with state pruning.
    //random_status: RndPortVec<LatencyVec<RndStatus>>,
    random_status: RndPortVec<RndTracker>,
    current_lat: Option<Latency>,
    pub last_det_exec: NspgiVec<Slatency>,
}

#[derive(Debug, Clone)]
pub struct SimulationState {
    eval_state: EvaluatorState,
}

impl Simulator {
    pub fn new(module_id: ModuleId, netlist: &Netlist) -> Self {
        Self {
            module_id,
            evaluator: ModuleEvaluator::new(
                module_id,
                netlist,
                vec![],
                &mut EvalInstanceIds::new(),
            ),
        }
    }
    pub fn gadget_vcd_inputs(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        time_offset: usize,
        start_exec_offset: usize,
        netlist: &Netlist,
    ) -> Result<InputVec<WireState>> {
        let module = &netlist[self.module_id];
        let gadget = netlist.gadget(self.module_id).unwrap();
        Ok(izip!(&module.input_ports, &gadget.input_roles,)
            .map(|(con_id, input_role)| {
                let wire_name = &module.ports[*con_id];
                let value: Option<super::WireValue> = vcd
                    .lookup(vec![wire_name.name.clone()], time_offset, wire_name.offset)
                    .unwrap()
                    .unwrap_or_else(|| {
                        panic!("No value for {:?}, wire {}", vcd.root_module(), wire_name)
                    })
                    .to_bool()
                    .map(|v| v.into());
                let valid = time_offset >= start_exec_offset
                    && gadget.valid_latencies[*con_id]
                        .binary_search(&Latency::from_raw((time_offset - start_exec_offset) as u32))
                        .is_ok();
                /*
                eprintln!(
                    "gadget_vcd_inputs, time_offset: {}, start_exec_offset: {}, input {:?}, latencies: {:?}, valid: {}",
                time_offset, start_exec_offset,
                    module.ports[*con_id],
                gadget.valid_latencies[*con_id], valid
                );
                        */
                match input_role {
                    InputRole::Share(id) => WireState::share(*id),
                    // FIXME: randomness freshness/validity
                    InputRole::Random(rnd_id) => {
                        WireState::random(*rnd_id, Latency::from_usize(time_offset))
                    }
                    //.valid(time_offset >= start_exec_offset),
                    InputRole::Control => WireState::control(),
                }
                .with_value(value)
            })
            .collect())
    }
    fn next(
        &self,
        prev_state: &SimulationState,
        glob_state: &mut GlobSimulationState,
        inputs: &InputVec<WireState>,
        netlist: &Netlist,
    ) -> Result<SimulationState> {
        if let Some(cur_lat) = glob_state.current_lat {
            for rnd_tracker in glob_state.random_status.iter_mut() {
                rnd_tracker.prune(cur_lat);
            }
            glob_state.current_lat = Some(cur_lat + 1);
        } else {
            glob_state.current_lat = Some(Latency::from_raw(0));
        }

        let mut eval_state = self.evaluator.init_next(&prev_state.eval_state, netlist);
        for (input_id, input_state) in inputs.iter_enumerated() {
            if input_state.random.is_some() {
                let module = &netlist[self.module_id];
                /*
                eprintln!(
                    "{:?}: {:?}",
                    module.ports[module.input_ports[input_id]], input_state
                );
                */
            }
            self.evaluator
                .set_input(&mut eval_state, input_id, input_state.clone(), netlist);
        }
        self.evaluator
            .eval_finish(&mut eval_state, glob_state, netlist);
        Ok(self.new_state(eval_state, netlist))
    }
    pub fn simu<'s, I>(
        &'s self,
        inputs: I,
        netlist: &'s Netlist,
        init_from_vcd: Option<super::clk_vcd::ModuleControls>,
    ) -> SimuIter<'s, I>
    where
        I: Iterator<Item = Result<InputVec<WireState>>> + 's,
    {
        let simu_state = if let Some(mut vcd) = init_from_vcd {
            self.new_state(self.evaluator.state_from_vcd(&mut vcd, netlist), netlist)
        } else {
            self.new_state(self.evaluator.x_state(netlist), netlist)
        };
        SimuIter::new(self, inputs, simu_state, netlist)
    }
    fn new_glob_state(&self, netlist: &Netlist) -> GlobSimulationState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        GlobSimulationState {
            random_status: RndPortVec::from_vec(vec![RndTracker::new(); gadget.rnd_ports.len()]),
            current_lat: None,
            last_det_exec: NspgiVec::new(),
        }
    }
    fn new_state(&self, eval_state: EvaluatorState, netlist: &Netlist) -> SimulationState {
        let gadget = netlist.gadget(self.module_id).unwrap();
        SimulationState { eval_state }
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
        self.current_lat.unwrap()
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
    I: Iterator<Item = Result<InputVec<WireState>>> + 'a,
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
            &inputs?,
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
            &mut self.glob_state,
            self.netlist,
        )?;
        self.check_random_uses()?;
        Ok(())
    }
    fn check_random_uses(&self) -> Result<()> {
        let module = &self.netlist[self.simulator.module_id];
        let gadget = self.netlist.gadget(self.simulator.module_id).unwrap();
        for (rnd_port_id, rnd_uses) in self.glob_state.random_status.iter_enumerated() {
            for (lat, status) in rnd_uses.iter_enumerated() {
                if !status.fresh_uses.is_empty() && status.leaks.len() > 1 {
                    let wire_name =
                        &module.ports[module.input_ports[gadget.rnd_ports[rnd_port_id]]];
                    let mut use_string = format!("As fresh randomness in:");
                    let write_uses = |x: &Vec<(GlobInstId, Latency)>, s: &mut String| {
                        for (inst_id, inst_lat) in x {
                            write!(
                                s,
                                "\n\t{} (at cycle {})",
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
                        "Random input {} at cycle {} is used in multiple places: {}.",
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
