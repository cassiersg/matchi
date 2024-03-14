use super::yosys_ext;
use super::{
    ConnectionSet, ConnectionVec, InputId, InputVec, Instance, InstanceId, InstanceType,
    InstanceVec, Module, ModuleCombDeps, OutputId, OutputVec, WireGraph, WireId, WireProperties,
    WireVec,
};
use crate::fv_cells::Gate;
use crate::gadget::PipelineGadget;
use crate::netlist::ModList;
use crate::{ModuleId, ModuleVec};
use anyhow::{anyhow, bail, Context, Result};
use fnv::FnvHashMap as HashMap;
use std::collections::BTreeSet;
use yosys_netlist_json as yosys;

#[derive(Debug, Clone)]
pub struct ModListBuilder {
    pub modules: ModuleVec<Module>,
    pub names: HashMap<String, ModuleId>,
    pub module_comb_deps: ModuleVec<ModuleCombDeps>,
    pub gadgets: ModuleVec<Option<PipelineGadget>>,
}

impl ModList for ModListBuilder {
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

impl ModListBuilder {
    fn empty_from_names(names: HashMap<String, ModuleId>) -> Self {
        Self {
            modules: ModuleVec::with_capacity(names.len()),
            module_comb_deps: ModuleVec::with_capacity(names.len()),
            gadgets: ModuleVec::with_capacity(names.len()),
            names,
        }
    }
    pub fn new(netlist: &yosys::Netlist) -> Result<Self> {
        let netlist_modules = ModuleVec::from_vec(sort_modules(netlist)?);
        let mut res = Self::empty_from_names(
            netlist_modules
                .iter_enumerated()
                .map(|(module_id, name)| ((*name).to_owned(), module_id))
                .collect(),
        );
        for (module_id, name) in netlist_modules.iter_enumerated() {
            let module = Module::from_yosys(&netlist.modules[*name], module_id, name, &res)
                .with_context(|| format!("Could not build netlist for module {}", name))?;
            res.modules.push(module);
            let module = res.module(module_id);
            let gadget = PipelineGadget::new(module, netlist)
                .with_context(|| format!("Could not build gadget for module {}", module.name))?;
            res.gadgets.push(gadget);
            let mut comb_deps = ModuleCombDeps::new(module_id, &res)?;
            let gadget = res.gadget(module_id);
            if let Some(gadget) = gadget {
                comb_deps.update_pipeline_gadget_deps(gadget, &res);
            }
            res.module_comb_deps.push(comb_deps);
        }
        Ok(res)
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
impl Module {
    pub fn from_yosys(
        yosys_module: &yosys::Module,
        id: ModuleId,
        name: &str,
        modlist: &ModListBuilder,
    ) -> Result<Self> {
        let module_instances = yosys_ext::module_instances(yosys_module, modlist)?;
        let clock_wire = module_clock_wire(module_instances.as_slice(), yosys_module, modlist)?;
        let (ports, clock) = yosys_ext::ports(yosys_module, clock_wire);
        let input_ports = InputVec::from_vec(yosys_ext::filter_ports(
            yosys_module,
            &ports,
            yosys::PortDirection::Input,
        ));
        let output_ports = OutputVec::from_vec(yosys_ext::filter_ports(
            yosys_module,
            &ports,
            yosys::PortDirection::Output,
        ));
        let inout_ports =
            yosys_ext::filter_ports(yosys_module, &ports, yosys::PortDirection::InOut);
        if !inout_ports.is_empty() {
            bail!("Gadgets cannot have inout ports.");
        }
        let instances: InstanceVec<Instance> = yosys_ext::tie_instances()
            .map(Ok)
            .chain(clock_wire.iter().map(|wire| Ok(Instance::new_clock(*wire))))
            .chain(yosys_ext::input_instances(
                yosys_module,
                &ports,
                &input_ports,
            ))
            .chain(module_instances.into_iter().map(Ok))
            .collect::<Result<_>>()?;
        let n_wires = yosys_ext::count_wires(yosys_module);
        let connection_wires = yosys_ext::connection_wires(yosys_module, &ports)?;
        let port_is_input = yosys_ext::ports_is_input(yosys_module, &ports);
        let wires_output_connection =
            yosys_ext::wires_output_connection(yosys_module, n_wires, &output_ports, &ports)?;
        let wires_source = wires_source(n_wires, &instances, modlist)?;
        let wire_sinks = wires_sinks(n_wires, &instances, modlist);
        let wires = itertools::izip!(wires_source, wires_output_connection, wire_sinks)
            .map(|(source, output, sinks)| WireProperties {
                source,
                output,
                sinks,
            })
            .collect::<WireVec<_>>();
        let wire_names = yosys_ext::wire_names(yosys_module, &wires);
        Ok(Module {
            id,
            name: name.to_owned(),
            clock,
            instances,
            wires,
            connection_wires,
            port_is_input,
            ports,
            input_ports,
            output_ports,
            wire_names,
        })
    }
}

impl ModuleCombDeps {
    pub fn new(module_id: ModuleId, modlist: &impl ModList) -> Result<Self> {
        let module = modlist.module(module_id);
        //eprintln!("Sorting wires for module {}", module.name);
        let comb_wire_dag = comb_wire_dag(&module.instances, &module.wires, modlist)?;
        /*
        std::fs::write(
            format!("wg_{}.dot", name),
            format!(
                "{:?}",
                petgraph::dot::Dot::with_config(
                    &comb_wire_dag.graph,
                    &[petgraph::dot::Config::EdgeNoLabel]
                )
            )
            .as_bytes(),
        )
        .unwrap();
        */
        let eval_sorted_wires = sort_wires(&comb_wire_dag)?;
        let comb_depsets = comb_depsets(
            &module.instances,
            &module.wires,
            &eval_sorted_wires,
            modlist,
        )?;
        let comb_input_deps = comb_input_deps(
            &comb_depsets,
            &module.connection_wires,
            &module.wires,
            &module.instances,
        );
        Ok(Self {
            module_id,
            comb_input_deps,
            comb_wire_dag,
        })
    }
}

fn module_clock_wire(
    instances: &[Instance],
    yosys_module: &yosys::Module,
    modlist: &ModListBuilder,
) -> Result<Option<WireId>> {
    let mut clocks = instances
        .iter()
        .flat_map(|instance| instance.clock_connection(yosys_module, modlist))
        .collect::<BTreeSet<_>>();
    if clocks.len() > 1 {
        bail!(
            "Cannot have more than one clock signal (clock signals: {:?})",
            clocks
        );
    }
    Ok(clocks.pop_first())
}
fn wires_source(
    n_wires: usize,
    instances: &InstanceVec<Instance>,
    netlist: &impl ModList,
) -> Result<WireVec<(InstanceId, OutputId)>> {
    let mut sources = WireVec::<Option<(InstanceId, OutputId)>>::from_vec(vec![None; n_wires]);
    for (instance_id, instance) in instances.iter_enumerated() {
        for (output_id, output_port) in instance
            .architecture
            .output_ports(netlist)
            .iter_enumerated()
        {
            let wire = instance.connections[*output_port];
            if let Some(other_instance) = sources[wire] {
                bail!(
                    "Wire {} is an output of both {} and {}.",
                    wire,
                    instances[other_instance.0].name,
                    instance.name
                );
            } else {
                sources[wire] = Some((instance_id, output_id));
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

fn wires_sinks(
    n_wires: usize,
    instances: &InstanceVec<Instance>,
    netlist: &impl ModList,
) -> WireVec<Vec<(InstanceId, InputId)>> {
    let mut res = WireVec::from_vec(vec![vec![]; n_wires]);
    for (instance_id, instance) in instances.iter_enumerated() {
        for (input_id, con_id) in instance.architecture.input_ports(netlist).iter_enumerated() {
            let wire_id = instance.connections[*con_id];
            res[wire_id].push((instance_id, input_id));
        }
    }
    res
}
fn comb_wire_dag(
    instances: &InstanceVec<Instance>,
    wires: &WireVec<WireProperties>,
    netlist: &impl ModList,
) -> Result<WireGraph> {
    let mut graph = petgraph::Graph::new();
    let node_indices = wires
        .indices()
        .map(|wire_id| graph.add_node(wire_id))
        .collect::<WireVec<_>>();
    for instance in instances.iter() {
        //eprintln!("instance {}", instance.name);
        for output in instance.architecture.output_ports(netlist).iter() {
            //eprintln!("\t output wire: {}", instance.connections[*output]);
            for input in instance.architecture.comb_deps(*output, netlist)?.as_ref() {
                //eprintln!("\t\t input wire: {}", instance.connections[*input]);
                /*
                eprintln!(
                    "\t\tAdd edge {} -> {}",
                    instance.connections[*input], instance.connections[*output],
                );
                */
                graph.add_edge(
                    node_indices[instance.connections[*input]],
                    node_indices[instance.connections[*output]],
                    (),
                );
            }
        }
    }
    Ok(WireGraph {
        graph,
        node_indices,
    })
}

fn sort_wires(comb_wire_dag: &WireGraph) -> Result<Vec<WireId>> {
    Ok(petgraph::algo::toposort(&comb_wire_dag.graph, None)
        .map_err(|cycle| {
            anyhow!(
                "Gadget contains combinational loop involving wire {}",
                comb_wire_dag.graph[cycle.node_id()].index()
            )
        })?
        .into_iter()
        .map(|node_id| comb_wire_dag.graph[node_id])
        .collect())
}
fn comb_depsets(
    instances: &InstanceVec<Instance>,
    wires: &WireVec<WireProperties>,
    eval_sorted_wires: &[WireId],
    netlist: &impl ModList,
) -> Result<WireVec<ConnectionSet>> {
    let mut dependency_sets = WireVec::from_vec(vec![ConnectionSet::new(); wires.len()]);
    for wire_id in eval_sorted_wires {
        let (instance_id, output_id) = &wires[*wire_id].source;
        let instance = &instances[*instance_id];
        let output_con_id = instance.architecture.output_ports(netlist)[*output_id];
        //eprintln!("wire {} from instance {}", wire_id, instance.name);
        let mut dep_set = ConnectionSet::new();
        if let InstanceType::Input(_, con_id) = instance.architecture {
            dep_set.insert(con_id.index());
        } else {
            for con_input_id in instance
                .architecture
                .comb_deps(output_con_id, netlist)?
                .as_ref()
            {
                /*
                eprintln!(
                    "\tinput wire {} with depset {:?}",
                    instance.connections[*con_input_id],
                    &dependency_sets[instance.connections[*con_input_id]]
                );
                    */
                dep_set.union_with(&dependency_sets[instance.connections[*con_input_id]]);
            }
        }
        //eprintln!("\tend depset: {:?}", dep_set);
        dependency_sets[*wire_id] = dep_set;
    }
    Ok(dependency_sets)
}
fn comb_input_deps(
    comb_depsets: &WireVec<ConnectionSet>,
    connection_wires: &ConnectionVec<WireId>,
    wires: &WireVec<WireProperties>,
    instances: &InstanceVec<Instance>,
) -> WireVec<Vec<InputId>> {
    comb_depsets
        .iter()
        .map(|depset| {
            depset
                .iter()
                .filter_map(|con_id| {
                    if let InstanceType::Input(input_id, _) =
                        instances[wires[connection_wires[con_id]].source.0].architecture
                    {
                        Some(input_id)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect()
}
