use super::{Latency, PortRole, RndPortId, RndPortVec};
use crate::sim::module::{self, ConnectionVec, InputId, InputVec, WireName};
use crate::sim::{ModuleId, Netlist};
use fnv::FnvHashMap as HashMap;

use super::yosys_ext;
use anyhow::{anyhow, bail, Context, Result};

use yosys_netlist_json as yosys;

/// Top-level gadget.
/// Each input wire is either share, random or control (or clock).
/// TODO output wires.
#[derive(Clone, Debug)]
pub struct TopGadget {
    pub module_id: ModuleId,
    /// Roles of the input wires. Output wires are all shares.
    pub input_roles: InputVec<PortRole>,
    /// Latency associated to each connection wire (including control, excluding clock).
    pub latency: ConnectionVec<LatencyCondition>,
    /// Security property
    pub prop: super::GadgetProp,
    /// Strategy to be used to prove the security
    pub strat: super::GadgetStrat,
    /// Number of shares
    pub nshares: u32,
    /// randomness ports
    pub rnd_ports: RndPortVec<InputId>,
}

/// TODO: this should be a ref to a signal found in the top-level simulation (or can it be a
/// "control" signal ?).
#[derive(Debug, Clone)]
pub struct SimSignal(String);

#[derive(Debug, Clone)]
pub enum LatencyCondition {
    Always,
    Never,
    Lats(Vec<Latency>),
    OnActive(SimSignal),
}

impl LatencyCondition {
    fn new(yosys_module: &yosys::Module, netname: &str) -> Result<Self> {
        let lat_cond = yosys_ext::get_str_wire_attr(yosys_module, netname, "fv_latcond")?;
        let lat = yosys_ext::get_int_wire_attr(yosys_module, netname, "fv_lat")?;
        match (lat_cond, lat) {
            (Some("always"), _) => Ok(Self::Always),
            (Some("never"), _) => Ok(Self::Never),
            (Some("on_active"), _) => {
                let refsig =
                    yosys_ext::get_str_wire_attr_needed(yosys_module, netname, "fv_refsig")?;
                Ok(Self::OnActive(SimSignal(refsig.to_owned())))
            }
            (Some("fixed"), Some(lat)) | (None, Some(lat)) => {
                Ok(Self::Lats(vec![Latency::from_raw(lat)]))
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
            (None, None) => {
                bail!("Missing 'fv_latcond' attribute for wire {}.", netname);
            }
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
        let input_roles = builder.input_roles()?;
        builder.check_clock(&module.clock)?;
        let rnd_ports = input_roles
            .iter_enumerated()
            .filter_map(|(id, input)| matches!(*input, PortRole::Random(_)).then_some(id))
            .collect::<RndPortVec<_>>();
        // FIXME: move roles to a single vec for I/O.
        // FIXME: require latency condition only for random and shares.
        /*
        let latency = module
            .ports
            .iter()
            .map(|wire_name| LatencyCondition::new(yosys_module, wire_name.name()))
            .collect::<Result<ConnectionVec<_>>>()?;
        */
        let latency = ConnectionVec::new();
        Ok(Self {
            module_id: module.id,
            input_roles,
            latency,
            rnd_ports,
            prop: builder.gadget_attrs.prop,
            strat: builder.gadget_attrs.strat,
            nshares: builder.gadget_attrs.nshares,
        })
    }
}
