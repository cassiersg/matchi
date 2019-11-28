use crate::clk_vcd;
use crate::error::{CResult, CompError, CompErrorKind, CompErrors};
use crate::gadget_internals::{self, Connection, GName, RndConnection};
use crate::gadgets::{self, Input, Latency, Sharing};
use crate::netlist;
use petgraph::{
    graph::{self, NodeIndex},
    visit::{EdgeRef, IntoNodeIdentifiers, IntoNodeReferences},
    Direction, Graph,
};
use std::collections::{BinaryHeap, HashMap, HashSet};

pub type Name<'a> = (GName<'a>, Latency);
pub type TRandom<'a> = (gadgets::Random<'a>, Latency);
pub type TRndConnection<'a> = (RndConnection<'a>, Latency);

type RndTrace<'a> = Vec<(RndConnection<'a>, gadgets::Latency)>;

#[derive(Debug, Clone)]
struct TGadget<'a, 'b> {
    base: gadget_internals::GadgetInstance<'a, 'b>,
    name: (GName<'a>, Latency),
    random_connections: HashMap<TRandom<'a>, TRndConnection<'a>>,
}

#[derive(Debug, Clone)]
enum TGInst<'a, 'b> {
    Gadget(TGadget<'a, 'b>),
    Input(Latency),
    Output,
    Invalid,
}

impl<'a, 'b> TGInst<'a, 'b> {
    fn is_gadget(&self) -> bool {
        if let TGInst::Gadget(_) = self {
            true
        } else {
            false
        }
    }
}

pub type Edge<'a> = (Sharing<'a>, Input<'a>);
pub struct AEdge<'a> {
    edge: Edge<'a>,
    valid: bool,
    sensitive: Sensitive,
}
pub type AUGIGraph<'a, 'b> = UGIGraph<'a, 'b, AEdge<'a>>;

pub struct UGIGraph<'a, 'b, E = Edge<'a>> {
    pub internals: gadget_internals::GadgetInternals<'a, 'b>,
    gadgets: Graph<TGInst<'a, 'b>, E>,
    n_cycles: Latency,
    o_node: petgraph::graph::NodeIndex,
    #[allow(dead_code)]
    inv_node: petgraph::graph::NodeIndex,
    i_nodes: Vec<petgraph::graph::NodeIndex>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sensitive {
    No,
    Glitch,
    Yes,
}

const EMPTY_SHARING: Sharing = Sharing {
    port_name: "",
    pos: 0,
};

fn random_connections<'a, 'b>(
    sgi: &gadget_internals::GadgetInstance<'a, 'b>,
    cycle: u32,
) -> CResult<'a, HashMap<TRandom<'a>, TRndConnection<'a>>> {
    let mut res = HashMap::new();
    for (r_name, random_lats) in sgi.kind.randoms.iter() {
        match random_lats {
            Some(random_lats) => {
                for lat in random_lats.iter() {
                    res.insert(
                        (*r_name, *lat),
                        (sgi.random_connections[r_name], lat + cycle),
                    );
                }
            }
            None => {
                Err(CompError::ref_sn(
                    sgi.kind.module,
                    &r_name.port_name,
                    CompErrorKind::MissingAnnotation("psim_lat".to_owned()),
                ))?;
            }
        }
    }
    return Ok(res);
}

fn time_connection<'a, 'b>(
    conn: &gadget_internals::Connection<'a>,
    internals: &gadget_internals::GadgetInternals<'a, 'b>,
    src_latency: Latency,
    cycle: Latency,
    n_cycles: Latency,
    i_nodes: &Vec<NodeIndex>,
    g_nodes: &HashMap<Name<'a>, NodeIndex>,
    inv_node: NodeIndex,
) -> (NodeIndex, Sharing<'a>) {
    match conn {
        Connection::GadgetOutput {
            gadget_name,
            output,
        } => {
            let output_latency = internals.subgadgets[*gadget_name].kind.outputs[output];
            if let Some(ref_cycle) = (cycle + src_latency)
                .checked_sub(output_latency)
                .filter(|ref_cycle| *ref_cycle < n_cycles)
            {
                return (g_nodes[&(*gadget_name, ref_cycle)], *output);
            }
        }
        Connection::Input(input) => {
            let in_lat = src_latency + cycle;
            if internals.gadget.inputs[input].contains(&in_lat) {
                return (i_nodes[in_lat as usize], *input);
            }
        }
    }
    return (inv_node, EMPTY_SHARING);
}

