//! Internals of a composite gadgets: sub-gadgets, their connections, inputs and outputs
//! connections, connections to the randomness.

use crate::clk_vcd;
use crate::error::{CompError, CompErrorKind};
use crate::gadgets::{Gadget, Latency, Random, Sharing};
use anyhow::bail;
use anyhow::Result;
use fnv::FnvHashMap as HashMap;
use petgraph::{
    graph::NodeIndex,
    visit::{EdgeRef, IntoNodeIdentifiers, IntoNodeReferences},
    Direction, Graph,
};
use std::collections::{hash_map, BTreeSet};
use yosys_netlist_json as yosys;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Share<'a> {
    sharing: Sharing<'a>,
    share_id: u32,
}
/// Gadget input: share or random
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum GadgetInput<'a> {
    Random(Random<'a>),
    Share(Share<'a>),
}

/// Types of boolean binary gates
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolBinKind {
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    AndNot,
    OrNot,
    Nmux,
}
impl BoolBinKind {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "AND" => Some(Self::And),
            "NAND" => Some(Self::Nand),
            "OR" => Some(Self::Or),
            "NOR" => Some(Self::Nor),
            "XOR" => Some(Self::Xor),
            "XNOR" => Some(Self::Xnor),
            _ => None,
        }
    }
}

/// A raw gate: reg, mux, inv, or boolean binary
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawGate<Ctrl = yosys::BitVal> {
    Reg,
    Mux(Ctrl), // control signal
    Inv,
    Buf,
    BoolBin(BoolBinKind),
}

