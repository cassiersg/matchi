use super::gadget::top::{ActiveWireId, ActiveWireVec, LatencyCondition};
use super::gadget::{Latency, PortRole, RndPortVec};
use super::module::{ConnectionId, InputId, InputVec, WireName};
use super::netlist::ModList;
use super::recsim::{
    EvalInstanceIds, Evaluator, EvaluatorState, GlobInstId, ModuleEvaluator, ModuleState, NspgiId,
    NspgiVec,
};
use super::simulation::WireState;
use super::WireValue;
use super::{ModuleId, Netlist};
use crate::share_set::ShareSet;
use crate::type_utils::new_id;
use crate::type_utils::ExtendIdx;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::VecDeque;
use std::fmt::Write;

new_id!(GlobSimCycle, GlobSimCycleVec, GlobSimCycleSlice);

// Execution cycle of a gadget: equal to the GlobSimCycle where inputs at lat 0 are provided.
new_id!(GadgetExecCycle);
impl GadgetExecCycle {
    pub fn from_global(value: GlobSimCycle) -> Self {
        Self::from_usize(value.index())
    }
}

/*
impl From<GlobSimCycle> for i32 {
    fn from(value: GlobSimCycle) -> i32 {
        value.index() as i32
    }
}
*/
impl std::ops::Add<Latency> for GadgetExecCycle {
    type Output = GadgetExecCycle;
    fn add(self, rhs: Latency) -> GadgetExecCycle {
        self + rhs.index()
    }
}
impl GadgetExecCycle {
    pub fn checked_sub_lat(self, lat: Latency) -> Option<Self> {
        self.index().checked_sub(lat.index()).map(Self::from_usize)
    }
}

#[derive(Debug, Clone)]
pub struct Simulator {
    module_id: ModuleId,
    evaluator: ModuleEvaluator,
    vcd_states: super::clk_vcd::VcdParsedStates,
    input_vcd_ids: InputVec<super::clk_vcd::VarOffsetId>,
    active_wire_ids: ActiveWireVec<super::clk_vcd::VarOffsetId>,
}

#[derive(Debug, Clone, Default)]
pub struct RndStatus {
    fresh_uses: Vec<(GlobInstId, GlobSimCycle)>,
    leaks: Vec<(GlobInstId, GlobSimCycle)>,
    last_stored: Option<GlobSimCycle>,
}

/// Conceptually equivalent to LatencyVec<RndStatus>,
/// but is able to throw away outdated elements at the front of the vec.
/// An element is outdated if its last_stored is too old.
/// but behaves as a Deque that auto-prunes
#[derive(Debug, Clone)]
struct RndTracker {
    offset: GlobSimCycle,
    states: VecDeque<RndStatus>,
}