impl<'a, 'b, E> UGIGraph<'a, 'b, E> {
    fn muxes_controls(
        &self,
        controls: &mut clk_vcd::ModuleControls,
    ) -> CResult<'a, HashMap<NodeIndex, Option<bool>>> {
        let mut muxes_ctrls = HashMap::<NodeIndex, Option<bool>>::new();
        for (idx, tginst) in self.gadgets.node_references() {
            if let TGInst::Gadget(gadget) = tginst {
                if gadget.base.kind.prop == netlist::GadgetProp::Mux {
                    let sel_name = "sel".to_owned();
                    let path: Vec<String> = vec![(gadget.name.0).to_owned(), sel_name.to_owned()];
                    let sel = controls
                        .lookup(path, gadget.name.1 as usize, 0)?
                        .unwrap_or_else(|| {
                            panic!(
                                "Missing simulation cycle for mux selector, gadget: {:?}",
                                gadget.name
                            )
                        });
                    let sel = match sel {
                        clk_vcd::VarState::Scalar(vcd::Value::V0) => Some(false),
                        clk_vcd::VarState::Scalar(vcd::Value::V1) => Some(true),
                        clk_vcd::VarState::Vector(_) => unreachable!(),
                        clk_vcd::VarState::Uninit => unreachable!(),
                        clk_vcd::VarState::Scalar(vcd::Value::X)
                        | clk_vcd::VarState::Scalar(vcd::Value::Z) => None,
                    };
                    muxes_ctrls.insert(idx, sel);
                }
            }
        }
        return Ok(muxes_ctrls);
    }

    fn g_inputs(&self, gadget: NodeIndex) -> graph::Edges<E, petgraph::Directed> {
        self.gadgets.edges_directed(gadget, Direction::Incoming)
    }

    fn g_outputs(&self, gadget: NodeIndex) -> graph::Edges<E, petgraph::Directed> {
        self.gadgets.edges_directed(gadget, Direction::Outgoing)
    }

    fn gadgets<'s>(&'s self) -> impl Iterator<Item = (NodeIndex, &'s TGadget<'a, 'b>)> + 's {
        self.gadgets.node_identifiers().filter_map(move |idx| {
            if let &TGInst::Gadget(ref gadget) = &self.gadgets[idx] {
                Some((idx, gadget))
            } else {
                None
            }
        })
    }

    pub fn gadget_names<'s>(&'s self) -> impl Iterator<Item = Name<'a>> + 's {
        self.gadgets().map(|(_, g)| g.name)
    }

    fn gadget(&self, idx: NodeIndex) -> &TGadget<'a, 'b> {
        if let TGInst::Gadget(ref gadget) = &self.gadgets[idx] {
            gadget
        } else {
            panic!("Gadget at index {:?} is not a gadget.", idx)
        }
    }
}

