//! Command-line parsing for the app.

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Config {
    #[arg(long)]
    /// Path to synthesized json file from Yosys.
    pub json: String,
    #[arg(long)]
    /// Path to simulation vcd file.
    pub vcd: String,
    #[arg(long)]
    /// Testbench module name.
    pub tb: String,
    #[arg(long)]
    /// Main gadget module name.
    pub gname: String,
    #[arg(long)]
    /// Name of the in_valid signal in the testbench.
    pub in_valid: String,
    #[arg(long)]
    /// Name of the DUT instance in the testbench.
    pub dut: String,
    #[arg(long("clock"))]
    /// Name of the clock signal in the testbench.
    pub clk: String,
    #[arg(long)]
    /// Do not check transition leakage.
    pub output_vcd: String,
    #[arg(long)]
    /// Do not check for the presence of remaining secrets after the execution.
    pub no_check_state_cleared: bool,
    #[arg(long)]
    /// Do not check transition leakage.
    pub no_check_transitions: bool,
    #[arg(short, long)]
    /// More verbose output.
    pub verbose: bool,
}

pub fn parse_cmd_line() -> Config {
    Config::parse()
}
