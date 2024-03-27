use super::gadget::PipelineGadget;
use super::gadget::TopGadget;
use super::module::{Module, ModuleCombDeps};
use super::{ModuleId, ModuleVec};
use fnv::FnvHashMap as HashMap;
use yosys_netlist_json as yosys;

use anyhow::{bail, Context, Result};

pub trait ModList {
    fn module(&self, module_id: ModuleId) -> &Module;
    fn module_comb_deps(&self, module_id: ModuleId) -> &ModuleCombDeps;
    fn id_of(&self, module_name: impl AsRef<str>) -> Option<ModuleId>;
    fn gadget(&self, module_id: ModuleId) -> Option<&PipelineGadget>;
    fn comb_input_deps(
        &self,
        module_id: ModuleId,
        connection: super::module::ConnectionId,
    ) -> &[super::module::InputId] {
        self.module_comb_deps(module_id).comb_input_deps
            [self.module(module_id).connection_wires[connection]]
            .as_slice()
    }
}

impl ModList for Netlist {
    fn module(&self, module_id: ModuleId) -> &Module {
        &self.modules[module_id]
    }
    fn module_comb_deps(&self, module_id: ModuleId) -> &ModuleCombDeps {
        &self.module_comb_deps[module_id]
    }
    fn id_of(&self, module_name: impl AsRef<str>) -> Option<ModuleId> {
        self.names.get(module_name.as_ref()).copied()
    }
    fn gadget(&self, module_id: ModuleId) -> Option<&PipelineGadget> {
        self.gadgets[module_id].as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct Netlist {
    modules: ModuleVec<Module>,
    module_comb_deps: ModuleVec<ModuleCombDeps>,
    gadgets: ModuleVec<Option<PipelineGadget>>,
    names: HashMap<String, ModuleId>,
    pub top_gadget: TopGadget,
}

impl Netlist {
    pub fn new(netlist: &yosys::Netlist, top_gadget: &str) -> Result<Self> {
        let builder = super::module::ModListBuilder::new(netlist)?;
        let Some(top_gadget_id) = builder.id_of(top_gadget) else {
            bail!("Top module {top_gadget} not found in the netlist.");
        };
        let top_gadget =
            TopGadget::new(builder.module(top_gadget_id), netlist).with_context(|| {
                format!("Couln't build top-level gadget representation for {top_gadget}")
            })?;
        Ok(Netlist {
            gadgets: builder.gadgets,
            modules: builder.modules,
            module_comb_deps: builder.module_comb_deps,
            names: builder.names,
            top_gadget,
        })
    }
    pub fn gadget(&self, module_id: ModuleId) -> Option<&PipelineGadget> {
        self.gadgets[module_id].as_ref()
    }
    // TODO: build "packed modules" that include Module, ModuleCombDeps and Option<PipelineGadget>.
}