impl<'a, 'b> UGIGraph<'a, 'b> {
    pub fn unroll(
        internals: gadget_internals::GadgetInternals<'a, 'b>,
        n_cycles: Latency,
    ) -> CResult<'a, Self> {
        let mut gadgets = Graph::new();
        let o_node = gadgets.add_node(TGInst::Output);
        let inv_node = gadgets.add_node(TGInst::Invalid);
        let i_nodes = (0..n_cycles)
            .map(|lat| gadgets.add_node(TGInst::Input(lat)))
            .collect::<Vec<_>>();
        let mut g_nodes = HashMap::new();
        for (name, sgi) in internals.subgadgets.iter() {
            for lat in 0..n_cycles {
                let g = gadgets.add_node(TGInst::Gadget(TGadget {
                    base: sgi.clone(),
                    name: (name, lat),
                    random_connections: random_connections(sgi, lat)?,
                }));
                g_nodes.insert((*name, lat), g);
            }
        }
        for (name, sgi) in internals.subgadgets.iter() {
            for (input, conn) in sgi.input_connections.iter() {
                for cycle in 0..n_cycles {
                    for src_latency in sgi.kind.inputs[input].iter() {
                        let (src_g, src_o) = time_connection(
                            conn,
                            &internals,
                            *src_latency,
                            cycle,
                            n_cycles,
                            &i_nodes,
                            &g_nodes,
                            inv_node,
                        );
                        gadgets.add_edge(
                            src_g,
                            g_nodes[&(*name, cycle)],
                            (src_o, (*input, *src_latency)),
                        );
                    }
                }
            }
        }
        for (output, conn) in internals.output_connections.iter() {
            for cycle in 0..n_cycles {
                let (src_g, src_o) = time_connection(
                    conn, &internals, 0, cycle, n_cycles, &i_nodes, &g_nodes, inv_node,
                );
                gadgets.add_edge(src_g, o_node, (src_o, (*output, cycle)));
            }
        }
        return Ok(Self {
            internals,
            gadgets,
            n_cycles,
            o_node,
            inv_node,
            i_nodes,
        });
    }

    fn toposort(
        &self,
        muxes_ctrls: &HashMap<NodeIndex, Option<bool>>,
    ) -> CResult<'a, Vec<NodeIndex>> {
        let mut edges_kept = vec![true; self.gadgets.raw_edges().len()];
        for (idx, ctrl) in muxes_ctrls.iter() {
            if let Some(ctrl) = ctrl {
                // invert because we remove the input
                let input = if !*ctrl { "in_true" } else { "in_false" };
                for e in self.g_inputs(*idx) {
                    if (e.weight().1).0.port_name == input {
                        edges_kept[e.id().index()] = false;
                    }
                }
            }
        }
        let mut gadgets = self.gadgets.clone();
        //gadgets.retain_edges(|_, e| edges_kept[e.index()]);
        gadgets.clear_edges();
        for (k, e) in edges_kept.into_iter().zip(self.gadgets.raw_edges().iter()) {
            if k {
                gadgets.add_edge(e.source(), e.target(), e.weight.clone());
            }
        }
        Ok(petgraph::algo::toposort(&gadgets, None).map_err(|cycle| {
            CompError::ref_nw(
                &self.internals.gadget.module,
                CompErrorKind::Other(format!(
                    "Looping data depdendency containing gadget {:?}",
                    gadgets[cycle.node_id()]
                )),
            )
        })?)
    }

    fn annotate_inner(
        &self,
        muxes_ctrls: &HashMap<NodeIndex, Option<bool>>,
        sorted_nodes: &[NodeIndex],
    ) -> Result<(Vec<bool>, Vec<Sensitive>), CompError<'a>> {
        let mut edges_valid: Vec<Option<bool>> = vec![None; self.gadgets.edge_count()];
        let mut edges_sensitive: Vec<Option<Sensitive>> = vec![None; self.gadgets.edge_count()];
        for idx in sorted_nodes.into_iter() {
            match &self.gadgets[*idx] {
                TGInst::Gadget(_) => {
                    if let Some(ctrl) = muxes_ctrls.get(idx) {
                        let mut outputs: Vec<Option<usize>> =
                            vec![None; self.g_outputs(*idx).count()];
                        for o_e in self.g_outputs(*idx) {
                            let pos = o_e.weight().0.pos as usize;
                            outputs[pos] = Some(o_e.id().index());
                        }
                        if let Some(ctrl) = ctrl {
                            let input = if *ctrl { "in_true" } else { "in_false" };
                            for e_i in self.g_inputs(*idx) {
                                let Sharing { port_name, pos } = &(e_i.weight().1).0;
                                if port_name == &input {
                                    edges_valid[outputs[*pos as usize].unwrap()] =
                                        edges_valid[e_i.id().index()];
                                    // Neglect glitches for now, will come to it later
                                    edges_sensitive[outputs[*pos as usize].unwrap()] =
                                        edges_sensitive[e_i.id().index()];
                                }
                            }
                        } else {
                            for e_i in self.g_inputs(*idx) {
                                let Sharing { pos, .. } = &(e_i.weight().1).0;
                                let output = outputs[*pos as usize].unwrap();
                                let sens_new = edges_sensitive[e_i.id().index()].unwrap();
                                let sens = edges_sensitive[output].get_or_insert(Sensitive::No);
                                *sens = (*sens).max(sens_new);
                            }
                        }
                    } else {
                        let g_valid = self.g_inputs(*idx).all(|e| {
                            edges_valid[e.id().index()].expect("Non-initialized validity")
                        });
                        let g_sensitive = self
                            .g_inputs(*idx)
                            .map(|e| {
                                edges_sensitive[e.id().index()]
                                    .expect("Non-initialized sensitivity")
                            })
                            .max()
                            .unwrap_or(Sensitive::No);
                        for e in self.g_outputs(*idx) {
                            edges_valid[e.id().index()] = Some(g_valid);
                            edges_sensitive[e.id().index()] = Some(g_sensitive);
                        }
                    }
                }
                TGInst::Input(lat) => {
                    for e in self.g_outputs(*idx) {
                        let (src_sharing, _) = e.weight();
                        let valid = self.internals.gadget.inputs[src_sharing].contains(&lat);
                        edges_valid[e.id().index()] = Some(valid);
                        // Worst-case analysis: we assume there may be glitches on the inputs.
                        edges_sensitive[e.id().index()] = if valid {
                            Some(Sensitive::Yes)
                        } else {
                            Some(Sensitive::Glitch)
                        };
                    }
                }
                TGInst::Output => {
                    assert!(self.g_outputs(*idx).next().is_none());
                }
                TGInst::Invalid => {
                    // Connections from non-existing gadgets or out of range inputs.
                    // Assume that they are non-sensitive (might want to assume glitch ?)
                    for e in self.g_outputs(*idx) {
                        edges_valid[e.id().index()] = Some(false);
                        edges_sensitive[e.id().index()] = Some(Sensitive::No);
                    }
                }
            }
        }
        // move sensitivity to full form
        let mut edges_sensitive = edges_sensitive
            .into_iter()
            .map(|x| x.unwrap().into())
            .collect::<Vec<Sensitive>>();
        // Propagate glitches
        // Stack of gadgets on which to propagate glitches
        let mut gl_to_analyze = self.gadgets().map(|x| x.0).collect::<BinaryHeap<_>>();
        while let Some(idx) = gl_to_analyze.pop() {
            let noglitch_cycle = compute_sensitivity(
                self.g_inputs(idx)
                    .map(|e| (edges_sensitive[e.id().index()], e.weight().1)),
            );
            for e in self.g_outputs(idx) {
                let output_lat = self.gadget(idx).base.kind.outputs[&e.weight().0];
                if edges_sensitive[e.id().index()] == Sensitive::No && output_lat < noglitch_cycle {
                    edges_sensitive[e.id().index()] = Sensitive::Glitch;
                    if self.gadgets[e.target()].is_gadget() {
                        gl_to_analyze.push(e.target());
                    }
                }
            }
        }
        return Ok((
            edges_valid.into_iter().map(Option::unwrap).collect(),
            edges_sensitive,
        ));
    }

    pub fn annotate(
        &self,
        controls: &mut clk_vcd::ModuleControls,
    ) -> CResult<'a, AUGIGraph<'a, 'b>> {
        let muxes_ctrls = self.muxes_controls(controls)?;
        let sorted_nodes = self.toposort(&muxes_ctrls)?;
        let (validities, sensitivities) = self.annotate_inner(&muxes_ctrls, &sorted_nodes)?;
        let mut new_gadgets = Graph::with_capacity(
            self.gadgets.raw_nodes().len(),
            self.gadgets.raw_edges().len(),
        );
        for (i, n) in self.gadgets.raw_nodes().iter().enumerate() {
            assert_eq!(i, new_gadgets.add_node(n.weight.clone()).index());
        }
        for (i, e) in self.gadgets.raw_edges().iter().enumerate() {
            assert_eq!(
                i,
                new_gadgets
                    .add_edge(
                        e.source(),
                        e.target(),
                        AEdge {
                            edge: e.weight.clone(),
                            valid: validities[i],
                            sensitive: sensitivities[i],
                        }
                    )
                    .index()
            );
        }
        return Ok(UGIGraph::<AEdge> {
            internals: self.internals.clone(),
            gadgets: new_gadgets,
            n_cycles: self.n_cycles,
            o_node: self.o_node,
            inv_node: self.inv_node,
            i_nodes: self.i_nodes.clone(),
        });
    }
}

