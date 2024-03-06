use super::fv_cells::Gate;
use super::gadget::{Latency, LatencyVec, StaticGadget};
use super::netlist::Netlist;
use super::ModuleId;
use crate::type_utils::new_id;
use anyhow::{anyhow, bail, Context, Result};
use fnv::FnvHashMap as HashMap;
use index_vec::Idx;
use std::collections::BTreeSet;
use yosys_netlist_json as yosys;

new_id!(InstanceId, InstanceVec, InstanceSlice);
new_id!(WireId, WireVec, WireSlice);
new_id!(ConnectionId, ConnectionVec, ConnectionSlice);
new_id!(InputId, InputVec, InputSlice);
new_id!(OutputId, OutputVec, OutputSlice);

/// Contains ConnectionId.
pub type ConnectionSet = bit_set::BitSet;
type Ports = ConnectionVec<WireName>;

#[derive(Debug, Clone)]
pub struct Module {
    pub id: ModuleId,
    pub name: String,
    pub clock: Option<ConnectionId>,
    pub instances: InstanceVec<Instance>,
    instance_names: HashMap<String, InstanceId>,
    pub wires: WireVec<WireProperties>,
    pub connection_wires: ConnectionVec<WireId>,
    // FIXME: make (String, usize) its own struct that implements Display.
    pub ports: ConnectionVec<WireName>,
    pub input_ports: InputVec<ConnectionId>,
    pub output_ports: OutputVec<ConnectionId>,
    comb_input_deps: WireVec<Vec<InputId>>,
    pub wire_names: WireVec<Option<WireName>>,
    pub comb_wire_dag: WireGraph,
}

#[derive(Debug, Clone)]
pub struct WireName {
    pub name: String,
    pub offset: usize,
}
impl WireName {
    pub fn new(name: String, offset: usize) -> Self {
        Self { name, offset }
    }
}
impl std::fmt::Display for WireName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{}]", self.name, self.offset)
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: String,
    pub architecture: InstanceType,
    pub connections: ConnectionVec<WireId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireValue {
    _0,
    _1,
}

impl std::convert::From<WireValue> for WireId {
    fn from(value: WireValue) -> Self {
        match value {
            WireValue::_0 => WireId::from_usize(0),
            WireValue::_1 => WireId::from_usize(1),
        }
    }
}

impl std::convert::From<WireValue> for bool {
    fn from(value: WireValue) -> Self {
        match value {
            WireValue::_0 => false,
            WireValue::_1 => true,
        }
    }
}

impl std::convert::From<bool> for WireValue {
    fn from(value: bool) -> Self {
        if value {
            WireValue::_1
        } else {
            WireValue::_0
        }
    }
}

impl std::ops::Not for WireValue {
    type Output = Self;
    fn not(self) -> Self::Output {
        match self {
            Self::_0 => Self::_1,
            Self::_1 => Self::_0,
        }
    }
}
impl std::ops::BitAnd for WireValue {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        if self == Self::_0 || rhs == Self::_0 {
            Self::_0
        } else {
            Self::_1
        }
    }
}
impl std::ops::BitOr for WireValue {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        if self == Self::_1 || rhs == Self::_1 {
            Self::_1
        } else {
            Self::_0
        }
    }
}
impl std::ops::BitXor for WireValue {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self::Output {
        if self == rhs {
            Self::_0
        } else {
            Self::_1
        }
    }
}

#[derive(Debug, Clone)]
pub enum InstanceType {
    Gate(Gate),
    Module(ModuleId),
    Input(InputId, ConnectionId),
    Tie(WireValue),
}

#[derive(Debug, Clone)]
struct Input {
    port: String,
    offset: usize,
}

#[derive(Debug, Clone)]
pub struct WireProperties {
    pub source: (InstanceId, OutputId),
    pub output: Option<ConnectionId>,
    pub sinks: Vec<(InstanceId, InputId)>,
}

#[derive(Debug, Clone)]
pub struct WireGraph {
    pub graph: petgraph::Graph<WireId, ()>,
    pub node_indices: WireVec<petgraph::graph::NodeIndex>,
}

