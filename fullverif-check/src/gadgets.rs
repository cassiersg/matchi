//! Interface of a gadget: its security properties, its input and output signals.

use crate::error::{CompError, CompErrorKind};
use crate::netlist::{self, GadgetArch, GadgetProp, GadgetStrat, WireAttrs};
use anyhow::{bail, Result};
use fnv::FnvHashMap as HashMap;
use yosys_netlist_json as yosys;

/// Time unit, in clock cycles
pub type Latency = u32;

pub type Latencies = Vec<u32>;

/// Description of a bit of a random port of a gadget
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Random<'a> {
    pub port_name: &'a str,
    pub offset: u32,
}

/// Id of an input/output sharing of a gadget
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sharing<'a> {
    pub port_name: &'a str,
    pub pos: u32,
}

/// Id of a functional input.
pub type Input<'a> = (Sharing<'a>, Latency);

/// A gadget definition.
// Invariant: all output latencies are >= input and randomness latencies
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gadget<'a> {
    /// Name of the module
    pub name: &'a str,
    /// Verilog module netlist
    pub module: &'a yosys::Module,
    /// Name of the clock signal
    pub clock: Option<&'a str>,
    /// Input sharings
    pub inputs: HashMap<Sharing<'a>, Latencies>,
    /// Output sharings
    pub outputs: HashMap<Sharing<'a>, Latency>,
    /// Randomness inputs
    pub randoms: HashMap<Random<'a>, Option<netlist::RndLatencies>>,
    /// Security property
    pub prop: GadgetProp,
    /// Strategy to be used to prove the security
    pub strat: GadgetStrat,
    /// Structure: Pipeline or Loopy
    pub arch: GadgetArch,
    /// Masking order
    pub order: u32,
    pub ports: crate::composite_gadget::ConnectionVec<(&'a str, usize)>,
    pub output_ports: Vec<crate::composite_gadget::ConnectionId>,
    pub input_ports: crate::composite_gadget::InputVec<crate::composite_gadget::ConnectionId>,
}

/// Convert a module to a gadget.
pub fn module2gadget<'a>(module: &'a yosys::Module, name: &'a str) -> Result<Option<Gadget<'a>>> {
    let prop = if let Some(prop) = netlist::module_prop(module)? {
        prop
    } else if let Ok(None) = netlist::module_strat(module) {
        return Ok(None);
    } else {
        bail!(
            "TODO format {:?}",
            CompErrorKind::MissingAnnotation("fv_prop".to_owned()),
        );
    };
    // Decide if gadget is composite or not.
    let Some(strat) = netlist::module_strat(module)? else {
        bail!(
            "TODO format {:?}",
            CompErrorKind::MissingAnnotation("fv_strat".to_owned())
        );
    };
    let arch = netlist::module_arch(module)?.unwrap_or_else(|| {
        warn!(
            "Gadget {} has no fv_arch annotation, assuming 'pipeline'.",
            name
        );
        GadgetArch::Pipeline
    });
    let order = netlist::module_order(module)?;
    let mut module_ports: Vec<_> = module.ports.iter().collect();
    module_ports.sort_unstable_by_key(|&(name, _port)| name);
    let ports = module_ports
        .iter()
        .flat_map(|(port_name, port)| {
            (0..port.bits.len()).map(|offset| (port_name.as_str(), offset))
        })
        .collect::<crate::composite_gadget::ConnectionVec<_>>();
    let port_filter = |direction| {
        ports
            .iter_enumerated()
            .filter_map(|(id, (port_name, offset))| {
                (module.ports[*port_name].direction == direction).then_some(id)
            })
            .collect::<Vec<_>>()
    };
    let input_ports =
        crate::composite_gadget::InputVec::from_vec(port_filter(yosys::PortDirection::Input));
    let output_ports = port_filter(yosys::PortDirection::Output);
    let inout_ports = port_filter(yosys::PortDirection::InOut);
    if !inout_ports.is_empty() {
        bail!("Gadgets cannot have inout ports.");
    }
    // Initialize gadget.
    let mut res = Gadget {
        name,
        module,
        clock: None,
        inputs: HashMap::default(),
        outputs: HashMap::default(),
        randoms: HashMap::default(),
        prop,
        strat,
        arch,
        order,
        ports,
        input_ports,
        output_ports,
    };
    // Classify ports of the gadgets.
    for (port_name, port) in module_ports {
        match (netlist::net_attributes(module, port_name)?, port.direction) {
            (WireAttrs::Sharing { latencies, count }, dir @ yosys::PortDirection::Input)
            | (WireAttrs::Sharing { latencies, count }, dir @ yosys::PortDirection::Output) => {
                if port.bits.len() as u32 != order * count {
                    bail!(
                        "TODO format {:?}",
                        CompError::ref_sn(
                            module,
                            port_name,
                            CompErrorKind::WrongWireWidth(port.bits.len() as u32, order * count),
                        )
                    );
                }
                for pos in 0..count {
                    if dir == yosys::PortDirection::Input {
                        res.inputs
                            .insert(Sharing { port_name, pos }, latencies.clone());
                    } else {
                        if latencies.len() != 1 {
                            bail!("TODO format {:?}", CompError::ref_sn(
                        module,
                        port_name,
                        CompErrorKind::Other(format!("Outputs can be valid at only one cycle (current latencies: {:?})", latencies))));
                        }
                        res.outputs.insert(Sharing { port_name, pos }, latencies[0]);
                    }
                }
            }
            (WireAttrs::Random(randoms), yosys::PortDirection::Input) => {
                for (i, latency) in randoms.into_iter().enumerate() {
                    res.randoms.insert(
                        Random {
                            port_name,
                            offset: i as u32,
                        },
                        latency,
                    );
                }
            }
            (WireAttrs::Control, _) => {}
            (WireAttrs::Clock, _) => {
                if res.clock.is_some() {
                    bail!(
                        "TODO format {:?}",
                        CompError::ref_sn(
                            module,
                            port_name,
                            CompErrorKind::Other(
                                "Multiple clocks for gadget, while only one is supported."
                                    .to_string(),
                            ),
                        )
                    );
                }
                res.clock = Some(port_name);
                if port.bits.len() != 1 {
                    bail!(
                        "TODO format {:?}",
                        CompError::ref_sn(
                            module,
                            port_name,
                            CompErrorKind::WrongWireWidth(port.bits.len() as u32, 1),
                        )
                    );
                }
            }
            (attr, yosys::PortDirection::InOut)
            | (attr @ WireAttrs::Random(_), yosys::PortDirection::Output) => {
                bail!(
                    "TODO format {:?}",
                    CompError::ref_sn(
                        module,
                        port_name,
                        CompErrorKind::InvalidPortDirection {
                            attr,
                            direction: port.direction,
                        },
                    )
                );
            }
        }
    }
    if res.outputs.is_empty() {
        bail!("TODO format {:?}", CompErrorKind::NoOutput);
    }
    res.output_lat_ok()?;
    Ok(Some(res))
}

