use crate::gadget_internals::GadgetInternals;
use crate::gadgets::Latency;
use crate::utils::format_set;

use anyhow::{anyhow, bail, Context, Result};

use fnv::FnvHashMap as HashMap;
use std::fs::File;
use std::io::BufReader;

use yosys_netlist_json as yosys;

#[macro_use]
extern crate log;
#[macro_use]
extern crate derivative;

mod clk_vcd;
mod comp_prop;
mod composite_gadget;
mod config;
mod error;
mod gadget_internals;
mod gadgets;
mod inner_affine;
mod netlist;
mod raw_internals;
mod sim;
mod tg_graph;
#[macro_use]
mod type_utils;
mod utils;

use crate::composite_gadget::{GadgetId, GadgetLibrary};

// rnd_timings to map: port -> offsets for each cycle
fn abstract_rnd_timings<'a>(
    rnd_timings: impl Iterator<Item = &'a (gadgets::Random<'a>, Latency)>,
) -> Vec<HashMap<&'a str, Vec<usize>>> {
    let mut res = Vec::new();
    for (rnd, lat) in rnd_timings {
        if res.len() <= *lat as usize {
            res.resize(*lat as usize + 1, HashMap::default());
        }
        res[*lat as usize]
            .entry(rnd.port_name)
            .or_insert_with(Vec::new)
            .push(rnd.offset as usize);
    }
    res
}

/// Map the rnd_timings information to user-readable form.
fn rnd_timing_disp<'a>(
    rnd_timings: impl Iterator<Item = &'a (gadgets::Random<'a>, Latency)>,
) -> Vec<Vec<(&'a str, String)>> {
    abstract_rnd_timings(rnd_timings)
        .into_iter()
        .map(|t| {
            let mut res = t
                .into_iter()
                .map(|(rnd, offsets)| (rnd, crate::utils::format_set(offsets.into_iter())))
                .collect::<Vec<_>>();
            res.sort_unstable();
            res
        })
        .collect()
}

