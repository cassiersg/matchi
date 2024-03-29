use anyhow::{anyhow, Result};

use std::fs::File;
use std::io::BufReader;

use yosys_netlist_json as yosys;

#[macro_use]
extern crate log;

mod config;
#[macro_use]
mod type_utils;
mod clk_vcd;
mod gadget;
mod module;
mod netlist;
mod recsim;
mod share_set;
mod simulation;
mod top_sim;
mod vcd_writer;
mod wire_value;

use wire_value::WireValue;

new_id!(ModuleId, ModuleVec, ModuleSlice);

use netlist::Netlist;

/// Return the path of a signal in a module, splitting the signal name if needed.
fn signal_path(module: &[String], sig_name: &str) -> Vec<String> {
    module
        .iter()
        .cloned()
        .chain(sig_name.split('.').map(ToOwned::to_owned))
        .collect()
}

/// Verify that the top-level gadets (and all sub-gadgets) satisfy the rules.
fn check_gadget_top<'a>(netlist: &'a yosys::Netlist) -> Result<()> {
    println!("building netlist...");
    let gadget_name = config::config().gname.as_str();
    let netlist_sim = Netlist::new(netlist, gadget_name)?;
    let dut_path = signal_path(&[], config::config().dut.as_str());

    println!("initializing sim vcd states...");
    let mut vcd_file = open_simu_vcd()?;
    let vcd_parser = vcd::Parser::new(&mut vcd_file);
    // Simulation using recsim
    println!("Starting simu");
    let simulator = top_sim::Simulator::new(&netlist_sim, vcd_parser, &dut_path)?;
    let n_cycles = simulator.n_cycles();
    let mut sim_states_iter =
        simulator.simu(&netlist_sim, top_sim::GlobSimCycle::from_usize(n_cycles));
    let mut vcd_writer = config::config()
        .output_vcd
        .as_ref()
        .map(|fname| {
            let file = std::io::BufWriter::new(std::fs::File::create(fname)?);
            vcd_writer::VcdWriter::new(
                file,
                netlist_sim.top_gadget.module_id,
                &netlist_sim,
                netlist,
            )
        })
        .transpose()?;
    for i in 0.. {
        println!("Simu cycle {}/{}", i, n_cycles);
        let Some(iter) = sim_states_iter.next()? else {
            break;
        };
        sim_states_iter = iter;
        vcd_writer
            .as_mut()
            .map(|w| w.new_state(sim_states_iter.state()))
            .transpose()?;
        sim_states_iter.check()?;
    }
    println!("Verification successful.");
    Ok(())
}

fn open_simu_vcd() -> Result<std::io::BufReader<std::fs::File>> {
    let file_simu = File::open(&config::config().vcd).map_err(|_| {
        anyhow!(
            "Did not find the vcd file: '{}'.\nPlease check your testbench and simulator commands.",
            &config::config().vcd
        )
    })?;
    Ok(BufReader::new(file_simu))
}

pub fn main() -> Result<()> {
    let file_synth = File::open(&config::config().json).map_err(|_| {
        anyhow!(
            "Did not find the result of synthesis '{}'.",
            &config::config().json
        )
    })?;
    let file_synth = BufReader::new(file_synth);
    let netlist = yosys::Netlist::from_reader(file_synth)?;
    check_gadget_top(&netlist)?;
    Ok(())
}