impl Module {
    pub fn from_yosys(
        yosys_module: &yosys::Module,
        id: ModuleId,
        name: &str,
        netlist: &Netlist,
    ) -> Result<Self> {
        let ports = yosys_ext::ports(yosys_module);
        let input_ports = InputVec::from_vec(yosys_ext::filter_ports(
            yosys_module,
            &ports,
            yosys::PortDirection::Input,
        ));
        let output_ports = OutputVec::from_vec(yosys_ext::filter_ports(
            yosys_module,
            &ports,
            yosys::PortDirection::Output,
        ));
        let inout_ports =
            yosys_ext::filter_ports(yosys_module, &ports, yosys::PortDirection::InOut);
        if !inout_ports.is_empty() {
            bail!("Gadgets cannot have inout ports.");
        }
        let instances = yosys_ext::instances(yosys_module, &ports, &input_ports, netlist)?;
        let instance_names = instances
            .iter_enumerated()
            .map(|(instance_id, instance)| (instance.name.clone(), instance_id))
            .collect();
        let n_wires = yosys_ext::count_wires(yosys_module);
        let connection_wires = yosys_ext::connection_wires(yosys_module, &ports)?;
        let wires_output_connection =
            yosys_ext::wires_output_connection(yosys_module, n_wires, &output_ports, &ports)?;
        let wires_source = wires_source(n_wires, &instances, netlist)?;
        let wire_sinks = wires_sinks(n_wires, &instances, netlist);
        let wires = itertools::izip!(wires_source, wires_output_connection, wire_sinks)
            .map(|(source, output, sinks)| WireProperties {
                source,
                output,
                sinks,
            })
            .collect::<WireVec<_>>();
        eprintln!("Sorting wires for module {}", name);
        let comb_wire_dag = comb_wire_dag(&instances, &wires, netlist)?;
        /*
        std::fs::write(
            format!("wg_{}.dot", name),
            format!(
                "{:?}",
                petgraph::dot::Dot::with_config(
                    &comb_wire_dag.graph,
                    &[petgraph::dot::Config::EdgeNoLabel]
                )
            )
            .as_bytes(),
        )
        .unwrap();
        */
        let eval_sorted_wires = sort_wires(&comb_wire_dag)?;
        let clocks = instances
            .iter()
            .flat_map(|instance| instance.clock(netlist))
            .collect::<BTreeSet<_>>();
        if clocks.len() > 1 {
            bail!(
                "Cannot have more than one clock signal (clock signals: {:?})",
                clocks
            );
        }
        let clock = clocks.into_iter().next().map(|clock| {
            let InstanceType::Input(_, conid) = instances[wires[clock].source.0].architecture
            else {
                unreachable!();
            };
            conid
        });
        eprintln!("module {}:", name);
        let comb_depsets = comb_depsets(&instances, &wires, &eval_sorted_wires, netlist)?;
        let comb_inputs = output_ports
            .iter()
            .fold(ConnectionSet::new(), |mut set, con_id| {
                set.union_with(&comb_depsets[connection_wires[*con_id]]);
                set
            });
        let comb_input_deps = comb_input_deps(&comb_depsets, &connection_wires, &wires, &instances);
        eprintln!("module {}:", name);
        //eprintln!("\tcomb_depsets: {:?}", comb_depsets);
        eprintln!("\tcomb_inputs: {:?}", comb_inputs);
        let wire_names = yosys_ext::wire_names(yosys_module, &wires);
        Ok(Module {
            id,
            name: name.to_owned(),
            clock,
            instances,
            instance_names,
            wires,
            connection_wires,
            ports,
            input_ports,
            output_ports,
            comb_input_deps,
            wire_names,
            comb_wire_dag,
        })
    }
    fn comb_input_deps(&self, connection: ConnectionId) -> &[InputId] {
        self.comb_input_deps[self.connection_wires[connection]].as_slice()
    }
    /// Add dependencies in comb_wire_dag and comb_input_deps to match worst-case inference based
    /// on gadget annotations.
    /// TODO: document this assumption.
    pub fn update_pipeline_gadget_deps(&mut self, gadget: &StaticGadget) {
        assert_eq!(gadget.arch, super::gadget::GadgetArch::Pipeline);
        let mut inputs_by_latency =
            LatencyVec::from_vec(vec![vec![]; (gadget.max_latency + 1usize).into()]);
        for (input_id, con_id) in self.input_ports.iter_enumerated() {
            if Some(*con_id) != self.clock {
                inputs_by_latency[gadget.valid_latencies[*con_id][0]].push(input_id);
            }
        }
        for con_id in self.output_ports.iter() {
            for input_id in &inputs_by_latency[gadget.valid_latencies[*con_id][0]] {
                self.comb_wire_dag.graph.add_edge(
                    self.comb_wire_dag.node_indices
                        [self.connection_wires[self.input_ports[*input_id]]],
                    self.comb_wire_dag.node_indices[self.connection_wires[*con_id]],
                    (),
                );
                self.comb_input_deps[self.connection_wires[*con_id]].push(*input_id);
            }
        }
    }
}

