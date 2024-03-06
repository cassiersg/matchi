use super::module::{ConnectionVec, InputId, InputVec, OutputVec};
use super::{ModuleId, Netlist};
use crate::type_utils::new_id;
use crate::utils::ShareId;
use itertools::Itertools;

use anyhow::{anyhow, bail, Context, Error, Result};

use yosys_netlist_json as yosys;

// Time unit, in clock cycles
new_id!(Latency, LatencyVec, LatencySlice);
pub type Slatency = i32;
impl From<Latency> for Slatency {
    fn from(value: Latency) -> Self {
        value.index().try_into().unwrap()
    }
}
impl std::ops::Add<i32> for Latency {
    type Output = Latency;
    fn add(self, rhs: i32) -> Self::Output {
        let res = (self.raw() as i32) + rhs;
        assert!(res >= 0);
        Latency::from_raw(res as u32)
    }
}
// List of randomness ports of a gadget.
new_id!(RndPortId, RndPortVec, RndPortSlice);

/// Gadget with fixed input/output structures, in particular:
/// - each input wire is statically assigned to a single share id. Inputs are assumed to be
/// sensitive at all clock cycles.
/// - validity of each input wire is statically defined
/// - same for outputs: sensitivity and validity are static
/// - randomness is simple uniform bits
#[derive(Clone, Debug)]
pub struct StaticGadget {
    pub module_id: ModuleId,
    name: String,
    pub input_roles: InputVec<InputRole>,
    pub output_roles: OutputVec<OutputRole>,
    pub max_latency: Latency,
    pub max_input_latency: Latency,
    // For randomness valid implies "fresh random".
    pub valid_latencies: ConnectionVec<Vec<Latency>>,
    /// Security property
    pub prop: GadgetProp,
    /// Strategy to be used to prove the security
    pub strat: GadgetStrat,
    /// Structure: Pipeline or Loopy
    pub arch: GadgetArch,
    /// Number of shares
    pub nshares: u32,
    /// randomness ports
    pub rnd_ports: RndPortVec<InputId>,
}

#[derive(Clone, Debug)]
pub enum InputRole {
    Share(ShareId),
    Random(RndPortId),
    Control, // includes clock
}
#[derive(Clone, Debug)]
pub enum OutputRole {
    Share(ShareId),
    Control,
}

/// Fullverif security property for a module gadget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GadgetProp {
    Mux,
    Affine,
    NI,
    SNI,
    PINI,
}

impl GadgetProp {
    pub fn is_pini(&self) -> bool {
        match self {
            GadgetProp::Mux | GadgetProp::Affine | GadgetProp::PINI => true,
            _ => false,
        }
    }
    pub fn is_affine(&self) -> bool {
        match self {
            GadgetProp::Mux | GadgetProp::Affine => true,
            _ => false,
        }
    }
}

/// Fullverif strategy for proving security of a gadget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GadgetStrat {
    Assumed,
    CompositeProp,
    Isolate,
    DeepVerif,
}

//TODO: when checking transitions: check that all gadgets are pipeline.

/// Structure of the evaluation of the gadget: Pipeline or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GadgetArch {
    Loopy,
    Pipeline,
}

