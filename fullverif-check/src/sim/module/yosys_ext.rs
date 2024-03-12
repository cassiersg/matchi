use super::ModList;
use super::{
    ConnectionId, ConnectionVec, Instance, InstanceType, InstanceVec, Netlist, Ports, WireId,
    WireName, WireProperties, WireVec,
};
use crate::sim::WireValue;
use anyhow::Result;
use yosys_netlist_json as yosys;

pub fn ports(yosys_module: &yosys::Module, clock: Option<WireId>) -> (Ports, Option<WireName>) {
    // Sort port names for reproducibility.
    let mut port_names = yosys_module.ports.keys().collect::<Vec<_>>();
    port_names.sort_unstable();
    let mut clock_name = None;
    let mut ports: Ports = ConnectionVec::new();
    for port_name in port_names.iter() {
        for (offset, bitval) in yosys_module.ports[*port_name].bits.iter().enumerate() {
            let wire_name = WireName::new((*port_name).clone(), offset);
            if (*bitval).try_into().is_ok_and(|w_id| Some(w_id) == clock) {
                clock_name = Some(wire_name);
            } else {
                ports.push(wire_name);
            }
        }
    }
    (ports, clock_name)
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

pub fn module_instances(
    yosys_module: &yosys::Module,
    netlist: &impl ModList,
) -> Result<Vec<Instance>> {
    // Sort cells for reproducibility.
    let mut module_cells: Vec<_> = yosys_module.cells.iter().collect();
    module_cells.sort_unstable_by_key(|&(name, _cell)| name);
    module_cells
        .into_iter()
        .map(|(cell_name, cell)| Instance::from_cell(cell, cell_name, netlist))
        .collect()
}

pub fn tie_instances() -> impl Iterator<Item = Instance> {
    // Yosys never uses wire 0 and 1, and instead uses "0" and "1".
    // We make use of these indices to make the handling more uniform.
    [
        Instance {
            name: "TIELO".to_owned(),
            architecture: InstanceType::Tie(WireValue::_0),
            connections: ConnectionVec::from_vec(vec![WireId::from_usize(0)]),
        },
        Instance {
            name: "TIEHI".to_owned(),
            architecture: InstanceType::Tie(WireValue::_1),
            connections: ConnectionVec::from_vec(vec![WireId::from_usize(1)]),
        },
    ]
    .into_iter()
}
pub fn input_instances<'a>(
    yosys_module: &'a yosys::Module,
    ports: &'a Ports,
    input_ports: &'a super::InputVec<ConnectionId>,
) -> impl Iterator<Item = Result<Instance>> + 'a {
    input_ports
        .indices()
        .map(|input_id| Instance::from_input_of(yosys_module, input_id, ports, input_ports))
}

pub fn cell_connection_wire(cell: &yosys::Cell, connection_name: WireName<&str>) -> Result<WireId> {
    cell.connections[connection_name.name][connection_name.offset].try_into()
}

pub fn cell_connection_wires<'a, T: AsRef<str>>(
    cell: &'a yosys::Cell,
    connection_names: impl IntoIterator<Item = impl AsRef<WireName<T>>>,
) -> Result<ConnectionVec<WireId>> {
    connection_names
        .into_iter()
        .map(|wire_name| {
            cell.connections[wire_name.as_ref().as_name_ref().name][wire_name.as_ref().offset]
                .try_into()
        })
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
pub fn connection_wires(module: &yosys::Module, ports: &Ports) -> Result<ConnectionVec<WireId>> {
    ports
        .iter()
        .map(|wire_name| port_desc2wire_id(module, wire_name))
        .collect()
}

/// Map connection ids to wire ids
pub fn ports_is_input(module: &yosys::Module, ports: &Ports) -> ConnectionVec<bool> {
    ports
        .iter()
        .map(|wire_name| match module.ports[&wire_name.name].direction {
            yosys::PortDirection::Input => true,
            yosys::PortDirection::Output => false,
            yosys::PortDirection::InOut => unreachable!("inout port"),
        })
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