/// Unitility functions for yosys modules.
mod yosys_ext {
    use super::{
        ConnectionId, ConnectionVec, Instance, InstanceType, InstanceVec, Netlist, Ports, WireId,
        WireName, WireProperties, WireValue, WireVec,
    };
    use anyhow::Result;
    use yosys_netlist_json as yosys;

    pub fn ports(yosys_module: &yosys::Module) -> Ports {
        // Sort port names for reproducibility.
        let mut port_names = yosys_module.ports.keys().collect::<Vec<_>>();
        port_names.sort_unstable();
        port_names
            .iter()
            .flat_map(|port_name| {
                (0..yosys_module.ports[*port_name].bits.len())
                    .map(|offset| WireName::new((*port_name).clone(), offset))
            })
            .collect::<ConnectionVec<_>>()
    }

    pub fn filter_ports(
        yosys_module: &yosys::Module,
        ports: &Ports,
        direction: yosys::PortDirection,
    ) -> Vec<ConnectionId> {
        ports
            .iter_enumerated()
            .filter_map(|(id, wire_name)| {
                (yosys_module.ports[&wire_name.name].direction == direction).then_some(id)
            })
            .collect::<Vec<_>>()
    }

    pub fn instances(
        yosys_module: &yosys::Module,
        ports: &Ports,
        input_ports: &super::InputVec<ConnectionId>,
        netlist: &Netlist,
    ) -> Result<InstanceVec<Instance>> {
        // Sort cells for reproducibility.
        let mut module_cells: Vec<_> = yosys_module.cells.iter().collect();
        module_cells.sort_unstable_by_key(|&(name, _cell)| name);
        // Yosys never uses wire 0 and 1, and instead uses "0" and "1".
        // We make use of these indices to make the handling more uniform.
        let tie_instances = [
            Ok(Instance {
                name: "TIELO".to_owned(),
                architecture: InstanceType::Tie(WireValue::_0),
                connections: ConnectionVec::from_vec(vec![WireId::from_usize(0)]),
            }),
            Ok(Instance {
                name: "TIEHI".to_owned(),
                architecture: InstanceType::Tie(WireValue::_1),
                connections: ConnectionVec::from_vec(vec![WireId::from_usize(1)]),
            }),
        ];
        let inner_instances = module_cells
            .into_iter()
            .map(|(cell_name, cell)| Instance::from_cell(cell, cell_name, netlist));
        let input_instances = input_ports
            .indices()
            .map(|input_id| Instance::from_input_of(yosys_module, input_id, ports, input_ports));
        tie_instances
            .into_iter()
            .chain(inner_instances)
            .chain(input_instances)
            .collect()
    }

