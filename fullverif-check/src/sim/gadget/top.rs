use super::{Latency, PortRole, RndPortId, RndPortVec};
use crate::sim::module::{self, ConnectionVec, InputId, InputVec, WireName};
use crate::sim::{ModuleId, Netlist};
use crate::type_utils::new_id;
use fnv::FnvHashMap as HashMap;

use super::yosys_ext;
use anyhow::{anyhow, bail, Context, Result};

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
    /// Security property
    pub prop: super::GadgetProp,
    /// Strategy to be used to prove the security
    pub strat: super::GadgetStrat,
    /// Number of shares
    pub nshares: u32,
    /// randomness ports
    pub rnd_ports: RndPortVec<InputId>,
    /// Active-high signal denoting active execution.
    pub exec_active: ActiveWireId,
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
    ) -> Result<Option<Self>> {
        let lat_cond = yosys_ext::get_str_wire_attr(yosys_module, netname, "fv_latcond")?;
        let lat = yosys_ext::get_int_wire_attr(yosys_module, netname, "fv_lat")?;
        match (lat_cond, lat) {
            (Some("always"), _) => Ok(Some(Self::Always)),
            (Some("never"), _) => Ok(Some(Self::Never)),
            (Some("on_active"), _) => {
                let refsig =
                    yosys_ext::get_str_wire_attr_needed(yosys_module, netname, "fv_refsig")?;
                Ok(Some(Self::OnActive(SimSignal(
                    awbuilder.add_wire(refsig.to_owned()),
                ))))
            }
            (Some("fixed"), Some(lat)) | (None, Some(lat)) => {
                Ok(Some(Self::Lats(vec![Latency::from_raw(lat)])))
            }
            (Some("fixed"), None) => {
                bail!("Missing attribute 'fv_lat' on wire {}", netname);
            }
            (Some(lat_cond), _) => {
                bail!(
                    "Unknown latency condition '{}' for wire {}.",
                    lat_cond,
                    netname
                );
            }
            (None, None) => Ok(None),
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
        let mut awbuilder = ActiveWireBuilder::default();
        let latency = std::iter::zip(&module.ports, &port_roles)
            .map(|(wire_name, port_role)| {
                let cond = LatencyCondition::new(yosys_module, wire_name.name(), &mut awbuilder)?;
                if cond.is_none() && matches!(port_role, PortRole::Share(_) | PortRole::Random(_)) {
                    bail!(
                        "Missing 'fv_latcond' attribute for wire {}.",
                        wire_name.name()
                    );
                }
                Ok(cond)
            })
            .collect::<Result<ConnectionVec<_>>>()?;
        let exec_active = awbuilder.add_wire(
            builder
                .gadget_attrs
                .exec_active
                .ok_or_else(|| anyhow!("Missing 'fv_active' annotation on top-level module."))?,
        );
        Ok(Self {
            module_id: module.id,
            port_roles,
            latency,
            rnd_ports,
            prop: builder.gadget_attrs.prop,
            strat: builder.gadget_attrs.strat,
            nshares: builder.gadget_attrs.nshares,
            exec_active,
            active_wires: awbuilder.active_wires,
        })
    }
}