/// Verify that a gadgets satisfies all the rules
fn check_gadget<'a, 'b>(
    library: &'b GadgetLibrary<'a>,
    gadget_id: GadgetId,
    check_rnd_annot: bool,
    controls: &mut clk_vcd::ModuleControls,
    config: &config::Config,
) -> Result<Option<tg_graph::GadgetFlow<'a, 'b>>> {
    let gadget = &library.gadgets[gadget_id];
    match gadget.strat {
        netlist::GadgetStrat::Assumed => Ok(None),
        netlist::GadgetStrat::Isolate => {
            println!("Checking gadget {}...", gadget.name);
            if gadget.prop != netlist::GadgetProp::Affine {
                bail!("Invalid strategy 'isolate' for non-affine gadget");
            }
            inner_affine::check_inner_affine(gadget)?;
            let gg = raw_internals::GadgetGates::from_gadget(gadget)?;
            let ugg = gg.unroll(controls)?;
            ugg.check_outputs_valid(ugg.annotate_valid())?;
            println!("outputs valid");
            if !config.no_check_state_cleared {
                ugg.check_state_cleared(ugg.annotate_sensitive())?;
                println!("state cleared");
            }
            let _cg = ugg.computation_graph(ugg.annotate_sensitive());
            // Return None as there is no sub-gadget to check (we don't accept sub-gadgets in isolate strategy).
            Ok(None)
        }
        netlist::GadgetStrat::DeepVerif => {
            println!("Checking gadget {} (deep verif)...", gadget.name);
            if gadget.prop == netlist::GadgetProp::Affine {
                bail!("Invalid strategy 'deep_verif' for non-affine gadget");
            }
            let gg = raw_internals::GadgetGates::from_gadget(gadget)?;
            let ugg = gg.unroll(controls)?;
            ugg.check_outputs_valid(ugg.annotate_valid())?;
            println!("outputs valid");
            if !config.no_check_state_cleared {
                ugg.check_state_cleared(ugg.annotate_sensitive())?;
                println!("state cleared");
            }
            //let cg = ugg.computation_graph(ugg.annotate_sensitive());
            //deep_verif::check_deep_verif(&cg, gadget)?;
            unimplemented!("Deep verif is not implemented");
            // Return None as there is no sub-gadget to check (we don't accept sub-gadgets in deep_verif strategy).
            //Ok(None)
        }
        netlist::GadgetStrat::CompositeProp => {
            println!("Checking gadget {}...", gadget.name);
            assert_eq!(gadget.strat, netlist::GadgetStrat::CompositeProp);
            println!("computing internals...");
            let gadget_internals = GadgetInternals::<'a, 'b>::from_module(gadget, library)?;
            println!("internals computed");
            gadget_internals.check_sharings()?;
            println!("Sharings preserved: ok.");

            let n_simu_cycles = controls.len() as gadgets::Latency;
            let max_delay_output = gadget.max_output_lat();
            if !config.no_check_state_cleared {
                assert!(max_delay_output + 1 < n_simu_cycles);
            } else if max_delay_output + 1 > n_simu_cycles {
                println!(
                    "Error: not enough simulated cycles to simulate gadget {}.\nThis indicates \
                     that computation of this gadget is late with respect to the output shares. \
                     Skipping verification of this gadget.",
                    gadget.name
                );
                return Ok(None);
            }
            let n_analysis_cycles = if !config.no_check_state_cleared {
                max_delay_output + 2
            } else {
                max_delay_output + 1
            };
            println!(
                "Analyzing execution of the gadget over {} cycles (based on output latencies).",
                n_analysis_cycles
            );
            println!("Loaded simulation states.");
            println!("to graph...");
            let graph =
                tg_graph::GadgetFlow::new(gadget_internals.clone(), n_analysis_cycles, controls)?;
            if config.verbose {
                graph.disp_full();
            }
            println!("Valid gadgets:");
            let mut valid_gadgets: Vec<String> = graph
                .list_valid()
                .into_iter()
                .map(|(g, c)| format!("\t{}: {}", g, format_set(c.into_iter())))
                .collect();
            valid_gadgets.sort_unstable();
            for vg in valid_gadgets {
                println!("{}", vg);
            }
            println!("Sensitive gadgets:");
            for (g, c) in graph.list_sensitive(tg_graph::Sensitive::Yes) {
                println!("\t{}: {}", g, format_set(c.into_iter()));
            }
            println!("Glitch-sensitive gadgets:");
            for (g, c) in graph.list_sensitive(tg_graph::Sensitive::Glitch) {
                println!("\t{}: {}", g, format_set(c.into_iter()));
            }
            graph.check_valid_outputs()?;
            println!("Outputs valid: ok.");
            println!("Inputs exist.");
            for name in graph.warn_useless_rnd() {
                println!("Warning: the gadget {:?} does not perform valid computations, but it has sensitive inputs, hence requires randomness to not leak them. Consider muxing the sensitive inputs to avoid wasting randomness.", name);
            }
            let _rnd_times2 = graph.randoms_input_timing(controls)?;
            println!("Randoms timed");
            println!("rnd_times:");
            for (i, times) in rnd_timing_disp(_rnd_times2.keys()).into_iter().enumerate() {
                println!("Cycle {}:", i);
                for (rnd, offsets) in times {
                    println!("\t{}: {}", rnd, offsets);
                }
            }
            if check_rnd_annot {
                graph.check_randomness_usage(controls)?;
            }
            if !config.no_check_state_cleared {
                graph.check_state_cleared()?;
            }
            if !config.no_check_transitions {
                graph.check_parallel_seq_gadgets()?;
            }
            comp_prop::check_sec_prop(&graph)?;
            println!("check successful for gadget {}", gadget.name);
            Ok(Some(graph))
        }
    }
}

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
    let gadget_name = config.gname.as_str();
    let netlist_sim: sim::Netlist = netlist.try_into()?;
    let clk_path = signal_path(root_simu_mod.as_slice(), config.clk.as_str());
    let in_valid_path = signal_path(root_simu_mod.as_slice(), config.in_valid.as_str());
    let dut_path = signal_path(root_simu_mod.as_slice(), config.dut.as_str());

    println!("initializing sim vcd states...");
    let sim_vcd_states = sim::clk_vcd::VcdStates::new(&mut open_simu_vcd(config)?, &clk_path)?;
    println!("...initialized!");
    let sim_controls = sim::clk_vcd::ModuleControls::new(&sim_vcd_states, dut_path.clone(), 0);
    let n_cycles = sim_controls.len() as gadgets::Latency;
    // Simulation using sim::recsim
    if true {
        dbg!("Starting simu");
        let sim_controls = sim::clk_vcd::ModuleControls::new(&sim_vcd_states, dut_path.clone(), 0);
        let module_id = netlist_sim.id_of(gadget_name).unwrap();
        let evaluator = sim::recsim::InstanceEvaluator::new(module_id, &netlist_sim, vec![]);
        let mut sim_states = vec![];
        let mut sim_states_iter = evaluator.simu(sim_controls, &netlist_sim, false);
        let mut vcd_write_file =
            std::io::BufWriter::new(std::fs::File::create(&config.output_vcd)?);
        let mut vcd_writer = sim::vcd_writer::VcdWriter::new(
            &mut vcd_write_file,
            config.dut.clone(),
            netlist_sim.id_of(gadget_name).unwrap(),
            &netlist_sim,
            netlist,
        )?;
        for i in 0..(dbg!(n_cycles) - 1) {
            println!("Simu cycle {}/{}", i + 1, n_cycles);
            let new_state = sim_states_iter.next().unwrap()?;
            vcd_writer.new_state(&new_state)?;
            sim_states.push(new_state);
        }
        dbg!("Simu done");
    }

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
