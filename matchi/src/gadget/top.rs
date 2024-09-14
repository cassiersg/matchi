use super::{GadgetArch, GadgetProp, GadgetStrat, Latency, PortRole, RndPortVec};
use crate::module::{self, ConnectionVec, InputId, WireName};
use crate::type_utils::new_id;
use crate::ModuleId;
use fnv::FnvHashMap as HashMap;

use super::yosys_ext;
use anyhow::{bail, Result};

use yosys_netlist_json as yosys;

// Wires in the gadget that are used for input/output lat and exec_active.
new_id!(ActiveWireId, ActiveWireVec, ActiveWireSlice);

/// Top-level gadget.
/// Each input wire is either share, random or control (or clock).
/// TODO output wires.
#[derive(Clone, Debug)]
pub struct TopGadget {
    pub module_id: ModuleId,
    /// Roles of the input wires. Output wires are all shares.
    pub port_roles: ConnectionVec<PortRole>,
    /// Latency associated to each connection wire (including control, excluding clock).
    pub latency: ConnectionVec<Option<LatencyCondition>>,
    /// Number of shares
    pub nshares: u32,
    /// randomness ports
    pub rnd_ports: RndPortVec<InputId>,
    /// Active-high signal denoting active execution.
    pub exec_active: Option<ActiveWireId>,
    /// Map of ActiveWireId to the corresponding WireName.
    pub active_wires: ActiveWireVec<WireName>,
}

/// TODO: this should be a ref to a signal found in the top-level simulation (or can it be a
/// "control" signal ?).
#[derive(Debug, Clone)]
pub struct SimSignal(pub ActiveWireId);

#[derive(Debug, Clone)]
pub enum LatencyCondition {
    Always,
    Never,
    Lats(Vec<Latency>),
    OnActive(SimSignal),
}

#[derive(Debug, Default)]
struct ActiveWireBuilder {
    active_wires: ActiveWireVec<WireName>,
    name_map: HashMap<String, ActiveWireId>,
}

impl ActiveWireBuilder {
    fn add_wire(&mut self, wire: String) -> ActiveWireId {
        *self
            .name_map
            .entry(wire.clone())
            .or_insert_with(|| self.active_wires.push(WireName::single_port(wire)))
    }
}

impl LatencyCondition {
    fn new(
        yosys_module: &yosys::Module,
        netname: &str,
        awbuilder: &mut ActiveWireBuilder,
        allow_relative_lat: bool,
    ) -> Result<Option<Self>> {
        let lat = yosys_ext::get_int_wire_attr(yosys_module, netname, "matchi_lat")?;
        let active = yosys_ext::get_str_wire_attr(yosys_module, netname, "matchi_active")?;
        match (active, lat, allow_relative_lat) {
            (Some("1"), None, _) => Ok(Some(Self::Always)),
            (Some("0"), None, _) => Ok(Some(Self::Never)),
            (Some(refsig), None, _) => Ok(Some(Self::OnActive(SimSignal(
                awbuilder.add_wire(refsig.to_owned()),
            )))),
            (None, Some(lat), true) => Ok(Some(Self::Lats(vec![Latency::from_raw(lat)]))),
            (None, Some(_), false) => {
                bail!("'matchi_lat' annotation given on wire {}, but not gadget-level 'matchi_active' is given.", netname);
            }
            (Some(_), Some(_), _) => {
                bail!(
                    "Conflicting 'matchi_lat' and 'matchi_active' annotations on wire {}",
                    netname
                );
            }
            (None, None, _) => Ok(None),
        }
    }
}

impl TopGadget {
    pub fn new(module: &module::Module, yosys_netlist: &yosys::Netlist) -> Result<Self> {
        let yosys_module = &yosys_netlist.modules[&module.name];
        let Some(builder) = super::gadget_builder::GadgetBuilder::new(module, yosys_module)? else {
            bail!(
                "Top-level module {} has no module-level gadget annotations.",
                module.name
            );
        };
        let port_roles = builder.port_roles()?;
        builder.check_clock(&module.clock)?;
        let mut rnd_ports = module
            .input_ports
            .iter_enumerated()
            .filter_map(|(input_id, con_id)| {
                if let PortRole::Random(rnd_id) = port_roles[*con_id] {
                    Some((rnd_id, input_id))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        rnd_ports.sort_unstable();
        let rnd_ports = rnd_ports
            .into_iter()
            .map(|(_, input_id)| input_id)
            .collect::<RndPortVec<_>>();
        for (id, port) in rnd_ports.iter_enumerated() {
            eprintln!(
                "rnd port id: {:?} name: {}",
                id, module.ports[module.input_ports[*port]]
            );
        }
        let mut awbuilder = ActiveWireBuilder::default();
        let exec_active = builder
            .gadget_attrs
            .exec_active
            .map(|aw| awbuilder.add_wire(aw));
        let latency = std::iter::zip(&module.ports, &port_roles)
            .map(|(wire_name, port_role)| {
                let cond = LatencyCondition::new(
                    yosys_module,
                    wire_name.name(),
                    &mut awbuilder,
                    exec_active.is_some(),
                )?;
                if cond.is_none() && matches!(port_role, PortRole::Share(_) | PortRole::Random(_)) {
                    bail!(
                        "Missing active information for share or randomness wire {}.",
                        wire_name.name()
                    );
                }
                Ok(cond)
            })
            .collect::<Result<ConnectionVec<_>>>()?;
        if builder.gadget_attrs.strat != GadgetStrat::CompositeTop {
            bail!("Top-level gadget must have 'composite_top' verification strategy.");
        }
        if builder.gadget_attrs.arch != GadgetArch::Loopy {
            bail!("Cannot verify 'pipeline' top-level gadgets.");
        }
        if builder.gadget_attrs.prop != GadgetProp::Pini {
            bail!("Cannot verify non-'PINI' top-level gadgets.");
        }
        Ok(Self {
            module_id: module.id,
            port_roles,
            latency,
            rnd_ports,
            nshares: builder.gadget_attrs.nshares,
            exec_active,
            active_wires: awbuilder.active_wires,
        })
    }
}
