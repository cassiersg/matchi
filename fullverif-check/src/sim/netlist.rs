#![allow(dead_code)]
use super::fv_cells::Gate;
use super::gadget::StaticGadget;
use super::module::Module;
use super::{ModuleId, ModuleVec};
use fnv::FnvHashMap as HashMap;
use yosys_netlist_json as yosys;

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Netlist {
    modules: ModuleVec<Module>,
    gadgets: ModuleVec<Option<StaticGadget>>,
    names: HashMap<String, ModuleId>,
}

// FIXME check everything uses the same clock.

impl std::convert::TryFrom<&yosys::Netlist> for Netlist {
    type Error = anyhow::Error;
    fn try_from(netlist: &yosys::Netlist) -> Result<Self> {
        let netlist_modules = ModuleVec::from_vec(sort_modules(netlist)?);
        let names = netlist_modules
            .iter_enumerated()
            .map(|(module_id, name)| ((*name).to_owned(), module_id))
            .collect::<HashMap<_, _>>();
        dbg!(names.keys().collect::<Vec<_>>());
        let mut res = Netlist {
            modules: ModuleVec::with_capacity(names.len()),
            gadgets: ModuleVec::with_capacity(names.len()),
            names,
        };
        for (module_id, name) in netlist_modules.iter_enumerated() {
            let module = Module::from_yosys(&netlist.modules[*name], module_id, name, &res)
                .with_context(|| format!("Building netlist for module {}", name))?;
            res.modules.push(module);
            let gadget = StaticGadget::new(&res[module_id], &res, netlist)
                .with_context(|| format!("Building gadget for module {}", name))?;
            if gadget.as_ref().is_some_and(|gadget| gadget.is_pipeline()) {
                res.modules[module_id].update_pipeline_gadget_deps(&gadget.as_ref().unwrap());
            }
            res.gadgets.push(gadget);
        }
        Ok(res)
    }
}

impl Netlist {
    pub fn get(&self, module_name: impl AsRef<str>) -> Option<&Module> {
        self.id_of(module_name).map(|module_id| &self[module_id])
    }
    pub fn id_of(&self, module_name: impl AsRef<str>) -> Option<ModuleId> {
        self.names.get(module_name.as_ref()).copied()
    }
    pub fn modules(&self) -> &ModuleVec<Module> {
        &self.modules
    }
    pub fn gadget(&self, module_id: ModuleId) -> Option<&StaticGadget> {
        self.gadgets[module_id].as_ref()
    }
}

impl std::ops::Index<ModuleId> for Netlist {
    type Output = Module;
    fn index(&self, index: ModuleId) -> &Self::Output {
        &self.modules[index]
    }
}

fn sort_modules(yosys_netlist: &yosys::Netlist) -> Result<Vec<&str>> {
    let mut graph = petgraph::Graph::new();
    let name2id = yosys_netlist
        .modules
        .keys()
        // Exclude Gates from the module list.
        .filter(|name| !Gate::is_gate(name))
        .map(|name| (name, graph.add_node(name)))
        .collect::<HashMap<_, _>>();
    for module_name in name2id.keys() {
        for (cell_name, cell) in yosys_netlist.modules[*module_name].cells.iter() {
            if !Gate::is_gate(&cell.cell_type) {
                if yosys_netlist.modules.contains_key(&cell.cell_type) {
                    graph.add_edge(name2id[&cell.cell_type], name2id[module_name], ());
                } else {
                    bail!(
                    "Cell {} in module {} has type {}, which is not a module in the netlist nor a library gate.",
                    cell_name, module_name, cell.cell_type
                );
                }
            }
        }
    }
    Ok(petgraph::algo::toposort(&graph, None)
        .map_err(|cycle| {
            anyhow!(
                "Netlist contains recursive module instantiation: {}.",
                graph[cycle.node_id()]
            )
        })?
        .into_iter()
        .map(|node_id| graph[node_id].as_str())
        .collect())
}

/*
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
*/
