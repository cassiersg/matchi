pub mod clk_vcd;
pub mod fv_cells;
pub mod gadget;
pub mod module;
mod netlist;
pub mod recsim;
pub mod simulation;
pub mod top_sim;
pub mod vcd_writer;

pub use module::WireValue;

use crate::type_utils::new_id;
new_id!(ModuleId, ModuleVec, ModuleSlice);

pub use netlist::Netlist;
