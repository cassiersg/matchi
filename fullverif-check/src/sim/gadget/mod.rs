use super::module::OutputVec;
use crate::type_utils::new_id;
use crate::utils::ShareId;

use anyhow::{bail, Error, Result};

mod gadget_builder;
mod pipeline;
pub mod top;
mod yosys_ext;

pub use pipeline::PipelineGadget;
pub use top::TopGadget;

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

#[derive(Clone, Debug)]
pub enum PortRole {
    Share(ShareId),
    Random(RndPortId),
    Control, // includes clock
}

/// Fullverif security property for a module gadget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GadgetProp {
    Pini,
    // TODO: handle O-PINI gadgets.
    Opini,
}

/// Fullverif strategy for proving security of a gadget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GadgetStrat {
    Assumed,
    CompositeTop,
    CompositePipeline,
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

impl TryFrom<&str> for GadgetProp {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "PINI" => Self::Pini,
            "affine" | "OPINI" => Self::Opini,
            _ => bail!("{value} is not a known gadget security property."),
        })
    }
}

impl TryFrom<&str> for GadgetStrat {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        Ok(match value {
            "assumed" => Self::Assumed,
            "composite_top" => Self::CompositeTop,
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