/// Id of a gate.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GateId<'a> {
    pub cell: &'a str,
    pub offset: u32,
}
impl<'a> GateId<'a> {
    fn new(cell: &'a str, offset: u32) -> Self {
        Self { cell, offset }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum GNode<'a, Cst = yosys::BitVal> {
    Gate(RawGate<Cst>, GateId<'a>),
    Input(GadgetInput<'a>),
    Constant(Cst),
}

#[derive(Debug, Clone, Copy)]
pub struct Edge<'a> {
    input_name: &'a str,
}
const A_EDGE: Edge<'static> = Edge { input_name: "A" };
const B_EDGE: Edge<'static> = Edge { input_name: "B" };
const D_EDGE: Edge<'static> = Edge { input_name: "D" };

#[derive(Debug, Clone)]
pub struct GadgetGates<'a, 'b> {
    pub gates: Graph<GNode<'a>, Edge<'a>>,
    pub wires: HashMap<yosys::BitVal, petgraph::graph::NodeIndex>,
    pub gate_names: HashMap<GateId<'a>, petgraph::graph::NodeIndex>,
    pub gadget: &'b Gadget<'a>,
}

#[derive(Debug, Clone)]
pub struct UnrolledGates<'a, 'b> {
    pub tgates: Graph<(GNode<'a, bool>, Latency), Edge<'a>>,
    pub gates2timed: HashMap<(NodeIndex, Latency), NodeIndex>,
    pub sorted_gates: Vec<NodeIndex>,
    pub gadget: &'b GadgetGates<'a, 'b>,
    pub n_cycles: Latency,
    pub outputs: HashMap<NodeIndex, Share<'a>>,
}

// Hacky rust "impl Trait" limitation workaround
trait LT<'a> {}
impl<'a, T> LT<'a> for T {}

impl<'a: 'b, 'b> UnrolledGates<'a, 'b> {
    fn inputs<'c>(&'c self, node: NodeIndex) -> impl Iterator<Item = NodeIndex> + LT<'c> + LT<'a>
    where
        'a: 'c,
    {
        self.tgates.neighbors_directed(node, Direction::Incoming)
    }
    fn input(&self, node: NodeIndex, name: &str) -> NodeIndex {
        self.tgates
            .edges_directed(node, Direction::Incoming)
            .find(|e| e.weight().input_name == name)
            .unwrap()
            .source()
    }
    pub fn annotate_valid(&self) -> Vec<bool> {
        let mut valid = vec![false; self.tgates.node_count()];
        for node in self.sorted_gates.iter() {
            valid[node.index()] = match self.tgates[*node].0 {
                GNode::Gate(RawGate::Reg, _)
                | GNode::Gate(RawGate::Inv, _)
                | GNode::Gate(RawGate::Buf, _)
                | GNode::Gate(RawGate::BoolBin(_), _) => self
                    .tgates
                    .neighbors_directed(*node, Direction::Incoming)
                    .all(|n| valid[n.index()]),
                GNode::Gate(RawGate::Mux(ctrl), _) => {
                    valid[self
                        .tgates
                        .edges_directed(*node, Direction::Incoming)
                        .filter(|e| e.weight().input_name == (if ctrl { "A" } else { "B" }))
                        .next()
                        .unwrap()
                        .source()
                        .index()]
                }
                GNode::Input(_) | GNode::Constant(_) => true,
            };
        }
        return valid;
    }
    pub fn annotate_sensitive(&self) -> Vec<bool> {
        let mut sensitive = vec![false; self.tgates.node_count()];
        for node in self.sorted_gates.iter() {
            sensitive[node.index()] = match self.tgates[*node].0 {
                GNode::Gate(RawGate::Reg, _)
                | GNode::Gate(RawGate::Inv, _)
                | GNode::Gate(RawGate::Buf, _)
                | GNode::Gate(RawGate::BoolBin(_), _) => self
                    .tgates
                    .neighbors_directed(*node, Direction::Incoming)
                    .any(|n| sensitive[n.index()]),
                GNode::Gate(RawGate::Mux(ctrl), _) => {
                    sensitive[self
                        .tgates
                        .edges_directed(*node, Direction::Incoming)
                        .filter(|e| e.weight().input_name == (if ctrl { "A" } else { "B" }))
                        .next()
                        .unwrap()
                        .source()
                        .index()]
                }
                GNode::Input(_) => true,
                GNode::Constant(_) => false,
            };
        }
        return sensitive;
    }
    pub fn check_state_cleared(&self, sensitive: Vec<bool>) -> Result<()> {
        for (node, g) in self.gadget.gates.node_references() {
            if let GNode::Gate(RawGate::Reg, id) = g {
                let input = self.gadget.input(node, "D");
                if let Some(n) = self.gates2timed.get(&(input, self.n_cycles - 1)) {
                    if sensitive[n.index()] {
                        bail!(
                            "DFF {}[{}] contains sensitive state past the last output",
                            id.cell,
                            id.offset
                        );
                    }
                }
            }
        }
        return Ok(());
    }
    pub fn check_outputs_valid(&self, valid: Vec<bool>) -> Result<()> {
        for (output, o_share) in self.outputs.iter() {
            if !valid[output.index()] {
                bail!(
                    "TODO format {:?}",
                    CompErrorKind::OutputNotValid(vec![(o_share.sharing, self.tgates[*output].1)]),
                );
            }
        }
        return Ok(());
    }
    // eliminate non-used constants ? -> when we output the result ? -> is_gate_useless
    fn is_gate_useless(&self, node: NodeIndex) -> bool {
        if let (GNode::Constant(_), _) = self.tgates[node] {
            self.tgates
                .neighbors_directed(node, Direction::Outgoing)
                .next()
                .is_none()
        } else {
            false
        }
    }
    fn extend_probe(&self, probe: NodeIndex) -> Vec<NodeIndex> {
        let mut res = Vec::new();
        let mut to_explore = vec![probe];
        while let Some(probe) = to_explore.pop() {
            match self.tgates[probe].0 {
                GNode::Gate(RawGate::Reg, _) | GNode::Input(_) => {
                    res.push(probe);
                }
                GNode::Gate(_, _) => {
                    for n in self.tgates.neighbors_directed(probe, Direction::Incoming) {
                        to_explore.push(n);
                    }
                }
                GNode::Constant(_) => {}
            }
        }
        return res;
    }
    pub fn computation_graph(&self, sensitive: Vec<bool>) -> LeakComputationGraph<'a> {
        let mut max_eprobes_internal = Vec::<Vec<NodeIndex>>::new();
        let mut max_eprobes_output = Vec::<Vec<NodeIndex>>::new();
        let mut outputs = Vec::new();
        for (node, _gate) in self.tgates.node_references() {
            if sensitive[node.index()] {
                // Is only used as input of regs -> won't be extended more,
                // let's put a probe on it.
                let only_reg_uses = self
                    .tgates
                    .neighbors_directed(node, Direction::Outgoing)
                    .all(|n| !matches!(self.tgates[n].0, GNode::Gate(RawGate::Reg, _)));
                let is_output = if let Some(share) = self.outputs.get(&node) {
                    outputs.push(*share);
                    true
                } else {
                    false
                };
                // TODO handle transitions
                if is_output {
                    max_eprobes_output.push(self.extend_probe(node));
                } else if only_reg_uses {
                    max_eprobes_internal.push(self.extend_probe(node));
                }
            }
        }
        let mut used_gates = vec![false; self.tgates.node_count()];
        for probe in max_eprobes_internal
            .iter()
            .chain(max_eprobes_output.iter())
            .flat_map(|p| p.iter())
        {
            used_gates[probe.index()] = true;
        }
        let sorted_tgates = petgraph::algo::toposort(&self.tgates, None).unwrap();
        for node in sorted_tgates.iter().rev() {
            if used_gates[node.index()] {
                if let (GNode::Gate(RawGate::Mux(ctrl), _), _) = self.tgates[*node] {
                    let in_edge = if ctrl { "A" } else { "B" };
                    let n = self.input(*node, in_edge);
                    used_gates[n.index()] = true;
                } else {
                    for n in self.inputs(*node) {
                        used_gates[n.index()] = true;
                    }
                }
            }
        }
        let mut mapped_gates = vec![None; self.tgates.node_count()];
        let mut cg = Graph::new();
        // For constant unification
        let mut zero = None;
        let mut one = None;
        for node in sorted_tgates.iter() {
            match self.tgates[*node] {
                (GNode::Gate(RawGate::Reg, _), _) => {
                    mapped_gates[node.index()] = mapped_gates[self.input(*node, "D").index()];
                }
                (GNode::Gate(RawGate::Mux(ctrl), _), _) => {
                    let in_edge = if ctrl { "A" } else { "B" };
                    mapped_gates[node.index()] = mapped_gates[self.input(*node, in_edge).index()];
                }
                (GNode::Gate(RawGate::Inv, id), lat) => {
                    let n = cg.add_node(CGNode::Gate(BoolGate::Not, (id, lat)));
                    cg.add_edge(mapped_gates[self.input(*node, "A").index()].unwrap(), n, ());
                    mapped_gates[node.index()] = Some(n);
                }
                (GNode::Gate(RawGate::Buf, id), lat) => {
                    let n = cg.add_node(CGNode::Gate(BoolGate::Buf, (id, lat)));
                    cg.add_edge(mapped_gates[self.input(*node, "A").index()].unwrap(), n, ());
                    mapped_gates[node.index()] = Some(n);
                }
                (GNode::Gate(RawGate::BoolBin(kind), id), lat) => {
                    let n = cg.add_node(CGNode::Gate(BoolGate::from_bin_kind(kind), (id, lat)));
                    cg.add_edge(mapped_gates[self.input(*node, "A").index()].unwrap(), n, ());
                    cg.add_edge(mapped_gates[self.input(*node, "B").index()].unwrap(), n, ());
                    mapped_gates[node.index()] = Some(n);
                }
                (GNode::Input(input), lat) => {
                    let n = cg.add_node(CGNode::Input(input, lat));
                    mapped_gates[node.index()] = Some(n);
                }
                (GNode::Constant(cst), _) => {
                    let cst_n = if cst { &mut one } else { &mut zero };
                    if cst_n.is_none() {
                        *cst_n = Some(cg.add_node(CGNode::Constant(cst)));
                    }
                    mapped_gates[node.index()] = *cst_n;
                }
            }
        }
        let mut outputs_map = HashMap::default();
        let mut e_probes_output = Vec::new();
        for (output, ep) in outputs.into_iter().zip(max_eprobes_output.into_iter()) {
            let ep = ep
                .iter()
                .map(|p| mapped_gates[p.index()].unwrap())
                .collect::<BTreeSet<_>>();
            if let Some(pos) = e_probes_output.iter().position(|p| &ep == p) {
                outputs_map.insert(output, pos);
            } else {
                outputs_map.insert(output, e_probes_output.len());
                e_probes_output.push(ep);
            }
        }
        let e_probes_output_set = e_probes_output.iter().collect::<BTreeSet<_>>();
        let e_probes_internal = max_eprobes_internal
            .iter()
            .map(|ep| {
                ep.iter()
                    .map(|p| mapped_gates[p.index()].unwrap())
                    .collect::<BTreeSet<_>>()
            })
            .filter(|ep| !e_probes_output_set.contains(ep))
            .collect::<BTreeSet<_>>();
        let e_probes_internal = e_probes_internal.into_iter().collect::<Vec<_>>();
        return LeakComputationGraph {
            cg,
            e_probes_output,
            e_probes_internal,
            outputs_map,
            n_shares: self.gadget.gadget.order,
        };
    }
}