impl StaticGadget {
    pub fn new(
        module: &super::module::Module,
        netlist: &Netlist,
        yosys_netlist: &yosys::Netlist,
    ) -> Result<Option<Self>> {
        let yosys_module = &yosys_netlist.modules[&module.name];
        let Some(order) = yosys_ext::get_int_module_attr(yosys_module, "fv_order")? else {
            return Ok(None);
        };
        let nshares = order;
        let prop = yosys_ext::get_str_module_attr_needed(yosys_module, "fv_prop")?.try_into()?;
        let strat = yosys_ext::get_str_module_attr_needed(yosys_module, "fv_strat")?.try_into()?;
        let arch = yosys_ext::get_str_module_attr_needed(yosys_module, "fv_arch")?.try_into()?;
        let connection_attrs = module
            .ports
            .iter()
            .map(|wire_name| yosys_ext::net_attributes(yosys_module, &wire_name.name, nshares))
            .collect::<Result<ConnectionVec<_>>>()?;
        let valid_latencies: ConnectionVec<_> = connection_attrs
            .iter()
            .map(|attrs| match attrs {
                yosys_ext::WireAttrs::Sharing { latency, .. } => vec![*latency],
                yosys_ext::WireAttrs::Random(latency) | yosys_ext::WireAttrs::Control(latency) => {
                    (*latency).into_iter().collect()
                }
                yosys_ext::WireAttrs::Clock => vec![],
            })
            .collect();
        let mut next_rnd_id = RndPortId::from_usize(0);
        let input_roles = module
            .input_ports
            .iter()
            .map(|con_id| {
                let wire_name = &module.ports[*con_id];
                Ok(match &connection_attrs[*con_id] {
                    yosys_ext::WireAttrs::Sharing { .. } => {
                        InputRole::Share(ShareId::from_raw(wire_name.offset as u32 % nshares))
                    }
                    yosys_ext::WireAttrs::Random(_) => {
                        let id = next_rnd_id;
                        next_rnd_id += 1;
                        InputRole::Random(id)
                    },
                    yosys_ext::WireAttrs::Control(_)  =>
                        InputRole::Control,
                    yosys_ext::WireAttrs::Clock => {
                            if module.clock.is_some_and(|clk| clk != *con_id) {
                                bail!("Input {wire_name} is denoted as clock, but another wire is used as clock in the module.");
                            }
                        InputRole::Control
                    }
                })
            })
            .collect::<Result<InputVec<_>>>()?;
        let output_roles = module
            .output_ports
            .iter()
            .map(|con_id| {
                let wire_name = &module.ports[*con_id];
                Ok(match &connection_attrs[*con_id] {
                    yosys_ext::WireAttrs::Sharing { .. } => {
                        OutputRole::Share(ShareId::from_raw(wire_name.offset as u32 % nshares))
                    }
                    yosys_ext::WireAttrs::Random(_) => {
                        bail!("An output wire cannot be randomness (wire {wire_name})")
                    }
                    yosys_ext::WireAttrs::Control(_) => OutputRole::Control,
                    yosys_ext::WireAttrs::Clock => {
                        bail!("An output wire cannot be clock (wire {wire_name})")
                    }
                })
            })
            .collect::<Result<OutputVec<_>>>()?;
        let max_latency = valid_latencies
            .iter()
            .flat_map(|x| x.iter().copied())
            .max()
            .unwrap_or(Latency::from_raw(0));
        let max_input_latency = module
            .input_ports
            .iter()
            .flat_map(|con_id| valid_latencies[*con_id].iter().copied())
            .max()
            .unwrap_or(Latency::from_raw(0));
        let rnd_ports = input_roles
            .iter_enumerated()
            .filter_map(|(id, input)| matches!(*input, InputRole::Random(_)).then_some(id))
            .collect::<RndPortVec<_>>();
        let res = Self {
            module_id: module.id,
            name: module.name.clone(),
            input_roles,
            output_roles,
            rnd_ports,
            valid_latencies,
            prop,
            strat,
            arch,
            nshares: order,
            max_latency,
            max_input_latency,
        };
        res.check_pipeline(netlist)?;
        Ok(Some(res))
    }
    pub fn latencies(&self) -> impl Iterator<Item = Latency> {
        (0..=self.max_latency.raw()).map(Latency::from_raw)
    }
    pub fn is_pipeline(&self) -> bool {
        self.arch == GadgetArch::Pipeline
    }
    pub fn check_pipeline(&self, netlist: &Netlist) -> Result<()> {
        if self.arch != GadgetArch::Pipeline {
            return Ok(());
        }
        let module = &netlist[self.module_id];
        for (output_id, output_role) in self.output_roles.iter_enumerated() {
            if !matches!(output_role, OutputRole::Share(_)) {
                bail!(
                    "Output {} is not a share.",
                    module.ports[module.output_ports[output_id]]
                );
            }
        }
        for (con_id, lats) in self.valid_latencies.iter_enumerated() {
            if Some(con_id) == module.clock {
                if !lats.is_empty() {
                    bail!("Clock signal cannot have a latency.");
                }
            } else if lats.len() != 1 {
                bail!(
                    "Pipeline gadget wire {} must have a single latency.",
                    &module.ports[con_id]
                );
            }
        }
        Ok(())
    }
}

mod yosys_ext {
    use super::Latency;
    use anyhow::{anyhow, bail, Context, Error, Result};
    use yosys_netlist_json as yosys;

    fn attr_display(attr: &yosys::AttributeVal) -> std::borrow::Cow<str> {
        match attr {
            yosys::AttributeVal::N(x) => format!("{x}").into(),
            yosys::AttributeVal::S(s) => s.into(),
        }
    }

