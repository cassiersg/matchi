pub mod clk_vcd;
pub mod fv_cells;
pub mod module;
mod netlist;
pub mod recsim;
pub mod simulation;
pub mod vcd_writer;

use crate::type_utils::new_id;
new_id!(ModuleId, ModuleVec, ModuleSlice);

pub use netlist::Netlist;