impl RndTracker {
    fn new() -> Self {
        Self {
            offset: GlobSimCycle::from_usize(0),
            states: VecDeque::new(),
        }
    }
    fn index_of(&mut self, lat: GlobSimCycle) -> usize {
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
    fn get_mut(&mut self, lat: GlobSimCycle) -> &mut RndStatus {
        let index = self.index_of(lat);
        &mut self.states[index]
    }
    fn iter_enumerated(&self) -> impl Iterator<Item = (GlobSimCycle, &RndStatus)> + '_ {
        self.states
            .iter()
            .enumerate()
            .map(|(i, rnd_status)| (self.offset + GlobSimCycle::from_usize(i), rnd_status))
    }
    /// Remove states whose last_stored is less than last_cycle
    fn prune(&mut self, last_cycle: GlobSimCycle) {
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
    current_cycle: GlobSimCycle,
    /// Last "valid" execution start cycle.
    last_exec_start: Option<GlobSimCycle>,
    /// Last "pipeline bubble" execution. Sim
    pub last_nonsensitive_exec: NspgiVec<Option<GadgetExecCycle>>,
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
        let mut add_var = |wire_name: &WireName| {
            let mut path = dut_path.to_owned();
            path.push(wire_name.name().to_owned());
            vcd_parsed_header.add_var_offset(&path, wire_name.offset)
        };
        //eprintln!("input ports: {:#?}", netlist.top_gadget.port_roles);
        let input_vcd_ids = module
            .input_ports
            .iter()
            .map(|con_id| add_var(&module.ports[*con_id]))
            .collect::<Result<InputVec<_>>>()
            .with_context(|| "Error while looking up input ports in vcd.")?;
        let active_wire_ids = netlist
            .top_gadget
            .active_wires
            .iter()
            .map(add_var)
            .collect::<Result<ActiveWireVec<_>>>()
            .with_context(|| "Error while looking up 'matchi_active' signals in vcd.")?;
        let vcd_states = vcd_parsed_header.get_states()?;
        Ok(Self {
            module_id,
            evaluator: ModuleEvaluator::new(
                module_id,
                netlist,
                vec![],
                &mut EvalInstanceIds::new(),
                vec![],
                false,
            ),
            vcd_states,
            input_vcd_ids,
            active_wire_ids,
        })
    }
    fn vcd_avar(&self, aw_id: ActiveWireId, cycle: GlobSimCycle) -> bool {
        self.vcd_states
            .get_var_offset(self.active_wire_ids[aw_id], cycle.index())
            == Some(WireValue::_1)
    }
    fn con_valid(
        &self,
        con_id: ConnectionId,
        netlist: &Netlist,
        cycle: GlobSimCycle,
        last_exec_start: Option<GlobSimCycle>,
    ) -> Option<bool> {
        netlist.top_gadget.latency[con_id]
            .as_ref()
            .map(|lat_cond| match lat_cond {
                LatencyCondition::Always => true,
                LatencyCondition::Never => false,
                LatencyCondition::Lats(lats) => (|| -> Option<_> {
                    lats.binary_search(&Latency::from_usize(
                        cycle.checked_sub(last_exec_start?)?.index(),
                    ))
                    .ok()
                })()
                .is_some(),
                LatencyCondition::OnActive(sim_signal) => self.vcd_avar(sim_signal.0, cycle),
            })
    }
    fn gadget_vcd_input(
        &self,
        input_id: InputId,
        netlist: &Netlist,
        cycle: GlobSimCycle,
        last_exec_start: Option<GlobSimCycle>,
    ) -> WireState {
        let module = netlist.module(self.module_id);
        let gadget = &netlist.top_gadget;
        let value: Option<super::WireValue> = self
            .vcd_states
            .get_var_offset(self.input_vcd_ids[input_id], cycle.index());
        let con_id = module.input_ports[input_id];
        let valid = self.con_valid(con_id, netlist, cycle, last_exec_start);
        if valid == Some(true) && value.is_none() {
            println!(
                "Warning: input {} is annotated as valid, but simulation value is 'x'.",
                module.ports[con_id]
            );
        }
        match (&gadget.port_roles[con_id], valid) {
            (PortRole::Share(id), Some(true)) => WireState::share(*id),
            (PortRole::Random(rnd_id), Some(true)) => WireState::random(*rnd_id, cycle),
            (PortRole::Share(_), Some(false))
            | (PortRole::Random(_), Some(false))
            | (PortRole::Control, _) => WireState::control(),
            (PortRole::Share(_), None) | (PortRole::Random(_), None) => {
                unreachable!("No lat for share or random")
            }
        }
        .with_value(value)
        /*
        eprintln!(
            "vcd input, input_id: {}, name: {}, var_offset_id: {:?}, role: {:?}, value: {:?}, valid: {:?}, res: {:?}",
            input_id, module.ports[con_id], self.input_vcd_ids[input_id], gadget.port_roles[con_id], value, valid, res
        );
        */
    }
    pub fn simu<'s>(&'s self, netlist: &'s Netlist, n_cycles: GlobSimCycle) -> SimuIter<'s> {
        let simu_state = self.new_state(self.evaluator.x_state(netlist));
        SimuIter::new(self, simu_state, netlist, n_cycles)
    }
    fn next(
        &self,
        prev_state: &SimulationState,
        glob_state: &mut GlobSimulationState,
        netlist: &Netlist,
    ) -> Result<SimulationState> {
        let module = netlist.module(self.module_id);
        let cycle = glob_state.current_cycle;
        if let Some(ea) = netlist.top_gadget.exec_active {
            let exec_active = self.vcd_avar(ea, cycle);
            let past_exec_active = cycle > 0 && self.vcd_avar(ea, cycle - 1);
            if exec_active && !past_exec_active {
                glob_state.last_exec_start = Some(cycle);
            }
        }
        let mut eval_state = self.evaluator.init_next(&prev_state.eval_state, netlist);
        for input_id in module.input_ports.indices() {
            let input_state =
                self.gadget_vcd_input(input_id, netlist, cycle, glob_state.last_exec_start);
            self.evaluator
                .set_input(&mut eval_state, input_id, input_state.clone(), netlist);
        }
        self.evaluator
            .eval_finish(&mut eval_state, Some(glob_state), netlist);
        //self.evaluator.debug_state(&eval_state, netlist);
        Ok(self.new_state(eval_state))
    }
    fn new_glob_state(&self, netlist: &Netlist) -> GlobSimulationState {
        let gadget = &netlist.top_gadget;
        GlobSimulationState {
            random_status: RndPortVec::from_vec(vec![RndTracker::new(); gadget.rnd_ports.len()]),
            current_cycle: GlobSimCycle::from_raw(0),
            last_exec_start: None,
            last_nonsensitive_exec: NspgiVec::new(),
        }
    }
    fn new_state(&self, eval_state: EvaluatorState) -> SimulationState {
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
            eprintln!(
                "use random port: {:?}, lat: {:?}, cur_lat: {:?}",
                rnd_source.port,
                rnd_source.lat,
                self.cur_lat()
            );
            let cur_lat = self.cur_lat();
            self.random_status[rnd_source.port]
                .get_mut(rnd_source.lat)
                .fresh_use(inst, cur_lat - cycle_offset.index());
        }
    }
    pub fn store_random(&mut self, wire: &WireState) {
        if let Some(rnd_source) = wire.random.as_ref() {
            /*
            eprintln!(
                "store random port: {:?}, lat: {:?}, cur_lat: {:?}",
                rnd_source.port,
                rnd_source.lat,
                self.cur_lat()
            );
            */
            self.random_status[rnd_source.port]
                .get_mut(rnd_source.lat)
                .last_stored = Some(self.cur_lat());
        }
    }
    pub fn cur_lat(&self) -> GlobSimCycle {
        self.current_cycle
    }
    pub fn nspgi_det_exec(&mut self, nspgi_id: NspgiId, exec_cycle: GadgetExecCycle) {
        self.last_nonsensitive_exec.extend_idx(nspgi_id, None);
        self.last_nonsensitive_exec[nspgi_id] = Some(exec_cycle);
    }
}

