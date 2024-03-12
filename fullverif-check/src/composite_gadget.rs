use crate::gadgets::Gadget;
use crate::sim::fv_cells::Gate;
use crate::type_utils::new_id;
use fnv::FnvHashMap as HashMap;
use yosys_netlist_json as yosys;

use anyhow::{anyhow, bail, Context, Result};

new_id!(InstanceId, InstanceVec, InstanceSlice);
new_id!(WireId, WireVec, WireSlice);
pub use crate::sim::module::{ConnectionId, ConnectionVec};
new_id!(GadgetId, GadgetVec, GadgetSlice);
new_id!(InputId, InputVec, InputSlice);

type ConnectionSet = bit_set::BitSet;

#[derive(Debug, Clone)]
struct CompositeGadget {
    instances: InstanceVec<Instance>,
    wires: WireVec<WireProperties>,
    gadget_id: GadgetId,
    connection_wires: ConnectionVec<Option<WireId>>,
}

#[derive(Debug, Clone)]
struct Instance {
    name: String,
    architecture: InstanceType,
    connections: ConnectionVec<WireId>,
}

#[derive(Debug, Clone)]
enum BitVal {
    Wire(WireId),
    Value(WireValue),
}

#[derive(Debug, Clone)]
pub enum WireValue {
    _0,
    _1,
}

#[derive(Debug, Clone)]
enum InstanceType {
    Gate(Gate),
    Gadget(GadgetId),
    Input(InputId, ConnectionId),
}

#[derive(Debug, Clone)]
struct Input {
    port: String,
    offset: usize,
}

#[derive(Debug, Clone)]
struct WireProperties {
    output_share: bool,
    source: (InstanceId, ConnectionId),
    output: Option<ConnectionId>,
}

#[derive(Debug, Clone)]
pub struct GadgetLibrary<'nl> {
    pub gadgets: GadgetVec<Gadget<'nl>>,
    names: HashMap<String, GadgetId>,
}

impl<'nl> std::convert::TryFrom<&'nl yosys::Netlist>
    for crate::composite_gadget::GadgetLibrary<'nl>
{
    type Error = anyhow::Error;
    fn try_from(netlist: &'nl yosys::Netlist) -> Result<Self> {
        let mut netlist_modules: Vec<_> = netlist.modules.iter().collect();
        netlist_modules.sort_unstable_by_key(|&(name, _module)| name);
        let gadgets = netlist_modules
            .iter()
            .filter_map(|(module_name, module)| {
                crate::gadgets::module2gadget(module, module_name).transpose()
            })
            .collect::<Result<crate::composite_gadget::GadgetVec<_>>>()?;
        let names = gadgets
            .iter_enumerated()
            .map(|(gadget_id, gadget)| (gadget.name.to_owned(), gadget_id))
            .collect::<HashMap<_, _>>();
        dbg!(names.keys().collect::<Vec<_>>());
        let gadget_ids = gadgets.indices();
        let mut res = crate::composite_gadget::GadgetLibrary { gadgets, names };
        Ok(res)
    }
}

impl<'nl> GadgetLibrary<'nl> {
    pub fn get<'s>(&'s self, gadget_name: impl AsRef<str>) -> Option<&'s Gadget<'nl>> {
        self.id_of(gadget_name)
            .map(|gadget_id| &self.gadgets[gadget_id])
    }
    pub fn id_of(&self, gadget_name: impl AsRef<str>) -> Option<GadgetId> {
        self.names.get(gadget_name.as_ref()).copied()
    }
}

fn module_instances(gadget: &Gadget, library: &GadgetLibrary) -> Result<InstanceVec<Instance>> {
    // Sort cells for reproducibility.
    let mut module_cells: Vec<_> = gadget.module.cells.iter().collect();
    module_cells.sort_unstable_by_key(|&(name, _cell)| name);
    module_cells
        .into_iter()
        .map(|(cell_name, cell)| Instance::from_cell(cell, cell_name, library))
        .chain(
            gadget
                .input_ports
                .indices()
                .map(|input_id| Instance::from_input(gadget, input_id)),
        )
        .collect()
}