impl<'a, 'b> GadgetGates<'a, 'b> {
    pub fn from_gadget(gadget: &'b Gadget<'a>) -> Result<Self> {
        let mut gates = petgraph::Graph::new();
        let mut wires = HashMap::default();
        let mut gate_names = HashMap::default();
        for rnd in gadget.randoms.keys() {
            let node = gates.add_node(GNode::Input(GadgetInput::Random(rnd.clone())));
            wires.insert(
                gadget.module.ports[rnd.port_name].bits[rnd.offset as usize],
                node,
            );
        }
        for sharing in gadget.inputs.keys() {
            for i in 0..gadget.order {
                let offset = sharing.pos * gadget.order + i;
                let node = gates.add_node(GNode::Input(GadgetInput::Share(Share {
                    sharing: sharing.clone(),
                    share_id: i,
                })));
                wires.insert(
                    gadget.module.ports[sharing.port_name].bits[offset as usize],
                    node,
                );
            }
        }
        let mut to_explore: Vec<yosys::BitVal> = wires.keys().cloned().collect();
        // The explore recursively the gates
        let bit_uses: HashMap<yosys::BitVal, Vec<_>> =
            crate::gadget_internals::list_wire_uses(gadget.module);
        let clock_bitval = gadget
            .clock
            .as_ref()
            .map(|clk| &gadget.module.netnames[*clk].bits);
        let v = Vec::new(); // constant empty vector
        while let Some(bitval) = to_explore.pop() {
            for (cell_name, port_name, offset) in bit_uses.get(&bitval).unwrap_or_else(|| &v).iter()
            {
                let gate_id = GateId::new(*cell_name, *offset);
                if let hash_map::Entry::Vacant(entry) = gate_names.entry(gate_id) {
                    let cell = &gadget.module.cells[*cell_name];
                    let cell_type = cell.cell_type.as_str();
                    let output = match (cell_type, BoolBinKind::from_str(cell_type)) {
                        ("DFF", _) => {
                            assert_eq!(
                                Some(&cell.connections["C"]),
                                clock_bitval,
                                "Wrong clock on random DFF"
                            );
                            Some((RawGate::Reg, "Q"))
                        }
                        ("MUX", _) => {
                            if *port_name == "S" {
                                bail!("The wire {:?} depends on randomness or shares and drives the selector of the mux {}. This is not supported.", bitval, cell_name)
                            }
                            let ctrl = cell.connections["S"][*offset as usize];
                            Some((RawGate::Mux(ctrl), "Y"))
                        }
                        ("NOT", _) => Some((RawGate::Inv, "Y")),
                        ("BUF", _) => Some((RawGate::Buf, "Y")),
                        (_, Some(kind)) => Some((RawGate::BoolBin(kind), "Y")),
                        _ => {
                            bail!("The cell {} (port {}[{}]) is connected to a random/sensitive wire but is not a known type of gate (type: {})", cell_name, port_name, offset, cell.cell_type)
                        }
                    };
                    if let Some((kind, output_name)) = output {
                        let node =
                            gates.add_node(GNode::Gate(kind, GateId::new(*cell_name, *offset)));
                        entry.insert(node);
                        let output_bitval = cell.connections[output_name][*offset as usize];
                        wires.entry(output_bitval).or_insert_with(|| {
                            to_explore.push(output_bitval);
                            node
                        });
                    }
                }
            }
        }
        for node in gates.node_identifiers() {
            if let GNode::Gate(kind, id) = gates[node] {
                let conn_names = match kind {
                    RawGate::Mux(_) | RawGate::BoolBin(_) => ["A", "B"].as_ref(),
                    RawGate::Inv => ["A"].as_ref(),
                    RawGate::Buf => ["A"].as_ref(),
                    RawGate::Reg => ["D"].as_ref(),
                };
                let cell = &gadget.module.cells[id.cell];
                for conn_name in conn_names {
                    let bita = &cell.connections[*conn_name][id.offset as usize];
                    let node_a = wires.get(bita).map(|node_a| *node_a).unwrap_or_else(|| {
                        let node_a = gates.add_node(GNode::Constant(*bita));
                        wires.insert(*bita, node_a);
                        node_a
                    });
                    gates.add_edge(
                        node_a,
                        node,
                        Edge {
                            input_name: conn_name,
                        },
                    );
                }
            }
        }
        for sharing in gadget.outputs.keys() {
            for i in 0..gadget.order {
                let offset = sharing.pos * gadget.order + i;
                let bitval = &gadget.module.ports[sharing.port_name].bits[offset as usize];
                wires
                    .entry(*bitval)
                    .or_insert_with(|| gates.add_node(GNode::Constant(*bitval)));
            }
        }
        Ok(Self {
            gates,
            wires,
            gate_names,
            gadget,
        })
    }
    fn input(&self, node: NodeIndex, name: &str) -> NodeIndex {
        self.gates
            .edges_directed(node, Direction::Incoming)
            .filter(|e| e.weight().input_name == name)
            .next()
            .unwrap()
            .source()
    }
    pub fn unroll(
        &'b self,
        controls: &mut clk_vcd::ModuleControls,
    ) -> Result<UnrolledGates<'a, 'b>> {
        let n_cycles = self.gadget.max_output_lat() + 1;
        let sorted_nodes = self.sort_nodes()?;
        let mut res = Graph::new();
        let mut new_nodes: HashMap<(NodeIndex, Latency), NodeIndex> = HashMap::default();
        for cycle in 0..n_cycles {
            for node in sorted_nodes.iter() {
                match self.gates[*node] {
                    GNode::Gate(RawGate::Reg, id) => {
                        if cycle == 0 {
                        } else if let Some(src) =
                            new_nodes.get(&(self.input(*node, "D"), cycle - 1))
                        {
                            let new_node = res.add_node((GNode::Gate(RawGate::Reg, id), cycle));
                            res.add_edge(*src, new_node, D_EDGE);
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Gate(RawGate::Inv, id) => {
                        if let Some(src) = new_nodes.get(&(self.input(*node, "A"), cycle)) {
                            let new_node = res.add_node((GNode::Gate(RawGate::Inv, id), cycle));
                            res.add_edge(*src, new_node, A_EDGE);
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Gate(RawGate::Buf, id) => {
                        if let Some(src) = new_nodes.get(&(self.input(*node, "A"), cycle)) {
                            let new_node = res.add_node((GNode::Gate(RawGate::Buf, id), cycle));
                            res.add_edge(*src, new_node, A_EDGE);
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Gate(kind, id) => {
                        let new_kind = match kind {
                            RawGate::Mux(ctrl) => {
                                RawGate::Mux(self.wire_value(ctrl, cycle, controls)?)
                            }
                            RawGate::BoolBin(bkind) => RawGate::BoolBin(bkind),
                            _ => unreachable!(),
                        };
                        let src_a = new_nodes.get(&(self.input(*node, "A"), cycle));
                        let src_b = new_nodes.get(&(self.input(*node, "B"), cycle));
                        if src_a.is_some() || src_b.is_some() {
                            let new_node = res.add_node((GNode::Gate(new_kind, id), cycle));
                            if let Some(src) = src_a {
                                res.add_edge(*src, new_node, A_EDGE);
                            }
                            if let Some(src) = src_b {
                                res.add_edge(*src, new_node, B_EDGE);
                            }
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Input(GadgetInput::Random(rnd)) => {
                        if crate::tg_graph::is_rnd_valid(self.gadget, &rnd, cycle, controls)? {
                            let new_node =
                                res.add_node((GNode::Input(GadgetInput::Random(rnd)), cycle));
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Input(GadgetInput::Share(share)) => {
                        if self.gadget.inputs[&share.sharing].contains(&cycle) {
                            let new_node =
                                res.add_node((GNode::Input(GadgetInput::Share(share)), cycle));
                            new_nodes.insert((*node, cycle), new_node);
                        }
                    }
                    GNode::Constant(cst) => {
                        let cst = self.wire_value(cst, cycle, controls)?;
                        let new_node = res.add_node((GNode::Constant(cst), cycle));
                        new_nodes.insert((*node, cycle), new_node);
                    }
                }
            }
        }
        let outputs = self
            .gadget
            .outputs
            .iter()
            .flat_map(|(output, lat)| {
                let gates2timed = &new_nodes;
                (0..self.gadget.order).map(move |i| {
                    let o_bitval = self.gadget.module.ports[output.port_name].bits
                        [(self.gadget.order * output.pos + i) as usize];

                    (
                        gates2timed[&(self.wires[&o_bitval], *lat)],
                        Share {
                            sharing: *output,
                            share_id: i,
                        },
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let sorted_gates = res.node_identifiers().collect::<Vec<_>>();
        Ok(UnrolledGates {
            tgates: res,
            gates2timed: new_nodes,
            sorted_gates,
            gadget: self,
            outputs,
            n_cycles,
        })
    }
    fn wire_value(
        &self,
        wire: yosys::BitVal,
        cycle: Latency,
        controls: &mut clk_vcd::ModuleControls,
    ) -> Result<bool> {
        let (wire_name, offset) = crate::netlist::get_names(self.gadget.module, wire)
            .next()
            .expect("No names for net");
        let res = controls
            .lookup(vec![wire_name.to_owned()], cycle as usize, offset)?
            .and_then(|var_state| var_state.to_bool())
            .ok_or_else(|| {
                anyhow::Error::msg(format!(
                    "TODO format {:?}",
                    CompError::other(
                        &self.gadget.module,
                        wire_name,
                        &format!(
                            "Control signal {}[{}] has no valid value at cycle {}",
                            wire_name, offset, cycle
                        ),
                    )
                ))
            })?;
        return Ok(res);
    }
    fn sort_nodes(&self) -> Result<Vec<petgraph::graph::NodeIndex>> {
        let mut g = self.gates.clone();
        g.clear_edges();
        for e in self.gates.raw_edges().iter() {
            // Drop input edges of regs: they come from the past, therefore to not model a
            // combinational dependency.
            if let GNode::Gate(RawGate::Reg, _) = self.gates[e.target()] {
            } else {
                g.add_edge(e.source(), e.target(), e.weight.clone());
            }
        }
        Ok(petgraph::algo::toposort(&g, None).map_err(|cycle| {
            anyhow::Error::msg(format!(
                "Looping data depdendency containing gadget {:?}",
                g[cycle.node_id()]
            ))
        })?)
    }
}

#[derive(Debug, Clone)]
pub enum BoolGate {
    Not,
    Buf,
    Bin(BoolBinKind),
}

impl BoolGate {
    fn from_bin_kind(kind: BoolBinKind) -> Self {
        Self::Bin(kind)
    }
}

#[derive(Debug, Clone)]
pub enum CGNode<'a> {
    Input(GadgetInput<'a>, Latency),
    Gate(BoolGate, (GateId<'a>, Latency)),
    Constant(bool),
}

#[derive(Debug, Clone)]
pub struct LeakComputationGraph<'a> {
    cg: Graph<CGNode<'a>, ()>,
    e_probes_output: Vec<BTreeSet<NodeIndex>>,
    e_probes_internal: Vec<BTreeSet<NodeIndex>>,
    // Map from output shares to index in the e_probes_output vector, as the
    // corresponding exended probe.
    outputs_map: HashMap<Share<'a>, usize>,
    n_shares: u32,
}
