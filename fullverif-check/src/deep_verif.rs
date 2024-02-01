use crate::error::{CompError, CompErrorKind};
use crate::gadgets::Gadget;
use crate::raw_internals::LeakComputationGraph;
use yosys_netlist_json as yosys;

/// Verify that the gadget is secure using deep verification.
pub fn check_deep_verif<'a>(
    cg: &LeakComputationGraph<'a>,
    gadget: &Gadget<'a>,
) -> Result<(), CompError<'a>> {
    //    println!("gadget: {:#?}", gadget);
    println!("cg: {:#?}", cg);
    todo!()
}
