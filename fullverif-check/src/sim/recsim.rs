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
use super::module::{ConnectionId, InputId, InstanceType, InstanceVec, WireId, WireVec};
use super::netlist::Netlist;
use super::simulation::WireState;
use super::ModuleId;
use anyhow::{anyhow, bail, Error, Result};

pub struct InstanceEvaluator {
    module_id: ModuleId,
    instance_evaluators: InstanceVec<Option<InstanceEvaluator>>,
    // The Option<ConnectionId> is only there for an assert.
    eval_order: Vec<(ConnectionId, Vec<WireId>)>,
    eval_finish: Vec<WireId>,
}

pub struct InstanceEvaluatorState {
    pub instance_states: InstanceVec<Option<InstanceEvaluatorState>>,
    pub wire_states: WireVec<Option<WireState>>, // None for "uninit", allows to check correct evaluation order.
    next_eval_query_id: usize,                   // index in eval_order
}

impl InstanceEvaluator {
    pub fn new(module_id: ModuleId, netlist: &Netlist, queries: Vec<ConnectionId>) -> Self {
        let module = &netlist[module_id];
        let wg = &module.comb_wire_dag;
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
        for output_con in queries {
            let output_wire = module.connection_wires[output_con];
            let eval_wires = eval_dfs(output_wire);
            eval_order.push((output_con, eval_wires));
        }
        let eval_finish = module
            .wires
            .indices()
            .flat_map(|eval_wire_id| eval_dfs(eval_wire_id).into_iter())
            .collect();
        let instance_evaluators = module
            .instances
            .iter()
            .zip(instance_queries)
            .map(|(instance, queries)| {
                if let InstanceType::Module(submodule_id) = &instance.architecture {
                    Some(InstanceEvaluator::new(*submodule_id, netlist, queries))
                } else {
                    None
                }
            })
            .collect::<InstanceVec<_>>();
        Self {
            module_id,
            instance_evaluators,
            eval_order,
            eval_finish,
        }
    }
    fn uninit_state(&self, netlist: &Netlist) -> InstanceEvaluatorState {
        let instance_states = self
            .instance_evaluators
            .iter()
            .map(|eval| eval.as_ref().map(|eval| eval.uninit_state(netlist)))
            .collect();
        let wire_states = WireVec::from_vec(vec![None; netlist[self.module_id].wires.len()]);
        let next_eval_query_id = 0;
        InstanceEvaluatorState {
            instance_states,
            wire_states,
            next_eval_query_id,
        }
    }
    fn x_state(&self, netlist: &Netlist) -> InstanceEvaluatorState {
        let instance_states = self
            .instance_evaluators
            .iter()
            .map(|eval| eval.as_ref().map(|eval| eval.x_state(netlist)))
            .collect();
        let wire_states = WireVec::from_vec(vec![
            Some(WireState::from_x());
            netlist[self.module_id].wires.len()
        ]);
        let next_eval_query_id = self.eval_order.len() + 1;
        InstanceEvaluatorState {
            instance_states,
            wire_states,
            next_eval_query_id,
        }
    }
    fn state_from_vcd(
        &self,
        vcd: &mut super::clk_vcd::ModuleControls,
        netlist: &Netlist,
    ) -> InstanceEvaluatorState {
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
                Some(if let Some((name, offset)) = opt_name {
                    vcd.lookup(vec![name.to_owned()], 0, *offset)
                        .unwrap()
                        .unwrap()
                        .to_bool()
                        .map(|v| WireState::constant(v.into()))
                        .unwrap_or(WireState::from_x())
                } else {
                    WireState::from_x()
                })
            })
            .collect();
        let next_eval_query_id = self.eval_order.len() + 1;
        InstanceEvaluatorState {
            instance_states,
            wire_states,
            next_eval_query_id,
        }
    }
    fn eval_input(
        &self,
        state: &mut InstanceEvaluatorState,
        input: InputId,
        input_state: WireState,
        netlist: &Netlist,
    ) {
        let module = &netlist[self.module_id];
        let con_id = module.input_ports[input];
        let wire_id = module.connection_wires[con_id];
        /*
        eprintln!(
            "eval_input {} {:?} in module {}",
            wire_id, module.ports[con_id], module.name,
        );
        */
        assert!(state.wire_states[wire_id].is_none());
        state.wire_states[wire_id] = Some(input_state);
        self.eval_fanout(wire_id, state, netlist);
    }
    fn eval_out(
        &self,
        out: ConnectionId,
        state: &mut InstanceEvaluatorState,
        prev_state: &InstanceEvaluatorState,
        netlist: &Netlist,
    ) -> WireState {
        let module = &netlist[self.module_id];
        let (next_con_id, next_wires) = &self.eval_order[state.next_eval_query_id];
        assert_eq!(out, *next_con_id);
        self.eval_wires(next_wires.as_slice(), state, prev_state, netlist);
        state.next_eval_query_id += 1;
        state.wire_states[module.connection_wires[out]].unwrap()
    }
    fn eval_finish(
        &self,
        state: &mut InstanceEvaluatorState,
        prev_state: &InstanceEvaluatorState,
        netlist: &Netlist,
    ) {
        assert_eq!(state.next_eval_query_id, self.eval_order.len());
        self.eval_wires(self.eval_finish.as_slice(), state, prev_state, netlist);
        state.next_eval_query_id += 1;
        for (evaluator, state, prev_state) in itertools::izip!(
            &self.instance_evaluators,
            &mut state.instance_states,
            &prev_state.instance_states,
        ) {
            if let (Some(evaluator), Some(state), Some(prev_state)) =
                (evaluator, state.as_mut(), prev_state)
            {
                evaluator.eval_finish(state, prev_state, netlist);
            } else {
                assert!(evaluator.is_none() && state.is_none() && prev_state.is_none());
            }
        }
    }
    fn eval_wires(
        &self,
        wires: &[WireId],
        state: &mut InstanceEvaluatorState,
        prev_state: &InstanceEvaluatorState,
        netlist: &Netlist,
    ) {
        let module = &netlist[self.module_id];
        for wire in wires {
            /*
            eprintln!(
                "eval wire {:?} ({:?}) in module {}",
                wire, module.wire_names[*wire], module.name
            );
            */
            let (src_inst_id, src_con) = module.wires[*wire].source;
            let instance = &module.instances[src_inst_id];
            match instance.architecture {
                InstanceType::Gate(gate) => {
                    //eprintln!("\tis gate {:?}", gate);
                    let operands = instance.connections.iter().map(|op_wire_id| {
                        (
                            prev_state.wire_states[*op_wire_id],
                            state.wire_states[*op_wire_id],
                        )
                    });
                    state.wire_states[*wire] = Some(gate.eval(operands));
                }
                InstanceType::Module(_) => {
                    /*
                    eprintln!(
                        "\tis from instance {} (con {:?})",
                        module.instances[src_inst_id].name, src_con
                    );
                    */
                    let res = self.instance_evaluators[src_inst_id]
                        .as_ref()
                        .unwrap()
                        .eval_out(
                            src_con,
                            state.instance_states[src_inst_id].as_mut().unwrap(),
                            prev_state.instance_states[src_inst_id].as_ref().unwrap(),
                            netlist,
                        );
                    state.wire_states[*wire] = Some(res);
                }
                InstanceType::Input(input_id, _) => {
                    //eprintln!("\tis input {:?}", input_id);
                    assert!(state.wire_states
                        [module.connection_wires[module.input_ports[input_id]]]
                        .is_some());
                    unreachable!("No need to eval inputs");
                }
                InstanceType::Tie(value) => {
                    //eprintln!("\tis TIE {:?}", value);
                    state.wire_states[*wire] = Some(WireState::constant(value));
                }
            }
            self.eval_fanout(*wire, state, netlist);
            //eprintln!("eval wire done");
        }
    }
    fn eval_fanout(&self, wire: WireId, state: &mut InstanceEvaluatorState, netlist: &Netlist) {
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
                sub_evaluator.eval_input(
                    state.instance_states[*instance_id].as_mut().unwrap(),
                    *input_id,
                    state.wire_states[wire].unwrap(),
                    netlist,
                );
            }
        }
        //eprintln!("eval fanout done");
    }
    fn inputs_vcd(
        &self,
        state: &mut InstanceEvaluatorState,
        vcd: &mut super::clk_vcd::ModuleControls,
        time_offset: usize,
        netlist: &Netlist,
    ) -> Result<()> {
        let module = &netlist[self.module_id];
        for (input_id, con_id) in module.input_ports.iter_enumerated() {
            let (port_name, offset) = &module.ports[*con_id];
            let input_state = vcd
                .lookup(vec![port_name.to_owned()], time_offset, *offset)
                .unwrap()
                .expect(&format!(
                    "No value for {:?}, wire {}",
                    vcd.root_module(),
                    port_name
                ))
                .to_bool()
                .map(|v| WireState::constant(v.into()))
                .unwrap_or(WireState::from_x());
            self.eval_input(state, input_id, input_state, netlist);
        }
        Ok(())
    }
    fn next_vcd(
        &self,
        prev_state: &InstanceEvaluatorState,
        vcd: &mut super::clk_vcd::ModuleControls,
        time_offset: usize,
        netlist: &Netlist,
    ) -> Result<InstanceEvaluatorState> {
        let mut res = self.uninit_state(netlist);
        self.inputs_vcd(&mut res, vcd, time_offset, netlist)?;
        self.eval_finish(&mut res, prev_state, netlist);
        Ok(res)
    }
    pub fn simu<'s>(
        &'s self,
        mut vcd: super::clk_vcd::ModuleControls<'s>,
        netlist: &'s Netlist,
        init_from_vcd: bool,
    ) -> impl Iterator<Item = Result<InstanceEvaluatorState>> + 's {
        let mut current = if init_from_vcd {
            Some(Ok(self.state_from_vcd(&mut vcd, netlist)))
        } else {
            Some(Ok(self.x_state(netlist)))
        };
        let mut time_offset = 0;
        std::iter::from_fn(move || {
            let prev = match current.take().unwrap() {
                Ok(prev) => prev,
                Err(e) => return Some(Err(e)),
            };
            time_offset += 1;
            current = Some(self.next_vcd(&prev, &mut vcd, time_offset, netlist));
            Some(Ok(prev))
        })
    }
}