impl<'a, 'b> UGIGraph<'a, 'b, AEdge<'a>> {
    fn gadget_valid(&self, n: NodeIndex) -> bool {
        self.g_inputs(n).all(|e| e.weight().valid)
    }
    fn gadget_sensitive(&self, n: NodeIndex) -> bool {
        self.g_inputs(n)
            .any(|e| e.weight().sensitive == Sensitive::Yes)
    }
    pub fn list_valid(&self) -> HashMap<GName<'a>, Vec<Latency>> {
        let mut gadgets = HashMap::new();
        for (id, gadget) in self.gadgets() {
            if gadget.base.kind.prop != netlist::GadgetProp::Mux && self.gadget_valid(id) {
                gadgets
                    .entry(gadget.name.0)
                    .or_insert_with(Vec::new)
                    .push(gadget.name.1);
            }
        }
        for cycles in gadgets.values_mut() {
            cycles.sort_unstable();
        }
        return gadgets;
    }
    pub fn list_sensitive(&self) -> HashMap<GName<'a>, Vec<Latency>> {
        let mut gadgets = HashMap::new();
        for (id, gadget) in self.gadgets() {
            if gadget.base.kind.prop != netlist::GadgetProp::Mux && self.gadget_sensitive(id) {
                gadgets
                    .entry(gadget.name.0)
                    .or_insert_with(Vec::new)
                    .push(gadget.name.1);
            }
        }
        for cycles in gadgets.values_mut() {
            cycles.sort_unstable();
        }
        return gadgets;
    }
    pub fn warn_useless_rnd(&self) -> Vec<Name<'a>> {
        self.gadgets()
            .filter(|(idx, gadget)| {
                !gadget.random_connections.is_empty()
                    && self.gadget_sensitive(*idx)
                    && !self.gadget_valid(*idx)
            })
            .map(|(_, g)| g.name)
            .collect()
    }
    pub fn disp_full(&self) {
        for (id, gadget) in self.gadgets() {
            println!("Gadget {:?}:", gadget.name);
            println!("Inputs:");
            for e in self.g_inputs(id) {
                let AEdge {
                    edge: (_, (sharing, lat)),
                    valid,
                    sensitive,
                } = e.weight();
                println!(
                    "\tinput ({}[{}], {}): Valid: {:?}, Sensitive: {:?}",
                    sharing.port_name, sharing.pos, lat, valid, sensitive
                );
            }
            for e in self.g_outputs(id) {
                let AEdge {
                    edge: (sharing, _),
                    valid,
                    sensitive,
                } = e.weight();
                println!(
                    "\toutput {}[{}]: Valid: {:?}, Sensitive: {:?}",
                    sharing.port_name, sharing.pos, valid, sensitive
                );
            }
        }
    }
    pub fn check_valid_outputs(&self) -> CResult<'a, ()> {
        let valid_outputs = self
            .g_inputs(self.o_node)
            .filter_map(|e| {
                if e.weight().valid {
                    Some(e.weight().edge.1)
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        let missing_outputs = self
            .internals
            .gadget
            .outputs
            .iter()
            .map(|(sh, lat)| (*sh, *lat))
            .filter(|(sh, lat)| !valid_outputs.contains(&(*sh, *lat)))
            .collect::<Vec<_>>();
        if !missing_outputs.is_empty() {
            Err(CompError::ref_nw(
                &self.internals.gadget.module,
                CompErrorKind::OutputNotValid(missing_outputs),
            ))?;
        }
        let excedentary_outputs = self
            .g_inputs(self.o_node)
            .filter_map(|e| {
                // We don't care about glitches, those are inferred and always assumed by the
                // outside world.
                if e.weight().sensitive == Sensitive::Yes {
                    let (sh, lat) = e.weight().edge.1;
                    if self.internals.gadget.outputs.get(&sh).map(|l| *l == lat) != Some(true) {
                        return Some((sh, lat));
                    }
                }
                return None;
            })
            .collect::<Vec<_>>();
        if !excedentary_outputs.is_empty() {
            Err(CompError::ref_nw(
                &self.internals.gadget.module,
                CompErrorKind::ExcedentaryOutput(excedentary_outputs),
            ))?;
        }
        return Ok(());
    }

    pub fn randoms_input_timing(
        &self,
        controls: &mut clk_vcd::ModuleControls,
    ) -> CResult<'a, HashMap<TRandom<'a>, (Name<'a>, TRandom<'a>)>> {
        let mut rnd_gadget2input: Vec<
            HashMap<TRandom<'a>, CResult<'a, (TRandom<'a>, RndTrace<'a>)>>,
        > = vec![HashMap::new(); self.gadgets.node_count()];
        let mut name_cache = HashMap::new();
        let mut errors: Vec<CompError<'a>> = Vec::new();
        for (idx, gadget) in self.gadgets() {
            for (conn, trandom) in gadget.random_connections.iter() {
                let rnd_in = random_to_input(
                    &self.internals,
                    controls,
                    trandom,
                    &gadget.name,
                    conn,
                    &mut name_cache,
                );
                if let Err(e) = &rnd_in {
                    if self.gadget_sensitive(idx) {
                        errors.extend_from_slice(&e.0);
                    }
                }
                rnd_gadget2input[idx.index()].insert(*conn, rnd_in);
            }
        }
        let mut rnd_input2use: HashMap<TRandom<'a>, Vec<(NodeIndex, TRandom<'a>)>> = HashMap::new();
        for (idx, rnd2input) in rnd_gadget2input.iter().enumerate() {
            for (rnd, rnd_input) in rnd2input.iter() {
                if let Ok((input, _)) = rnd_input {
                    rnd_input2use
                        .entry(*input)
                        .or_default()
                        .push((NodeIndex::new(idx), *rnd));
                }
            }
        }
        let mut res: HashMap<TRandom<'a>, (Name<'a>, TRandom<'a>)> = HashMap::new();
        for (in_rnd, uses) in rnd_input2use.iter() {
            assert!(!uses.is_empty());
            if uses.iter().any(|(idx, _)| self.gadget_sensitive(*idx)) {
                if uses.len() > 1 {
                    let random_uses = uses
                        .iter()
                        .map(|(idx, rnd)| {
                            let TGadget { name, .. } = self.gadget(*idx);
                            (
                                (*name, rnd.clone()),
                                rnd_gadget2input[idx.index()][rnd]
                                    .as_ref()
                                    .unwrap()
                                    .1
                                    .clone(),
                            )
                        })
                        .collect::<Vec<_>>();
                    errors.push(CompError::ref_nw(
                        &self.internals.gadget.module,
                        CompErrorKind::MultipleUseRandom {
                            random: *in_rnd,
                            uses: random_uses,
                        },
                    ));
                } else {
                    let TGadget { name, .. } = self.gadget(uses[0].0);
                    res.insert(*in_rnd, (*name, uses[0].1));
                }
            }
        }
        if !errors.is_empty() {
            return Err(CompErrors::new(errors));
        }
        return Ok(res);
    }

    pub fn check_state_cleared(&self) -> CResult<'a, ()> {
        let errors = self
            .gadgets()
            .filter(|(idx, _)| self.gadget_sensitive(*idx))
            .flat_map(|(idx, gadget)| {
                self.g_outputs(idx).filter_map(move |e| {
                    let out_lat = gadget.base.kind.outputs[&e.weight().edge.0];
                    if gadget.name.1 + out_lat > self.n_cycles - 1 {
                        Some(CompError::ref_nw(
                            &self.internals.gadget.module,
                            CompErrorKind::LateOutput(
                                gadget.name.1 + out_lat - self.n_cycles + 1,
                                gadget.name.0.to_owned(),
                                e.weight().edge.0,
                            ),
                        ))
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();
        if errors.is_empty() {
            return Ok(());
        } else {
            return Err(CompErrors::new(errors));
        }
    }

    pub fn sensitive_gadgets<'s>(
        &'s self,
    ) -> impl Iterator<Item = (Name<'a>, &'s gadget_internals::GadgetInstance<'a, 'b>)> + 's {
        self.gadgets()
            .filter(move |(idx, _)| self.gadget_sensitive(*idx))
            .map(|(_, g)| (g.name, &g.base))
    }
}

fn compute_sensitivity<'a>(inputs: impl Iterator<Item = (Sensitive, Input<'a>)>) -> Latency {
    inputs
        .filter(|(sensitive, _)| *sensitive != Sensitive::No)
        .map(|(_, (_, lat))| lat)
        .max()
        .map(|x| x + 1)
        .unwrap_or(0)
}