    pub fn cell_connection_wires<'a>(
        cell: &'a yosys::Cell,
        connection_names: impl IntoIterator<Item = &'a WireName>,
    ) -> Result<ConnectionVec<WireId>> {
        cell_connection_wires_ref(
            cell,
            connection_names
                .into_iter()
                .map(|wire_name| (&wire_name.name, wire_name.offset)),
        )
    }
    pub fn cell_connection_wires_ref(
        cell: &yosys::Cell,
        connection_names: impl IntoIterator<Item = (impl AsRef<str>, usize)>,
    ) -> Result<ConnectionVec<WireId>> {
        connection_names
            .into_iter()
            .map(|(port_name, offset)| cell.connections[port_name.as_ref()][offset].try_into())
            .collect()
    }
    pub fn port_desc2wire_id(module: &yosys::Module, wire_name: &WireName) -> Result<WireId> {
        module.ports[&wire_name.name].bits[wire_name.offset].try_into()
    }

    /// Number of wires in module, and check that they are consecutively numbered
    pub fn count_wires(module: &yosys::Module) -> usize {
        let mut wire_ids = module
            .netnames
            .values()
            .flat_map(|netname| netname.bits.iter())
            .filter_map(|bitval| (*bitval).try_into().ok())
            .collect::<Vec<_>>();
        // TIELO and TIEHI wires.
        wire_ids.push(WireId::from_usize(0));
        wire_ids.push(WireId::from_usize(1));
        wire_ids.sort_unstable();
        wire_ids.dedup();
        assert!(wire_ids
            .last()
            .filter(|x| **x + 1 == wire_ids.len())
            .is_some());
        wire_ids.len()
    }

    /// Map connection ids to wire ids
    pub fn connection_wires(
        module: &yosys::Module,
        ports: &Ports,
    ) -> Result<ConnectionVec<WireId>> {
        ports
            .iter()
            .map(|wire_name| port_desc2wire_id(module, wire_name))
            .collect()
    }

    /// Check whether wires are output, then give connection
    pub fn wires_output_connection(
        module: &yosys::Module,
        n_wires: usize,
        output_ports: &super::OutputVec<ConnectionId>,
        ports: &Ports,
    ) -> Result<super::WireVec<Option<ConnectionId>>> {
        let mut wires_output = super::WireVec::from_vec(vec![None; n_wires]);
        for con_id in output_ports {
            let wire_id = port_desc2wire_id(module, &ports[*con_id])?;
            wires_output[wire_id] = Some(*con_id);
        }
        Ok(wires_output)
    }

    pub fn wire_names(
        module: &yosys::Module,
        wires: &WireVec<WireProperties>,
    ) -> WireVec<Option<WireName>> {
        let mut res = WireVec::from_vec(vec![None; wires.len()]);
        for (name, netname) in module.netnames.iter() {
            for (offset, bitval) in netname.bits.iter().enumerate() {
                let wire_id: WireId = (*bitval).try_into().unwrap();
                res[wire_id] = Some(WireName::new(name.clone(), offset));
            }
        }
        res
    }
}

impl InstanceType {
    fn from_cell(cell: &yosys::Cell, netlist: &Netlist) -> Result<Self> {
        Ok(if let Some(module_id) = netlist.id_of(&cell.cell_type) {
            InstanceType::Module(module_id)
        } else if let Ok(gate) = cell.cell_type.parse() {
            InstanceType::Gate(gate)
        } else {
            bail!(
                "Cell type '{}' is not a gadget, nor a fv_cell gate.",
                cell.cell_type
            )
        })
    }
    fn output_ids(&self, netlist: &Netlist) -> std::ops::Range<OutputId> {
        let n_outputs = match self {
            InstanceType::Gate(gate) => gate.output_ports().len(),
            InstanceType::Module(module_id) => netlist[*module_id].output_ports.len(),
            InstanceType::Input(..) | InstanceType::Tie(_) => 1,
        };
        OutputId::from_usize(0)..OutputId::from_usize(n_outputs)
    }
    fn output_ports<'nl>(&self, netlist: &'nl Netlist) -> &'nl OutputSlice<ConnectionId> {
        const INPUT_GATE_OUTPUT_PORTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(0)];
        match self {
            InstanceType::Gate(gate) => gate.output_ports(),
            InstanceType::Module(module_id) => netlist[*module_id].output_ports.as_slice(),
            InstanceType::Input(..) | InstanceType::Tie(_) => {
                OutputSlice::new(&INPUT_GATE_OUTPUT_PORTS)
            }
        }
    }
    fn input_ports<'nl>(&self, netlist: &'nl Netlist) -> &'nl InputSlice<ConnectionId> {
        const INPUT_GATE_INPUT_PORTS: &InputSlice<ConnectionId> = InputSlice::from_slice(&[]);
        match self {
            InstanceType::Gate(gate) => gate.input_ports(),
            InstanceType::Module(module_id) => netlist[*module_id].input_ports.as_slice(),
            InstanceType::Input(..) | InstanceType::Tie(_) => INPUT_GATE_INPUT_PORTS,
        }
    }

    fn clock(&self, netlist: &Netlist) -> Option<ConnectionId> {
        match self {
            InstanceType::Gate(gate) => gate.clock(),
            InstanceType::Module(module_id) => netlist[*module_id].clock,
            InstanceType::Input(..) | InstanceType::Tie(_) => None,
        }
    }

    fn comb_deps(
        &self,
        output: ConnectionId,
        netlist: &Netlist,
    ) -> Result<std::borrow::Cow<[ConnectionId]>> {
        Ok(match self {
            InstanceType::Gate(gate) => {
                assert_eq!(gate.output_ports(), &[output]);
                gate.comb_deps().into()
            }
            InstanceType::Module(module_id) => {
                let module = &netlist[*module_id];
                module
                    .comb_input_deps(output)
                    .iter()
                    .map(|input_id| module.input_ports[*input_id])
                    .collect::<Vec<_>>()
                    .into()
            }
            InstanceType::Input(..) | InstanceType::Tie(_) => [].as_slice().into(),
        })
    }
}

