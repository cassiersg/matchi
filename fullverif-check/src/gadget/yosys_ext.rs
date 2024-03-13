use super::Latency;
use crate::share_set::ShareId;
use anyhow::{anyhow, bail, Context, Result};
use fnv::FnvHashMap as HashMap;
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
fn attr2str<'m>(attr: &'m yosys::AttributeVal, name: &str) -> Result<&'m str> {
    match attr {
        yosys::AttributeVal::S(v) => Ok(v.as_str()),
        yosys::AttributeVal::N(_) => {
            bail!("Attribute {name} is an integer, as string was expected.")
        }
    }
}
pub fn get_int_module_attr(module: &yosys::Module, attr: &str) -> Result<Option<u32>> {
    Ok(if let Some(attr_v) = module.attributes.get(attr) {
        Some(attr2int(attr_v).with_context(|| format!("Parsing module attribute {attr} as u32."))?)
    } else {
        None
    })
}
pub fn get_str_module_attr<'m>(module: &'m yosys::Module, attr: &str) -> Result<Option<&'m str>> {
    module
        .attributes
        .get(attr)
        .map(|val| attr2str(val, attr))
        .transpose()
}
pub fn get_int_wire_attr(module: &yosys::Module, netname: &str, attr: &str) -> Result<Option<u32>> {
    Ok(
        if let Some(attr_v) = module.netnames[netname].attributes.get(attr) {
            Some(
                attr2int(attr_v).with_context(|| {
                    format!("Parsing attribute {attr} of wire {netname} as u32.")
                })?,
            )
        } else {
            None
        },
    )
}
pub fn get_int_wire_attr_needed(module: &yosys::Module, netname: &str, attr: &str) -> Result<u32> {
    get_int_wire_attr(module, netname, attr)?
        .ok_or_else(|| anyhow!("Missing attribute {attr} on wire {netname}."))
}
pub fn get_str_wire_attr<'m>(
    module: &'m yosys::Module,
    netname: &str,
    attr: &str,
) -> Result<Option<&'m str>> {
    module.netnames[netname]
        .attributes
        .get(attr)
        .map(|val| attr2str(val, attr))
        .transpose()
}
pub fn get_str_wire_attr_needed<'m>(
    module: &'m yosys::Module,
    netname: &str,
    attr: &str,
) -> Result<&'m str> {
    get_str_wire_attr(module, netname, attr)?
        .ok_or_else(|| anyhow!("Missing attribute {attr} on wire {netname}."))
}

#[derive(Clone, Debug)]
pub enum PortKind {
    SharingsDense,
    SharingsStrided { stride: u32 },
    Share { share_id: ShareId },
    Random,
    Control,
    Clock,
}
impl PortKind {
    fn new(module: &yosys::Module, netname: &str, nshares: u32) -> Result<Self> {
        let net = &module.netnames[netname];
        let fv_type = net.attributes.get("fv_type");
        let check_port_width = || {
            let port_width: u32 = net.bits.len().try_into().unwrap();
            if port_width % nshares == 0 {
                Ok(port_width)
            } else {
                bail!("Port is a sharing, but its width is not a multiple of the number of shares.")
            }
        };
        Ok(
            match fv_type.and_then(yosys::AttributeVal::to_string_if_string) {
                // TODO: remove this, we keep it here for backcompat.
                Some("sharing") | Some("sharings_dense") => {
                    check_port_width()?;
                    PortKind::SharingsDense
                }
                Some("sharings_strided") => PortKind::SharingsStrided {
                    stride: check_port_width()? / nshares,
                },
                Some("share") => {
                    let share_id =
                        ShareId::from_raw(get_int_wire_attr_needed(module, netname, "fv_share")?);
                    PortKind::Share { share_id }
                }
                Some("random") => PortKind::Random,
                Some("control") => PortKind::Control,
                Some("clock") => {
                    if net.bits.len() != 1 {
                        bail!("Clock port has width != 1.");
                    }
                    PortKind::Clock
                }
                Some(s) => bail!("Unrecognized fv_type attribute '{s}' on wire {netname}."),
                None => bail!("Missing fv_type attribute on wire {netname}."),
            },
        )
    }
}

#[derive(Clone, Debug)]
pub struct ModulePortKinds<'a> {
    kinds: HashMap<&'a str, PortKind>,
    nshares: u32,
}
impl<'a> ModulePortKinds<'a> {
    pub fn new(module: &'a yosys::Module, nshares: u32) -> Result<Self> {
        Ok(Self {
            kinds: module
                .ports
                .keys()
                .map(|netname| {
                    PortKind::new(module, netname.as_str(), nshares)
                        .with_context(|| format!("Failed annotation analysis of port {}.", netname))
                        .map(|kind| (netname.as_str(), kind))
                })
                .collect::<Result<_>>()?,
            nshares,
        })
    }
    pub fn share_id(&self, netname: &str, offset: u32) -> ShareId {
        match &self.kinds[netname] {
            PortKind::SharingsDense => ShareId::from_raw(offset % self.nshares),
            PortKind::SharingsStrided { stride } => ShareId::from_raw(offset / stride),
            PortKind::Share { share_id } => *share_id,
            PortKind::Random | PortKind::Control | PortKind::Clock => panic!("Not a share."),
        }
    }
    pub fn is_share(&self, netname: &str) -> bool {
        matches!(
            self.kinds[netname],
            PortKind::SharingsDense | PortKind::SharingsStrided { .. } | PortKind::Share { .. }
        )
    }
    pub fn is_random(&self, netname: &str) -> bool {
        matches!(self.kinds[netname], PortKind::Random,)
    }
    pub fn is_control(&self, netname: &str) -> bool {
        matches!(self.kinds[netname], PortKind::Control,)
    }
    pub fn is_clock(&self, netname: &str) -> bool {
        matches!(self.kinds[netname], PortKind::Clock,)
    }
}
pub fn wire_latency(module: &yosys::Module, netname: &str) -> Result<Latency> {
    Ok(Latency::from_raw(
        get_int_wire_attr_needed(module, netname, "fv_latency")
            .with_context(|| format!("Couln't get latency annotation for wire {}.", netname))?,
    ))
}
#[derive(Debug, Clone)]
pub struct GadgetAttrs {
    pub arch: super::GadgetArch,
    pub nshares: u32,
    pub prop: super::GadgetProp,
    pub strat: super::GadgetStrat,
    pub exec_active: Option<String>,
}

impl GadgetAttrs {
    pub fn new(yosys_module: &yosys::Module) -> Result<Option<Self>> {
        match (
        get_str_module_attr(yosys_module, "fv_arch")?,
        get_int_module_attr(yosys_module, "fv_order")?,
        get_str_module_attr(yosys_module, "fv_prop")?,
        get_str_module_attr(yosys_module, "fv_strat")?
            ) {
                (Some(arch), Some(nshares), Some(prop), Some(strat)) => {
                    Ok(Some(Self{
                        arch: arch.try_into()?,
                        nshares,
                        prop: prop.try_into()?,
                        strat: strat.try_into()?,
                        exec_active: get_str_module_attr(yosys_module, "fv_active")?.map(ToOwned::to_owned),
                    }))
                }
                (None, None, None, None) => Ok(None),
                _ => bail!("All (or none) of 'fv_arch', 'fv_order', 'fv_prop' and 'fv_strat' module attributes must be given.")
            }
    }
}
