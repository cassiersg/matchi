use super::{OutputVec, PortRole, RndPortId};
use crate::sim::module::{self, ConnectionId, ConnectionVec, InputVec, WireName};

use super::yosys_ext;
use anyhow::{bail, Result};

use yosys_netlist_json as yosys;

#[derive(Debug, Clone)]
pub struct GadgetBuilder<'a> {
    module: &'a module::Module,
    pub gadget_attrs: yosys_ext::GadgetAttrs,
    port_kinds: yosys_ext::ModulePortKinds<'a>,
}

impl<'a> GadgetBuilder<'a> {
    pub fn new(
        module: &'a module::Module,
        yosys_module: &'a yosys::Module,
    ) -> Result<Option<Self>> {
        let Some(gadget_attrs) = yosys_ext::GadgetAttrs::new(yosys_module)? else {
            return Ok(None);
        };
        let port_kinds = yosys_ext::ModulePortKinds::new(yosys_module, gadget_attrs.nshares)?;
        Ok(Some(Self {
            module,
            gadget_attrs,
            port_kinds,
        }))
    }
    fn con2port(&self, con_id: ConnectionId, next_rnd_id: &mut RndPortId) -> Result<PortRole> {
        let wire_name = &self.module.ports[con_id];
        if self.port_kinds.is_clock(wire_name.name()) {
            bail!(
                "Port {} is annotated as clock, but is not detected as a clock of a DFF or gadget.",
                wire_name.name()
            );
        } else if self.port_kinds.is_share(wire_name.name()) {
            Ok(PortRole::Share(
                self.port_kinds
                    .share_id(wire_name.name(), wire_name.offset as u32),
            ))
        } else if self.port_kinds.is_random(wire_name.name()) {
            if self.module.port_is_input[con_id] {
                let id = *next_rnd_id;
                *next_rnd_id += 1;
                Ok(PortRole::Random(id))
            } else {
                bail!("Output ports cannot be randoms (port {})", wire_name);
            }
        } else {
            assert!(self.port_kinds.is_control(wire_name.name()));
            Ok(PortRole::Control)
        }
    }
    pub fn input_roles(&self) -> Result<InputVec<PortRole>> {
        let mut next_rnd_id = RndPortId::from_usize(0);
        self.module
            .input_ports
            .iter()
            .map(|con_id| self.con2port(*con_id, &mut next_rnd_id))
            .collect()
    }
    pub fn output_roles(&self) -> Result<OutputVec<PortRole>> {
        self.module
            .output_ports
            .iter()
            .map(|con_id| self.con2port(*con_id, &mut RndPortId::from_usize(0)))
            .collect()
    }
    pub fn port_roles(&self) -> Result<ConnectionVec<PortRole>> {
        let mut next_rnd_id = RndPortId::from_usize(0);
        self.module
            .ports
            .indices()
            .map(|con_id| self.con2port(con_id, &mut next_rnd_id))
            .collect()
    }
    pub fn check_clock(&self, wire_name: &Option<WireName>) -> Result<()> {
        if let Some(clock_name) = wire_name {
            if !self.port_kinds.is_clock(clock_name.name()) {
                bail!(
                    "Inferred clock port {} is not annotated as clock.",
                    clock_name.name()
                );
            }
        }
        Ok(())
    }
}
