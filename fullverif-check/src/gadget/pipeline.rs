use super::{Latency, PortRole, RndPortVec};
use crate::module::{self, ConnectionVec, InputId, InputVec, OutputVec};
use crate::ModuleId;
use crate::share_set::ShareId;

use super::yosys_ext;
use anyhow::{bail, Result};

use yosys_netlist_json as yosys;

/// Gadget with a strictly pipeline structure (no control-dependent latency, etc.).
/// Each input/output wire has therefore a single latency (exception being the clock).
/// Inputs can be share/random/control.
/// Output wires are shares.
/// Control inputs must be glitch-deterministic.
/// Random inputs cannot be sensitive.
/// If any input is sensitive, the randoms must be fresh.
#[derive(Clone, Debug)]
pub struct PipelineGadget {
    pub module_id: ModuleId,
    /// Roles of the input wires. Output wires are all shares.
    pub input_roles: InputVec<PortRole>,
    pub output_share_id: OutputVec<ShareId>,
    /// Latency associated to each connection wire (including control, excluding clock).
    pub latency: ConnectionVec<Latency>,
    pub max_latency: Latency,
    pub max_input_latency: Latency,
    /// Security property
    pub prop: super::GadgetProp,
    /// Strategy to be used to prove the security
    pub strat: super::GadgetStrat,
    /// Number of shares
    pub nshares: u32,
    /// randomness ports
    pub rnd_ports: RndPortVec<InputId>,
}

impl PipelineGadget {
    pub fn new(module: &module::Module, yosys_netlist: &yosys::Netlist) -> Result<Option<Self>> {
        let yosys_module = &yosys_netlist.modules[&module.name];
        let Some(builder) = super::gadget_builder::GadgetBuilder::new(module, yosys_module)? else {
            return Ok(None);
        };
        if builder.gadget_attrs.arch != super::GadgetArch::Pipeline {
            return Ok(None);
        }
        let input_roles = builder.input_roles()?;
        let output_share_id = builder
            .output_roles()?
            .iter_enumerated()
            .map(|(output_id, port)| match port {
                PortRole::Share(share_id) => Ok(*share_id),
                PortRole::Random(_) => unreachable!(),
                PortRole::Control => {
                    // TODO: should we allow this control output ports in pipeline gadgets ?
                    bail!(
                        "Pipeline gadget output ports must be shares, {} isn't one.",
                        module.ports[module.output_ports[output_id]].name()
                    );
                }
            })
            .collect::<Result<OutputVec<_>>>()?;
        builder.check_clock(&module.clock)?;
        let latency: ConnectionVec<_> = module
            .ports
            .iter()
            .map(|wire_name| yosys_ext::wire_latency(yosys_module, wire_name.name()))
            .collect::<Result<_>>()?;
        let max_latency = latency
            .iter()
            .copied()
            .max()
            .unwrap_or(Latency::from_raw(0));
        let max_input_latency = module
            .input_ports
            .iter()
            .map(|con_id| latency[*con_id])
            .max()
            .unwrap_or(Latency::from_raw(0));
        let rnd_ports = input_roles
            .iter_enumerated()
            .filter_map(|(id, input)| matches!(*input, PortRole::Random(_)).then_some(id))
            .collect::<RndPortVec<_>>();
        Ok(Some(Self {
            module_id: module.id,
            input_roles,
            output_share_id,
            rnd_ports,
            latency,
            prop: builder.gadget_attrs.prop,
            strat: builder.gadget_attrs.strat,
            nshares: builder.gadget_attrs.nshares,
            max_latency,
            max_input_latency,
        }))
    }
}