/// Number of wires in module, and check that they are consecutively numbered
fn count_wires(gadget: &Gadget) -> usize {
    let mut wire_ids = gadget
        .module
        .netnames
        .values()
        .flat_map(|netname| netname.bits.iter())
        .filter_map(|bitval| {
            if let Ok(BitVal::Wire(wire_id)) = (*bitval).try_into() {
                Some(wire_id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    wire_ids.sort_unstable();
    wire_ids.dedup();
    assert!(wire_ids
        .last()
        .filter(|x| **x + 1 == wire_ids.len())
        .is_some());
    wire_ids.len()
}

fn port_desc2wire_id(gadget: &Gadget, port_name: &str, offset: usize) -> Option<WireId> {
    let bitval = gadget.module.ports[port_name].bits[offset];
    bitval.try_into().ok()
}

/// Check whether wires are output shares.
fn wires_output_share(gadget: &Gadget, n_wires: usize) -> WireVec<bool> {
    let mut wires_output = WireVec::from_vec(vec![false; n_wires]);
    for bitval in gadget
        .outputs
        .keys()
        .flat_map(|sharing| gadget.sharing_bits(*sharing).iter())
    {
        let wire_id: Result<WireId, _> = (*bitval).try_into();
        if let Ok(wire_id) = wire_id {
            wires_output[wire_id] = true;
        }
    }
    wires_output
}

/// Check whether wires are output, then give connection
fn wires_output_connection(gadget: &Gadget, n_wires: usize) -> WireVec<Option<ConnectionId>> {
    let mut wires_output = WireVec::from_vec(vec![None; n_wires]);
    for con_id in &gadget.output_ports {
        let (port_name, offset) = gadget.ports[*con_id];
        if let Some(wire_id) = port_desc2wire_id(gadget, port_name, offset) {
            wires_output[wire_id] = Some(*con_id);
        }
    }
    wires_output
}

/// Map connection ids to wire ids
fn connection_wires(gadget: &Gadget) -> ConnectionVec<Option<WireId>> {
    gadget
        .ports
        .iter()
        .map(|(port_name, offset)| port_desc2wire_id(gadget, port_name, *offset))
        .collect()
}

fn wires_source(
    n_wires: usize,
    instances: &InstanceVec<Instance>,
    library: &GadgetLibrary,
) -> Result<WireVec<(InstanceId, ConnectionId)>> {
    let mut sources = WireVec::<Option<(InstanceId, ConnectionId)>>::from_vec(vec![None; n_wires]);
    for (instance_id, instance) in instances.iter_enumerated() {
        for output_port in instance.architecture.output_ports(library) {
            let wire = instance.connections[*output_port];
            if let Some(other_instance) = sources[wire] {
                bail!(
                    "Wire {} is an output of both {} and {}.",
                    wire,
                    instances[other_instance.0].name,
                    instance.name
                );
            } else {
                sources[wire] = Some((instance_id, *output_port));
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

impl CompositeGadget {
    pub fn from_module(gadget: &Gadget, library: &GadgetLibrary) -> Result<Self> {
        dbg!(gadget.name);
        let instances = module_instances(gadget, library)?;
        let n_wires = count_wires(gadget);
        let connection_wires = connection_wires(gadget);
        let wires_output = wires_output_share(gadget, n_wires);
        let wires_output_connection = wires_output_connection(gadget, n_wires);
        let wires_source = wires_source(n_wires, &instances, library)?;
        let wires = itertools::izip!(wires_output, wires_source, wires_output_connection)
            .map(|(output_share, source, output)| WireProperties {
                output_share,
                source,
                output,
            })
            .collect();
        Ok(CompositeGadget {
            gadget_id: library.id_of(gadget.name).expect("Gadget not in library"),
            instances,
            wires,
            connection_wires,
        })
    }

    pub fn sort_wires(&self, library: &GadgetLibrary) -> Result<Vec<WireId>> {
        let mut wire_graph = petgraph::Graph::new();
        let node_indices = self
            .wires
            .indices()
            .map(|wire_id| wire_graph.add_node(wire_id))
            .collect::<WireVec<_>>();
        for instance in self.instances.iter() {
            for output in instance.architecture.output_ports(library).iter() {
                for input in instance
                    .architecture
                    .combinational_dependencies(*output, library)?
                    .as_ref()
                {
                    wire_graph.add_edge(
                        node_indices[instance.connections[*input]],
                        node_indices[instance.connections[*output]],
                        (),
                    );
                }
            }
        }
        Ok(petgraph::algo::toposort(&wire_graph, None)
            .map_err(|cycle| {
                anyhow!(
                    "Gadget contains combinational loop involving wire {}",
                    wire_graph[cycle.node_id()].index()
                )
            })?
            .into_iter()
            .map(|node_id| wire_graph[node_id])
            .collect())
    }

    fn combinational_input_dependencies(
        &self,
        connection: ConnectionId,
        library: &GadgetLibrary,
    ) -> Result<Vec<ConnectionId>> {
        let Some(wire) = self.connection_wires[connection] else {
            return Ok(vec![]);
        };
        let depsets = self.combinational_depsets(library)?;
        let gadget = &library.gadgets[self.gadget_id];
        let depset = &depsets[wire];
        Ok(gadget
            .input_ports
            .iter()
            .copied()
            .filter(|input_conid| depset.contains(input_conid.index()))
            .collect())
    }
    fn combinational_depsets(&self, library: &GadgetLibrary) -> Result<WireVec<ConnectionSet>> {
        // TODO store sorted_wires and combinational_dependencies.
        let sorted_wires = self.sort_wires(library)?;
        let mut dependency_sets = WireVec::from_vec(vec![ConnectionSet::new(); self.wires.len()]);
        for wire_id in &sorted_wires {
            let (instance_id, output_id) = &self.wires[*wire_id].source;
            let instance = &self.instances[*instance_id];
            let mut dep_set = ConnectionSet::new();
            for input_id in instance
                .architecture
                .combinational_dependencies(*output_id, library)?
                .as_ref()
            {
                dep_set.union_with(&dependency_sets[instance.connections[*input_id]]);
            }
            dependency_sets[*wire_id] = dep_set;
        }
        Ok(dependency_sets)
    }
}

fn connection_set2ids(set: &ConnectionSet) -> Vec<ConnectionId> {
    set.iter().map(ConnectionId::from_usize).collect()
}

impl InstanceType {
    fn from_cell(cell: &yosys::Cell, library: &GadgetLibrary) -> Result<Self> {
        Ok(
            if let Some(gadget_id) = library.names.get(&cell.cell_type) {
                InstanceType::Gadget(*gadget_id)
            } else if let Ok(gate) = cell.cell_type.parse() {
                InstanceType::Gate(gate)
            } else {
                bail!(
                    "Cell type '{}' is not a gadget, nor a fv_cell gate.",
                    cell.cell_type
                )
            },
        )
    }
    fn n_ports(&self, library: &GadgetLibrary) -> usize {
        match self {
            InstanceType::Gate(gate) => gate.connections().len(),
            InstanceType::Gadget(gadget_id) => library.gadgets[*gadget_id].ports.len(),
            InstanceType::Input(..) => 1,
        }
    }
    fn output_ports<'gl, 'nl: 'gl>(&self, library: &'gl GadgetLibrary<'nl>) -> &'gl [ConnectionId] {
        const INPUT_GATE_OUTPUT_PORTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(0)];
        match self {
            InstanceType::Gate(gate) => &gate.output_ports().raw,
            InstanceType::Gadget(gadget_id) => library.gadgets[*gadget_id].output_ports.as_slice(),
            InstanceType::Input(..) => INPUT_GATE_OUTPUT_PORTS.as_slice(),
        }
    }
    fn combinational_dependencies<'gl, 'nl: 'gl>(
        &self,
        output: ConnectionId,
        library: &'gl GadgetLibrary<'nl>,
    ) -> Result<std::borrow::Cow<[ConnectionId]>> {
        Ok(match self {
            InstanceType::Gate(gate) => todo!(),
            InstanceType::Gadget(gadget_id) => todo!(),
            InstanceType::Input(..) => [].as_slice().into(),
        })
    }
}
// FIXME check everything uses the same clock.

impl Instance {
    fn from_cell(cell: &yosys::Cell, name: &String, library: &GadgetLibrary) -> Result<Self> {
        let architecture = InstanceType::from_cell(cell, library)?;
        let connections = match architecture {
            InstanceType::Gate(gate) => cell_connection_wires(
                cell,
                gate.connections()
                    .iter()
                    .map(|w| (w.name, w.offset))
                    .collect(),
            )?,
            InstanceType::Gadget(gadget_id) => {
                cell_connection_wires(cell, library.gadgets[gadget_id].connections())?
            }
            InstanceType::Input(..) => unreachable!(),
        };
        let connections = connections
            .into_iter()
            .filter_map(|con| match con {
                BitVal::Wire(wire_id) => Some(wire_id),
                BitVal::Value(_) => None,
            })
            .collect();
        Ok(Instance {
            name: name.clone(),
            architecture,
            connections,
        })
    }
    fn from_input(gadget: &Gadget, input_id: InputId) -> Result<Self> {
        let connection_id = gadget.input_ports[input_id];
        let architecture = InstanceType::Input(input_id, connection_id);
        let (port_name, offset) = gadget.ports[connection_id];
        let connection =
            port_desc2wire_id(gadget, port_name, offset).expect("Input not connected.");
        let connections = ConnectionVec::from_vec(vec![connection]);
        Ok(Instance {
            name: format!("input:{}[{}]", port_name, offset),
            architecture,
            connections,
        })
    }
}

impl TryFrom<yosys::BitVal> for BitVal {
    type Error = anyhow::Error;
    fn try_from(value: yosys::BitVal) -> Result<Self> {
        value.try_into().map(BitVal::Wire).or_else(|s| {
            Ok(match s {
                yosys::SpecialBit::_0 => BitVal::Value(WireValue::_0),
                yosys::SpecialBit::_1 => BitVal::Value(WireValue::_1),
                v @ yosys::SpecialBit::X | v @ yosys::SpecialBit::Z => {
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
        })
    }
}

impl TryFrom<yosys::BitVal> for WireId {
    type Error = yosys::SpecialBit;
    fn try_from(value: yosys::BitVal) -> Result<Self, Self::Error> {
        match value {
            yosys::BitVal::N(x) => {
                assert!(x >= 2);
                Ok(WireId::from_idx(x - 2))
            }
            yosys::BitVal::S(s) => Err(s),
        }
    }
}

fn cell_connection_wires(
    cell: &yosys::Cell,
    connection_names: Vec<(&str, usize)>,
) -> Result<Vec<BitVal>> {
    connection_names
        .into_iter()
        .map(|(wire, offset)| cell.connections[wire][offset].try_into())
        .collect()
}