impl<'a> Gadget<'a> {
    /// Test if the gadget is annotated as PINI.
    pub fn is_pini(&self) -> bool {
        self.prop.is_pini()
            || (self.prop == netlist::GadgetProp::SNI && self.inputs.len() <= 1)
            || self.prop == netlist::GadgetProp::Mux
    }

    /// Maximum output latency
    pub fn max_output_lat(&self) -> Latency {
        self.outputs
            .values()
            .cloned()
            .max()
            .expect("No output for gadget")
    }

    /// BitVal mapping to a sharing.
    pub fn sharing_bits(&self, sharing: Sharing<'a>) -> &'a [yosys::BitVal] {
        &self.module.ports[sharing.port_name].bits[(sharing.pos * self.order) as usize..]
            [..self.order as usize]
    }

    /// Verify that the output latencies are larger than any input or random latency.
    fn output_lat_ok(&self) -> Result<()> {
        let min_o_lat = self.outputs.values().cloned().max().unwrap();
        let inputs_lats = self.inputs.values().flat_map(|x| x.iter());
        let randoms_lats = self
            .randoms
            .values()
            .filter_map(|x| {
                if let Some(netlist::RndLatencies::Attr(x)) = x {
                    Some(x)
                } else {
                    None
                }
            })
            .flat_map(|x| x.iter());
        let max_in_lat = inputs_lats.chain(randoms_lats).copied().min();
        if let Some(max_in_lat) = max_in_lat {
            if max_in_lat > min_o_lat {
                bail!("TODO format {:?}", CompErrorKind::EarlyOutput);
            }
        }
        return Ok(());
    }

    /// List the logic inputs of the gadget.
    pub fn inputs<'s>(&'s self) -> impl Iterator<Item = Input<'a>> + 's {
        self.inputs
            .iter()
            .flat_map(|(sharing, latencies)| latencies.iter().map(move |lat| (*sharing, *lat)))
    }

    /// Does the gadget have an input or output sharing with that name ?
    pub fn has_port(&self, port_name: &str) -> bool {
        let port = Sharing { port_name, pos: 0 };
        self.inputs.contains_key(&port) || self.outputs.contains_key(&port)
    }

    pub fn connections(&self) -> Vec<(&str, usize)> {
        self.module
            .ports
            .iter()
            .flat_map(|(port_name, port)| (0..port.bits.len()).map(|i| (port_name.as_str(), i)))
            .collect()
    }
}
