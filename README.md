
# MATCHI

*Masking Analyzer for Trivially Composable Hardware Implementations.*

MATCHI is a tool to analyze security of masked circuits in the
glitch+transition probing model, at the composition
level: based on elementary gadgets that are assumed to be correct, this tool
checks that the larger composite circuit is secure by using a compositional
strategy based on PINI/OPINI (also known as hardware private circuits "HPC").

For more background on compositional strategies for masked hardware
implementations, see the papers
[1](https://eprint.iacr.org/2020/185)[2](https://doi.org/10.46586/tches.v2021.i2.136-158)
on which this tool is based.

## Principle

MATCHI is a field-specific symbolic simulator for synchronous circuits.
Taking as input the circuit netlist and the input signals, it simulates the
execution of the circuit, tracking for each wire properties such as the value,
dependency in input shares, randomness, etc.

This simulation requires some additional metadata about the circuit.
For the top-level gadget, MATCHI requires knowledge of the properties of the input and
output ports (share, random, etc.), and when the inputs are valid.
Then, for each non-trivial masked gadget, we also require annotations
about the input/output ports, and the security property achieved (typically,
only very few gadgets require this, e.g. only the AND gadget).

It can then detect violation of security properties.

In practice, MATCHI takes the input netlist in Yosys's JSON format (this does
not force the usage of Yosys for synthesis: it can be used simply as a netlist
format conversion tool) and the input signal values as a vcd file.
The annotations are parsed from attributes on the modules/wires/ports in the
netlist.

## Usage

**Build dependencies**:
- rust development toolchain (rustc, cargo) (tested with `1.74`) <https://rustup.rs/>

**Usage dependencies**:
- `yosys >= 0.11` <http://www.clifford.at/yosys/>
- a verilog simulator (we test with `iverilog 11.0`)
### Build

```sh
git clone https://github.com/cassiersg/matchi.git
cd matchi/matchi
cargo build --release
```

### Test

TODO

### Constraints

- The circuit must be use sequential logic using a single clock and posedge
synchronous-reset DFFs.
- Inout ports, multi-driver nets and `z` (high-impedence) values are not supported.

### Verilog source verification

Starting from a verilog implementation, the first step is to perform a
synthesis (the leakage model applies to netlists, not to behavioral models).
This synthesis should synthesize to the provided
`lib_matchi.lib`/`lib_matchi.v`, which is a very simple library aimed at
simplifying verification.
An example yosys synthesis script is provided under `synth.v`.

As a second step, a vcd for the circuit should be produced with a simulation.
Since only the inputs of the top level circuit (and some of its wires, if used in leakage annotations) are needed, the simulation can be performed using either the behavioral files or the synthesized netlist.

Finally, the MATCHI can be run. The typical invocation is:
```sh
matchi/target/release/matchi --json path/to/yosys/output.json --vcd path/to/simu.vcd --dut tb.dut --gname top_level_gadget
```
where `tb.dut` is the dot-separated path to the top-level gadget in the vcd,
and `top_level_gadget` is the name of the corresponding module in the netlist.

Other options are given by `matchi/target/release/matchi --help`.

### Output vcd

MATCHI outputs a vcd file that contains multiple top-level scopes:
- `value` is a normal simulation, it should match other HDL simulators
(remark: clock signals are wrong).
- `deterministic` is 1 if the value of the net does not depend on input shares or randomness, otherwise it is 0.
- `random` is 1 when the value of the net is equal to the value of a fresh input random bit
- `share_i` is 1 when the net value is sensitive for share index `i`
- `matchi_debug_mod` contains MATCHI-generated signals, including a correct clock signal and a simulation cycle counter (that matches cycle counts given in the error messages).


### Top-level annotations

Annotations are given as verilog attributes on the top-level module and on its
input/output ports.

On the module, the following attributes are required:
```
(* matchi_prop=PINI, matchi_strat=composite_top, matchi_arch=loopy, matchi_shares=2 *)
```
(The number of shares can be adjusted.)

The `matchi_type` attribute must be given for each port, its value one of the following:

