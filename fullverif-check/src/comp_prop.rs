use crate::netlist;
use anyhow::{bail, Result};

/// Verifies that the gadget dataflow satisfies its claimed property.
pub fn check_sec_prop<'a, 'b>(gadget: &crate::tg_graph::GadgetFlow<'a, 'b>) -> Result<()> {
    match gadget.internals.gadget.prop {
        netlist::GadgetProp::Affine => {
            for sgi_name in gadget.gadget_names() {
                if !gadget.internals.subgadgets[&sgi_name.0]
                    .kind
                    .prop
                    .is_affine()
                {
                    bail!("Subgadget {:?} is not Affine", sgi_name);
                }
            }
        }
        netlist::GadgetProp::PINI => {
            for sgi_name in gadget.gadget_names() {
                if !gadget.internals.subgadgets[&sgi_name.0].kind.is_pini() {
                    bail!("Subgadget {:?} is not PINI", sgi_name);
                }
            }
        }
        _ => {
            unimplemented!();
        }
    }
    Ok(())
}
