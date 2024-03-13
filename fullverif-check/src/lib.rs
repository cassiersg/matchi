use anyhow::{anyhow, Result};

use std::fs::File;
use std::io::BufReader;

use yosys_netlist_json as yosys;

#[macro_use]
extern crate log;

mod config;
mod sim;
#[macro_use]
mod type_utils;
mod utils;

/// Return the path of a signal in a module, splitting the signal name if needed.
fn signal_path(module: &[String], sig_name: &str) -> Vec<String> {
    module
        .iter()
        .cloned()
        .chain(sig_name.split('.').map(ToOwned::to_owned))
        .collect()
}

/// Verify that the top-level gadets (and all sub-gadgets) satisfy the rules.
fn check_gadget_top<'a>(
    netlist: &'a yosys::Netlist,
    root_simu_mod: Vec<String>,
    config: &'a config::Config,
) -> Result<()> {
    println!("building netlist...");
    let gadget_name = config.gname.as_str();
    let netlist_sim = sim::Netlist::new(netlist, gadget_name)?;
    let dut_path = signal_path(root_simu_mod.as_slice(), config.dut.as_str());

    println!("initializing sim vcd states...");
    let mut vcd_file = open_simu_vcd(config)?;
    let vcd_parser = vcd::Parser::new(&mut vcd_file);
    // Simulation using sim::recsim
    if true {
        println!("Starting simu");
        let simulator = sim::top_sim::Simulator::new(&netlist_sim, vcd_parser, &dut_path)?;
        let n_cycles = simulator.n_cycles();
        let mut sim_states_iter = simulator.simu(
            &netlist_sim,
            sim::top_sim::GlobSimCycle::from_usize(n_cycles),
        );
        let mut vcd_write_file =
            std::io::BufWriter::new(std::fs::File::create(&config.output_vcd)?);
        let mut vcd_writer = sim::vcd_writer::VcdWriter::new(
            &mut vcd_write_file,
            netlist_sim.id_of(gadget_name).unwrap(),
            &netlist_sim,
            netlist,
        )?;
        for i in 0.. {
            println!("Simu cycle {}/{}", i, n_cycles);
            let Some(iter) = sim_states_iter.next()? else {
                break;
            };
            sim_states_iter = iter;
            vcd_writer.new_state(sim_states_iter.state())?;
            sim_states_iter.check()?;
        }
        dbg!("Simu done");
    }
    return Ok(());

    /*
    let library: crate::composite_gadget::GadgetLibrary = netlist.try_into()?;
    println!("initializing vcd states...");
    let vcd_states = clk_vcd::VcdStates::new(&mut open_simu_vcd(config)?, &clk_path)?;
    println!("...initialized!");

    let cycle_count_path = signal_path(root_simu_mod.as_slice(), "cycle_count");
    let _ = vcd_states.get_var_id(&cycle_count_path).map(|id| {
        for i in 0..vcd_states.len() {
            debug!("cycle_count[{}] = {:?}", i, vcd_states.get_var(id, i));
        }
    });

    let mut controls = clk_vcd::ModuleControls::from_enable(&vcd_states, dut_path, &in_valid_path)?;
    let n_cycles = controls.len() as gadgets::Latency;

    let max_delay_output = if let Some(g) = library.get(gadget_name) {
        g.max_output_lat()
    } else {
        bail!(format!(
            "Cannot find gadget {} in the netlist. Does it have the fv_prop annotation ?",
            gadget_name
        ));
    };
    if (max_delay_output + 1 > n_cycles)
        || (max_delay_output + 1 >= n_cycles && !config.no_check_state_cleared)
    {
        bail!(format!(
            "Not enough simulated cycles to check the top-level gadget.\n\
                 Note: number of simulated cycles should be at least maximum output delay{}.\n\
                 Note: max_out_delay: {}, n_cycles: {}.",
            if !config.no_check_state_cleared {
                " + 2 (since we are checking if state is cleared after last output)"
            } else {
                " + 1"
            },
            max_delay_output,
            n_cycles
        ));
    }

    let g_graph = check_gadget(
        &library,
        library.id_of(gadget_name).unwrap(),
        false,
        &mut controls,
        config,
    )
    .with_context(|| format!("Checking gadget {}", gadget_name))?;
    let g_graph = if let Some(x) = g_graph {
        x
    } else {
        println!("Gadget is assumed to be correct");
        return Ok(());
    };

    let mut gadgets_to_check: Vec<(&str, _)> = Vec::new();
    // FIXME Should also check "only glitch" gadgets
    for ((name, cycle), base) in g_graph.sensitive_stable_gadgets() {
        let gadget_name = base.kind.name;
        let controls = controls.submodule(name.to_owned(), cycle as usize);
        gadgets_to_check.push((gadget_name, controls));
    }
    let mut gadgets_checked: HashMap<&str, Vec<clk_vcd::StateLookups>> = HashMap::default();
    while let Some((sg_name, mut sg_controls)) = gadgets_to_check.pop() {
        // Check if one of our state lookups matches the current state.
        let mut gadget_ok = false;
        for state_lookups in gadgets_checked.get(&sg_name).unwrap_or(&Vec::new()) {
            if state_lookups.iter().all(|((path, cycle, idx), state)| {
                &sg_controls.lookup(path.clone(), *cycle, *idx).unwrap() == state
            }) {
                gadget_ok = true;
                break;
            }
        }
        if gadget_ok {
            //println!("Gadget {} already checked, skipping.", sg_name);
            continue;
        }

        // Actual verification of the gadget.
        let ur_sg = check_gadget(
            &library,
            library.id_of(sg_name).unwrap(),
            true,
            &mut sg_controls,
            config,
        )
        .with_context(|| format!("Checking gadget {}", sg_name))?;

        // Add sub-gadgest to "to be checked" list.
        if let Some(ur_sg) = ur_sg {
            // FIXME Should also check "only glitch" gadgets
            for ((name, cycle), base) in ur_sg.sensitive_stable_gadgets() {
                gadgets_to_check.push((
                    base.kind.name,
                    sg_controls.submodule(name.to_owned(), cycle as usize),
                ));
            }
        }

        // Cache verification result.
        gadgets_checked
            .entry(sg_name)
            .or_insert_with(Vec::new)
            .push(sg_controls.lookups());
    }
    Ok(())
        */
}

fn open_simu_vcd(config: &config::Config) -> Result<std::io::BufReader<std::fs::File>> {
    let file_simu = File::open(&config.vcd).map_err(|_| {
        anyhow!(
            "Did not find the vcd file: '{}'.\nPlease check your testbench and simulator commands.",
            &config.vcd
        )
    })?;
    Ok(BufReader::new(file_simu))
}

pub fn main() -> Result<()> {
    let config = config::parse_cmd_line();
    let file_synth = File::open(&config.json)
        .map_err(|_| anyhow!("Did not find the result of synthesis '{}'.", &config.json))?;
    let file_synth = BufReader::new(file_synth);
    let netlist = yosys::Netlist::from_reader(file_synth)?;
    let root_simu_mod = signal_path(&[], config.tb.as_str());
    check_gadget_top(&netlist, root_simu_mod, &config)?;
    Ok(())
}