// Returns None if input is late
fn random_to_input<'a, 'b>(
    internals: &gadget_internals::GadgetInternals<'a, 'b>,
    controls: &mut clk_vcd::ModuleControls,
    trandom: &TRndConnection<'a>,
    sg_name: &Name<'a>,
    rnd_name: &TRandom<'a>,
    names_cache: &mut HashMap<&'a str, (&'a str, usize)>,
) -> CResult<'a, (TRandom<'a>, Vec<(RndConnection<'a>, gadgets::Latency)>)> {
    let module = internals.gadget.module;
    let mut trandom_w: Vec<(RndConnection<'a>, gadgets::Latency)> = vec![(trandom.0, trandom.1)];
    loop {
        let rnd_to_add = match &trandom_w[trandom_w.len() - 1] {
            (RndConnection::Invalid(bit), _) => {
                return Err(CompError::ref_nw(
                    module,
                    CompErrorKind::InvalidRandom(trandom_w.clone(), *sg_name, *rnd_name, *bit),
                )
                .into());
            }
            (RndConnection::Port(rnd), cycle) => {
                return Ok((((*rnd, *cycle)), trandom_w.clone()));
            }
            (RndConnection::Gate(gate_id), cycle) => match &internals.rnd_gates[&gate_id] {
                gadget_internals::RndGate::Reg { input: new_conn } => {
                    if *cycle == 0 {
                        Err(CompError::ref_nw(module, CompErrorKind::Other(format!("Randomness for random {:?} of gadget {:?} comes from a cycle before cycle 0 (through reg {:?})", rnd_name, sg_name, gate_id))))?;
                    }
                    (*new_conn, cycle - 1)
                }
                gadget_internals::RndGate::Mux { ina, inb } => {
                    let (var_name, offset) = names_cache.entry(gate_id.0).or_insert_with(|| {
                        netlist::get_names(module, module.cells[gate_id.0].connections["S"][0])
                            .next()
                            .expect("No names for net")
                    });
                    let var_name = netlist::format_name(var_name);
                    match controls.lookup(vec![var_name], *cycle as usize, *offset)? {
                        None => {
                            return Err(CompError::ref_nw(
                                module,
                                CompErrorKind::Other(format!(
                                    "Random comes from a late gate for gadget {:?}",
                                    sg_name
                                )),
                            )
                            .into());
                        }
                        Some(clk_vcd::VarState::Scalar(vcd::Value::V0)) => (*ina, *cycle),
                        Some(clk_vcd::VarState::Scalar(vcd::Value::V1)) => (*inb, *cycle),
                        Some(clk_vcd::VarState::Vector(_)) => unreachable!(),
                        Some(sel @ clk_vcd::VarState::Scalar(vcd::Value::Z))
                        | Some(sel @ clk_vcd::VarState::Uninit)
                        | Some(sel @ clk_vcd::VarState::Scalar(vcd::Value::X)) => {
                            return Err(CompError::ref_nw(module, CompErrorKind::Other(format!(
                                    "Invalid control signal {:?} for mux {} at cycle {} for randomness", sel,
                                    gate_id.0, cycle
                                ))).into());
                        }
                    }
                }
            },
        };
        trandom_w.push(rnd_to_add);
    }
}