impl Instance {
    fn from_cell(cell: &yosys::Cell, name: &str, netlist: &Netlist) -> Result<Self> {
        let architecture = InstanceType::from_cell(cell, netlist)?;
        let connections = match architecture {
            InstanceType::Gate(gate) => {
                yosys_ext::cell_connection_wires_ref(cell, gate.connections())?
            }
            InstanceType::Module(module_id) => {
                yosys_ext::cell_connection_wires(cell, netlist[module_id].ports.iter())?
            }
            InstanceType::Input(..) | InstanceType::Tie(_) => unreachable!(),
        };
        Ok(Instance {
            name: name.to_owned(),
            architecture,
            connections,
        })
    }
    fn from_input_of(
        yosys_module: &yosys::Module,
        input_id: InputId,
        ports: &Ports,
        input_ports: &InputVec<ConnectionId>,
    ) -> Result<Self> {
        let connection_id = input_ports[input_id];
        let architecture = InstanceType::Input(input_id, connection_id);
        let connection = yosys_ext::port_desc2wire_id(yosys_module, &ports[connection_id])?;
        let connections = ConnectionVec::from_vec(vec![connection]);
        Ok(Instance {
            name: format!("input:{}", ports[connection_id]),
            architecture,
            connections,
        })
    }

    fn clock(&self, netlist: &Netlist) -> Option<WireId> {
        self.architecture
            .clock(netlist)
            .map(|clk| self.connections[clk])
    }

    /*
    pub fn output_wires<'a>(&'a self, netlist: &'a Netlist) -> impl Iterator<Item = WireId> + 'a {
        self.architecture
            .output_ports(netlist)
            .iter()
            .map(move |con| self.connections[*con])
    }
    */
}

fn wires_source(
    n_wires: usize,
    instances: &InstanceVec<Instance>,
    netlist: &Netlist,
) -> Result<WireVec<(InstanceId, OutputId)>> {
    let mut sources = WireVec::<Option<(InstanceId, OutputId)>>::from_vec(vec![None; n_wires]);
    for (instance_id, instance) in instances.iter_enumerated() {
        for (output_id, output_port) in instance
            .architecture
            .output_ports(netlist)
            .iter_enumerated()
        {
            let wire = instance.connections[*output_port];
            if let Some(other_instance) = sources[wire] {
                bail!(
                    "Wire {} is an output of both {} and {}.",
                    wire,
                    instances[other_instance.0].name,
                    instance.name
                );
            } else {
                sources[wire] = Some((instance_id, output_id));
            }
        }
    }
    sources
        .into_iter_enumerated()
        .map(|(wire, instance)| {
            if let Some(inst) = instance {
                Ok(inst)
            } else {
                bail!(
                    "Wire {} is not the output of any cell, nor a module input.",
                    wire
                )
            }
        })
        .collect()
}

fn wires_sinks(
    n_wires: usize,
    instances: &InstanceVec<Instance>,
    netlist: &Netlist,
) -> WireVec<Vec<(InstanceId, InputId)>> {
    let mut res = WireVec::from_vec(vec![vec![]; n_wires]);
    for (instance_id, instance) in instances.iter_enumerated() {
        for (input_id, con_id) in instance.architecture.input_ports(netlist).iter_enumerated() {
            let wire_id = instance.connections[*con_id];
            res[wire_id].push((instance_id, input_id));
        }
    }
    res
}