- `"clock"` for the clock signal (there must be exactly one such port, with width of 1 bit),
- `"random"` for an input port of fresh randomness,
- `"sharings_dense"` for a bus of sharings where, assuming that the circuit has
`matchi_shares=d`, the first `d` bits form one sharing: they are `d` shares of
one value, the next `d` bits form another sharing, etc.
- `"sharings_strided"` for a bus of sharings of width `k*d` there the first `k`
bits are all the first share of `k` distinct sharings, the next `k` bit are the
second shares, etc.
- `"share"` for a bus of shares, the integer-valued (in `{0, ..., d-1}`)
`matchi_share` attribute must be provided (e.g., if the shares are all the
first shares of their sharing, `matchi_share=0`).
- `"control"` for deterministic input/output value (reset, valid signals, etc.)

Remark: if the shares are not freshly generated, the order of the shares
matters (see [1](https://eprint.iacr.org/2020/185)): do not shuffle the order
of the shares.

All the sharing/share and random ports must additionally provide activity
information through the `matchi_active` attribute. The value is the name of a (single-bit) net
in the module, e.g., `matchi_active=input_valid`. This net must be included in the
vcd, and its value is used to determine the symbolic properties of the port.
For all clock cycles where the `matchi_active` net is `1`:

- if the port is a `"random"`, then we assume that its value is a fresh uniform
random value that will be observed on the port only for this clock cycle
(otherwise, we do not assume any property on the value of the port, only that
it is not a sensitive value).
- if the port is an input port with share/sharing type, we assume that its
value is a sensitive share
- if the port is an output port with share/sharing type, we verify that only in
these cycles is the value of the port sensitive.

By "sensitive", we mean that the value depends of a secret (masked) value.

### Other gadgets

For most modules/gadgets in the circuit, MATCHI does not require any
annotation, including for all share-wise gadgets (these do actually not need to
be isolated in a separate module).
We however require annotations for gadgets which are are not share-wise nor the
composition of other gadgets, such as a masked AND gadget (or other non-linear
gadgets).

These annotated gadgets must have a fully pipeline structure all their logic
(including shares, control and randomness) must be separated into pipeline
stages. Input and outputs can be connected to arbitrary stages.

These gadget have the following top-level annotation:
```
(* matchi_prop = "PINI", matchi_strat = "assumed", matchi_shares=2, matchi_arch="pipeline" *)
```
where the `matchi_shares` should be adjusted, and the `matchi_prop` is either `PINI` or `OPINI`.

Each port must be annotated with a `matchi_type`, with the same value and
meaning as for the top-level gadget.
Further, the pipeline stage information must be given for all ports (except for
`"clock"`), with the `matchi_latency` attribute.
The pipeline stage should be an integer, with ports in the first stage of the
pipeline having `matchi_latency=0`.

Remark: MATCHI performs only a very shallow analysis of the pipeline gadgets,
and as a result it uses a worst-case assumption that, within a single pipeline
stage, each output port has a combinational dependency on all the input ports
in that stage. This may create spurious detection of combinational loops, which
can be fixed in many case by splitting up the gadget into smaller gadgets.


### Testbench

The testbench must exercise a standard behavior of one execution of the main
module, which it should instantiate.
There should be a startup signal (whose name can be set in the `main.sh`
script) that should raise to `1` to signal the cycle '0' w.r.t. the latency
annotations in the main module.
Name of the main module instance and clock signal can be set in `main.sh`.

The input sharings and randomness are not important (can be any value,
including `x` or `z`).

## Gadget libary

TODO

## Bugs, contributing, etc.

MATCHI is an actively maintained software.
You are welcome to report bugs, submit enhancements proposals or code on
github, or by e-mail (to the contact author of the papers linked above).

## License

The MATCHI tool is distributed under the terms of the GPL3 license.

See [LICENSE-GPL3](LICENSE-GPL3) and [COPYRIGHT](COPYRIGHT) for details.


## MATCHI code overview

See <ARCHITECTURE.md>.


## Performance

We aim at keeping MATCHI fairly fast to run, which concretely means a
computation time proportional to the size of the circuit and to the number of
simulated clock cycles.

There is a number of high-gain performance optimizations that have not been implemented.