#[derive(Debug, Clone)]
pub struct SimuIter<'a> {
    simu_state: SimulationState,
    glob_state: Option<GlobSimulationState>,
    netlist: &'a Netlist,
    simulator: &'a Simulator,
    n_cycles: GlobSimCycle,
}

impl<'a> SimuIter<'a> {
    fn new(
        simulator: &'a Simulator,
        simu_state: SimulationState,
        netlist: &'a Netlist,
        n_cycles: GlobSimCycle,
    ) -> Self {
        Self {
            simu_state,
            glob_state: None,
            netlist,
            simulator,
            n_cycles,
        }
    }
    pub fn next(mut self) -> Result<Option<Self>> {
        let glob_state = if let Some(mut glob_state) = self.glob_state.take() {
            for rnd_tracker in glob_state.random_status.iter_mut() {
                rnd_tracker.prune(glob_state.current_cycle);
            }
            glob_state.current_cycle += 1;
            glob_state
        } else {
            self.simulator.new_glob_state(self.netlist)
        };
        let glob_state = self.glob_state.insert(glob_state);
        if glob_state.current_cycle == self.n_cycles {
            return Ok(None);
        }
        self.simu_state = self
            .simulator
            .next(&self.simu_state, glob_state, self.netlist)?;
        Ok(Some(self))
    }
    pub fn state(&self) -> &SimulationState {
        &self.simu_state
    }
    pub fn check(&mut self) -> Result<()> {
        if let Some(glob_state) = self.glob_state.as_mut() {
            self.simulator.evaluator.check_safe_finish(
                &mut self.simu_state.eval_state,
                glob_state,
                self.netlist,
            )?;
            self.check_random_uses()?;
            self.check_output_ports()?;
        }
        Ok(())
    }
    fn check_random_uses(&self) -> Result<()> {
        let module = self.netlist.module(self.simulator.module_id);
        let gadget = &self.netlist.top_gadget;
        let glob_state = self.glob_state.as_ref().unwrap();
        for (rnd_port_id, rnd_uses) in glob_state.random_status.iter_enumerated() {
            for (lat, status) in rnd_uses.iter_enumerated() {
                if !status.fresh_uses.is_empty() && status.leaks.len() > 1 {
                    let wire_name =
                        &module.ports[module.input_ports[gadget.rnd_ports[rnd_port_id]]];
                    let mut use_string = "\n\tAs fresh randomness in:".to_owned();
                    let write_uses = |x: &Vec<(GlobInstId, GlobSimCycle)>, s: &mut String| {
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
    fn check_output_ports(&self) -> Result<()> {
        let module = self.netlist.module(self.simulator.module_id);
        let gadget = &self.netlist.top_gadget;
        let glob_state = self.glob_state.as_ref().unwrap();
        for con_id in module.output_ports.iter() {
            let valid = self.simulator.con_valid(
                *con_id,
                self.netlist,
                glob_state.current_cycle,
                glob_state.last_exec_start,
            );
            let wire_state = self.simu_state.eval_state.module().wire_states
                [module.connection_wires[*con_id]]
                .as_ref()
                .unwrap();
            match (&gadget.port_roles[*con_id], valid) {
                (PortRole::Share(id), Some(true)) => {
                    // FIXME: check glitch-sensitivity of outputs.
                    //if !wire_state.glitch_sensitivity.subset_of(ShareSet::from(*id)) {
                    if !wire_state.sensitivity.subset_of(ShareSet::from(*id)) {
                        bail!(
                            "Output share {} is (glitch-)sensitive for shares {}.",
                            module.ports[*con_id],
                            //wire_state.glitch_sensitivity
                            wire_state.sensitivity
                        );
                    } else if wire_state.sensitivity != ShareSet::from(*id) {
                        println!(
                            "Warning: output port {} is not sensitive, while marked as such.",
                            module.ports[*con_id]
                        )
                    }
                }
                (PortRole::Share(_), Some(false)) => {
                    //if !wire_state.glitch_sensitivity.is_empty() {
                    if !wire_state.sensitivity.is_empty() {
                        bail!(
                            "Output share {} is not at a valid latency, but it is (glitch-)sensitive for shares {}.",
                            module.ports[*con_id],
                            //wire_state.glitch_sensitivity
                            wire_state.sensitivity
                        );
                    }
                }
                (PortRole::Control, _) => {
                    if !wire_state.glitch_sensitivity.is_empty() {
                        bail!(
                            "Output {} is a control, but it is (glitch-)sensitive for shares {}.",
                            module.ports[*con_id],
                            wire_state.glitch_sensitivity
                        );
                    } else if !wire_state.deterministic {
                        bail!(
                            "Output {} is a control, but it is not deterministic.",
                            module.ports[*con_id],
                        );
                    }
                }
                (PortRole::Share(_), None) | (PortRole::Random(_), _) => {
                    unreachable!()
                }
            }
        }
        Ok(())
    }
}

impl RndStatus {
    fn leak(&mut self, inst: GlobInstId, lat: GlobSimCycle) {
        self.leaks.push((inst, lat));
    }
    fn fresh_use(&mut self, inst: GlobInstId, lat: GlobSimCycle) {
        self.fresh_uses.push((inst, lat));
    }
}