fn comb_wire_dag(
    instances: &InstanceVec<Instance>,
    wires: &WireVec<WireProperties>,
    netlist: &Netlist,
) -> Result<WireGraph> {
    let mut graph = petgraph::Graph::new();
    let node_indices = wires
        .indices()
        .map(|wire_id| graph.add_node(wire_id))
        .collect::<WireVec<_>>();
    for instance in instances.iter() {
        //eprintln!("instance {}", instance.name);
        for output in instance.architecture.output_ports(netlist).iter() {
            //eprintln!("\t output wire: {}", instance.connections[*output]);
            for input in instance.architecture.comb_deps(*output, netlist)?.as_ref() {
                //eprintln!("\t\t input wire: {}", instance.connections[*input]);
                /*
                eprintln!(
                    "\t\tAdd edge {} -> {}",
                    instance.connections[*input], instance.connections[*output],
                );
                */
                graph.add_edge(
                    node_indices[instance.connections[*input]],
                    node_indices[instance.connections[*output]],
                    (),
                );
            }
        }
    }
    Ok(WireGraph {
        graph,
        node_indices,
    })
}

fn sort_wires(comb_wire_dag: &WireGraph) -> Result<Vec<WireId>> {
    Ok(petgraph::algo::toposort(&comb_wire_dag.graph, None)
        .map_err(|cycle| {
            anyhow!(
                "Gadget contains combinational loop involving wire {}",
                comb_wire_dag.graph[cycle.node_id()].index()
            )
        })?
        .into_iter()
        .map(|node_id| comb_wire_dag.graph[node_id])
        .collect())
}

fn comb_depsets(
    instances: &InstanceVec<Instance>,
    wires: &WireVec<WireProperties>,
    eval_sorted_wires: &[WireId],
    netlist: &Netlist,
) -> Result<WireVec<ConnectionSet>> {
    let mut dependency_sets = WireVec::from_vec(vec![ConnectionSet::new(); wires.len()]);
    for wire_id in eval_sorted_wires {
        let (instance_id, output_id) = &wires[*wire_id].source;
        let instance = &instances[*instance_id];
        let output_con_id = instance.architecture.output_ports(netlist)[*output_id];
        //eprintln!("wire {} from instance {}", wire_id, instance.name);
        let mut dep_set = ConnectionSet::new();
        if let InstanceType::Input(_, con_id) = instance.architecture {
            dep_set.insert(con_id.index());
        } else {
            for con_input_id in instance
                .architecture
                .comb_deps(output_con_id, netlist)?
                .as_ref()
            {
                /*
                eprintln!(
                    "\tinput wire {} with depset {:?}",
                    instance.connections[*con_input_id],
                    &dependency_sets[instance.connections[*con_input_id]]
                );
                    */
                dep_set.union_with(&dependency_sets[instance.connections[*con_input_id]]);
            }
        }
        //eprintln!("\tend depset: {:?}", dep_set);
        dependency_sets[*wire_id] = dep_set;
    }
    Ok(dependency_sets)
}

impl TryFrom<yosys::BitVal> for WireId {
    type Error = anyhow::Error;
    fn try_from(value: yosys::BitVal) -> Result<Self, Self::Error> {
        Ok(match value {
            yosys::BitVal::N(x) => {
                assert!(x >= 2);
                WireId::from_usize(x)
            }
            yosys::BitVal::S(yosys::SpecialBit::_0) => WireId::from_usize(0),
            yosys::BitVal::S(yosys::SpecialBit::_1) => WireId::from_usize(1),
            yosys::BitVal::S(v @ yosys::SpecialBit::X)
            | yosys::BitVal::S(v @ yosys::SpecialBit::Z) => {
                bail!(
                    "Wires assigned to {} not supported.",
                    if v == yosys::SpecialBit::X {
                        "'x'"
                    } else {
                        "'z'"
                    }
                )
            }
        })
    }
}

fn comb_input_deps(
    comb_depsets: &WireVec<ConnectionSet>,
    connection_wires: &ConnectionVec<WireId>,
    wires: &WireVec<WireProperties>,
    instances: &InstanceVec<Instance>,
) -> WireVec<Vec<InputId>> {
    comb_depsets
        .iter()
        .map(|depset| {
            depset
                .iter()
                .filter_map(|con_id| {
                    if let InstanceType::Input(input_id, _) =
                        instances[wires[connection_wires[con_id]].source.0].architecture
                    {
                        Some(input_id)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect()
}
