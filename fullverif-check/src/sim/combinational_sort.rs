impl CompositeGadget {
    pub fn sort_wires(&self, library: &GadgetLibrary) -> Result<Vec<WireId>> {
        let mut wire_graph = petgraph::Graph::new();
        let node_indices = self
            .wires
            .indices()
            .map(|wire_id| wire_graph.add_node(wire_id))
            .collect::<WireVec<_>>();
        for instance in self.instances.iter() {
            for output in instance.architecture.output_ports(library).iter() {
                for input in instance
                    .architecture
                    .combinational_dependencies(*output, library)?
                    .as_ref()
                {
                    wire_graph.add_edge(
                        node_indices[instance.connections[*input]],
                        node_indices[instance.connections[*output]],
                        (),
                    );
                }
            }
        }
        Ok(petgraph::algo::toposort(&wire_graph, None)
            .map_err(|cycle| {
                anyhow!(
                    "Gadget contains combinational loop involving wire {}",
                    wire_graph[cycle.node_id()].index()
                )
            })?
            .into_iter()
            .map(|node_id| wire_graph[node_id])
            .collect())
    }

    fn combinational_input_dependencies(
        &self,
        connection: ConnectionId,
        library: &GadgetLibrary,
    ) -> Result<Vec<ConnectionId>> {
        let Some(wire) = self.connection_wires[connection] else {
            return Ok(vec![]);
        };
        let depsets = self.combinational_depsets(library)?;
        let gadget = &library.gadgets[self.gadget_id];
        let depset = &depsets[wire];
        Ok(gadget
            .input_ports
            .iter()
            .copied()
            .filter(|input_conid| depset.contains(input_conid.index()))
            .collect())
    }
    fn combinational_depsets(&self, library: &GadgetLibrary) -> Result<WireVec<ConnectionSet>> {
        // TODO store sorted_wires and combinational_dependencies.
        let sorted_wires = self.sort_wires(library)?;
        let mut dependency_sets = WireVec::from_vec(vec![ConnectionSet::new(); self.wires.len()]);
        for wire_id in &sorted_wires {
            let (instance_id, output_id) = &self.wires[*wire_id].source;
            let instance = &self.instances[*instance_id];
            let mut dep_set = ConnectionSet::new();
            for input_id in instance
                .architecture
                .combinational_dependencies(*output_id, library)?
                .as_ref()
            {
                dep_set.union_with(&dependency_sets[instance.connections[*input_id]]);
            }
            dependency_sets[*wire_id] = dep_set;
        }
        Ok(dependency_sets)
    }
}

fn connection_set2ids(set: &ConnectionSet) -> Vec<ConnectionId> {
    set.iter().map(ConnectionId::from_usize).collect()
}