    /// Convert an attribute to an u32 if possible
    fn attr2int(attr: &yosys::AttributeVal) -> Result<u32> {
        attr.to_number()
            .ok_or_else(|| anyhow!("Attribute is not a number: '{}'", attr_display(attr)))
            .and_then(|x| Ok(x.try_into()?))
    }
    pub fn get_int_module_attr(module: &yosys::Module, attr: &str) -> Result<Option<u32>> {
        Ok(if let Some(attr_v) = module.attributes.get(attr) {
            Some(
                attr2int(attr_v)
                    .with_context(|| format!("Parsing module attribute {attr} as u32."))?,
            )
        } else {
            None
        })
    }
    pub fn get_int_module_attr_needed(module: &yosys::Module, attr: &str) -> Result<u32> {
        get_int_module_attr(module, attr)?
            .ok_or_else(|| anyhow!("Missing module attribute {attr}."))
    }
    pub fn get_str_module_attr<'m>(
        module: &'m yosys::Module,
        attr: &str,
    ) -> Result<Option<&'m str>> {
        module
            .attributes
            .get(attr)
            .map(|val| match val {
                yosys::AttributeVal::S(v) => Ok(v.as_str()),
                yosys::AttributeVal::N(_) => {
                    bail!("Attribute {attr} is an integer, as string was expected.")
                }
            })
            .transpose()
    }
    pub fn get_str_module_attr_needed<'m>(
        module: &'m yosys::Module,
        attr: &str,
    ) -> Result<&'m str> {
        get_str_module_attr(module, attr)?
            .ok_or_else(|| anyhow!("Missing module attribute {attr}."))
    }
    fn get_int_wire_attr(module: &yosys::Module, netname: &str, attr: &str) -> Result<Option<u32>> {
        Ok(
            if let Some(attr_v) = module.netnames[netname].attributes.get(attr) {
                Some(attr2int(attr_v).with_context(|| {
                    format!("Parsing attribute {attr} of wire {netname} as u32.")
                })?)
            } else {
                None
            },
        )
    }
    fn get_int_wire_attr_needed<'a>(
        module: &'a yosys::Module,
        netname: &str,
        attr: &str,
    ) -> Result<u32> {
        get_int_wire_attr(module, netname, attr)?
            .ok_or_else(|| anyhow!("Missing attribute {attr} on wire {netname}."))
    }
    #[derive(Clone, Debug)]
    pub enum WireAttrs {
        Sharing { latency: super::Latency, count: u32 },
        Random(Option<Latency>),
        Control(Option<Latency>),
        Clock,
    }
    /// Get the type of a port.
    pub fn net_attributes<'a>(
        module: &'a yosys::Module,
        netname: &str,
        nshares: u32,
    ) -> Result<WireAttrs> {
        let net = &module.netnames[netname];
        let fv_type = net.attributes.get("fv_type");
        let fv_count = get_int_wire_attr(module, netname, "fv_count")?;
        Ok(
            match fv_type.and_then(yosys::AttributeVal::to_string_if_string) {
                Some("sharing") => {
                    let latency =
                        Latency::from_raw(get_int_wire_attr_needed(module, netname, "fv_latency")?);
                    let count = fv_count.unwrap_or(1);
                    assert_eq!(count * nshares, net.bits.len() as u32);
                    WireAttrs::Sharing { latency, count }
                }
                Some("random") => WireAttrs::Random(
                    get_int_wire_attr(module, netname, "fv_latency")?.map(Latency::from_raw),
                ),
                Some("control") => WireAttrs::Control(
                    get_int_wire_attr(module, netname, "fv_latency")?.map(Latency::from_raw),
                ),
                Some("clock") => WireAttrs::Clock,
                Some(s) => bail!("Unrecognized fv_type attribute '{s}' on wire {netname}."),
                None => bail!("Missing fv_type attribute on wire {netname}."),
            },
        )
    }
}

impl TryFrom<&str> for GadgetProp {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "_mux" => Self::Mux,
            "affine" => Self::Affine,
            "NI" => Self::NI,
            "SNI" => Self::SNI,
            "PINI" => Self::PINI,
            _ => bail!("{value} is not a known gadget security property."),
        })
    }
}

impl TryFrom<&str> for GadgetStrat {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "assumed" => Self::Assumed,
            "composite" => Self::CompositeProp,
            "isolate" => Self::Isolate,
            "deep_verif" => Self::DeepVerif,
            _ => bail!("{value} is not a known verification strategy."),
        })
    }
}

impl TryFrom<&str> for GadgetArch {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "loopy" => Self::Loopy,
            "pipeline" => Self::Pipeline,
            _ => bail!("{value} is not a known verification strategy."),
        })
    }
}
