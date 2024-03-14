use super::super::netlist::ModList;
use super::gates::Gate;
use super::ModuleId;
use anyhow::{bail, Result};
use yosys_netlist_json as yosys;

use super::{
    yosys_ext, ConnectionId, ConnectionVec, InputId, InputSlice, InputVec, OutputSlice, Ports,
    WireId, WireName,
};
use crate::WireValue;

#[derive(Debug, Clone)]
pub struct Instance {
    pub name: String,
    pub architecture: InstanceType,
    pub connections: ConnectionVec<WireId>,
}

#[derive(Debug, Clone)]
pub enum InstanceType {
    Gate(Gate),
    Module(ModuleId),
    Input(InputId, ConnectionId),
    Tie(WireValue),
    Clock,
}
impl InstanceType {
    fn from_cell(cell: &yosys::Cell, netlist: &impl ModList) -> Result<Self> {
        Ok(if let Some(module_id) = netlist.id_of(&cell.cell_type) {
            InstanceType::Module(module_id)
        } else if let Ok(gate) = cell.cell_type.parse() {
            InstanceType::Gate(gate)
        } else {
            bail!(
                "Cell type '{}' is not a gadget, nor a matchi_cells gate.",
                cell.cell_type
            )
        })
    }
    pub(super) fn output_ports<'nl>(
        &self,
        netlist: &'nl impl ModList,
    ) -> &'nl OutputSlice<ConnectionId> {
        const INPUT_GATE_OUTPUT_PORTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(0)];
        match self {
            InstanceType::Gate(gate) => gate.output_ports(),
            InstanceType::Module(module_id) => netlist.module(*module_id).output_ports.as_slice(),
            InstanceType::Input(..) | InstanceType::Tie(_) | InstanceType::Clock => {
                OutputSlice::new(&INPUT_GATE_OUTPUT_PORTS)
            }
        }
    }
    pub(super) fn input_ports<'nl>(
        &self,
        netlist: &'nl impl ModList,
    ) -> &'nl InputSlice<ConnectionId> {
        const INPUT_GATE_INPUT_PORTS: &InputSlice<ConnectionId> = InputSlice::from_slice(&[]);
        match self {
            InstanceType::Gate(gate) => gate.input_ports(),
            InstanceType::Module(module_id) => netlist.module(*module_id).input_ports.as_slice(),
            InstanceType::Input(..) | InstanceType::Tie(_) | InstanceType::Clock => {
                INPUT_GATE_INPUT_PORTS
            }
        }
    }

    pub(super) fn comb_deps(
        &self,
        output: ConnectionId,
        netlist: &impl ModList,
    ) -> Result<std::borrow::Cow<[ConnectionId]>> {
        Ok(match self {
            InstanceType::Gate(gate) => {
                assert_eq!(gate.output_ports(), &[output]);
                gate.comb_deps().into()
            }
            InstanceType::Module(module_id) => {
                let module = netlist.module(*module_id);
                netlist
                    .comb_input_deps(*module_id, output)
                    .iter()
                    .map(|input_id| module.input_ports[*input_id])
                    .collect::<Vec<_>>()
                    .into()
            }
            InstanceType::Input(..) | InstanceType::Tie(_) | InstanceType::Clock => {
                [].as_slice().into()
            }
        })
    }
}
impl Instance {
    pub(super) fn new_clock(wire: WireId) -> Self {
        Self {
            name: "clock".to_owned(),
            architecture: InstanceType::Clock,
            connections: ConnectionVec::from_vec(vec![wire]),
        }
    }
    pub(super) fn from_cell(
        cell: &yosys::Cell,
        name: &str,
        netlist: &impl ModList,
    ) -> Result<Self> {
        let architecture = InstanceType::from_cell(cell, netlist)?;
        let connections = match architecture {
            InstanceType::Gate(gate) => yosys_ext::cell_connection_wires(cell, gate.connections())?,
            InstanceType::Module(module_id) => {
                yosys_ext::cell_connection_wires(cell, &netlist.module(module_id).ports)?
            }
            InstanceType::Input(..) | InstanceType::Tie(_) | InstanceType::Clock => unreachable!(),
        };
        Ok(Instance {
            name: name.to_owned(),
            architecture,
            connections,
        })
    }
    pub(super) fn from_input_of(
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

    pub(super) fn clock_connection(
        &self,
        yosys_module: &yosys::Module,
        netlist: &impl ModList,
    ) -> Option<WireId> {
        let clock_name = match &self.architecture {
            InstanceType::Module(module_id) => netlist
                .module(*module_id)
                .clock
                .as_ref()
                .map(WireName::as_name_ref),
            InstanceType::Gate(gate) => gate.clock(),
            InstanceType::Input(..) | InstanceType::Tie(_) | InstanceType::Clock => None,
        };
        clock_name.map(|clock_name| {
            yosys_ext::cell_connection_wire(&yosys_module.cells[&self.name], clock_name).unwrap()
        })
    }
}
