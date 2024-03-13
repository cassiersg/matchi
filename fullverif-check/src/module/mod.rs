use super::gadget::{LatencyVec, PipelineGadget};
use super::netlist::ModList;
use super::ModuleId;
use crate::type_utils::new_id;
use anyhow::{bail, Result};
use yosys_netlist_json as yosys;

mod builder;
mod instance;
mod yosys_ext;

pub use instance::{Instance, InstanceType};

pub use builder::ModListBuilder;

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
    pub clock: Option<WireName>,
    pub instances: InstanceVec<Instance>,
    pub wires: WireVec<WireProperties>,
    pub connection_wires: ConnectionVec<WireId>,
    pub ports: ConnectionVec<WireName>,
    pub input_ports: InputVec<ConnectionId>,
    pub output_ports: OutputVec<ConnectionId>,
    pub port_is_input: ConnectionVec<bool>,
    pub wire_names: WireVec<Option<WireName>>,
}

#[derive(Debug, Clone)]
pub struct ModuleCombDeps {
    module_id: ModuleId,
    pub comb_input_deps: WireVec<Vec<InputId>>,
    pub comb_wire_dag: WireGraph,
}

#[derive(Debug, Clone, Copy)]
pub struct WireName<T = String> {
    pub name: T,
    pub offset: usize,
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
impl ModuleCombDeps {
    /// Add dependencies in comb_wire_dag and comb_input_deps to match worst-case inference based
    /// on gadget annotations.
    /// TODO: document this assumption.
    pub fn update_pipeline_gadget_deps(&mut self, gadget: &PipelineGadget, modlist: &impl ModList) {
        let module = modlist.module(self.module_id);
        let mut inputs_by_latency =
            LatencyVec::from_vec(vec![vec![]; (gadget.max_latency + 1usize).into()]);
        for (input_id, con_id) in module.input_ports.iter_enumerated() {
            inputs_by_latency[gadget.latency[*con_id]].push(input_id);
        }
        for con_id in module.output_ports.iter() {
            for input_id in &inputs_by_latency[gadget.latency[*con_id]] {
                self.comb_wire_dag.graph.add_edge(
                    self.comb_wire_dag.node_indices
                        [module.connection_wires[module.input_ports[*input_id]]],
                    self.comb_wire_dag.node_indices[module.connection_wires[*con_id]],
                    (),
                );
                self.comb_input_deps[module.connection_wires[*con_id]].push(*input_id);
            }
        }
    }
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

impl<T> WireName<T> {
    pub const fn single_port(name: T) -> Self {
        Self { name, offset: 0 }
    }
}
impl WireName {
    pub fn new(name: String, offset: usize) -> Self {
        Self { name, offset }
    }
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}
impl<T: AsRef<str>> WireName<T> {
    fn as_name_ref(&self) -> WireName<&str> {
        WireName {
            name: self.name.as_ref(),
            offset: self.offset,
        }
    }
}
impl<T> AsRef<WireName<T>> for WireName<T> {
    fn as_ref(&self) -> &WireName<T> {
        self
    }
}
impl std::fmt::Display for WireName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{}]", self.name, self.offset)
    }
}
