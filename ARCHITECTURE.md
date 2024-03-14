# Rust source architecture

The tool is essentially a symbolic simulator for structural netlists.
Its main inputs are the netlist, and a vcd file, from which we extract the
inputs of the top-level module (as well as some other control signal that are
metadata for the symbolic input values).

The top-level `main.rs` parses the command-line arguments (`config.rs`), opens the
netlist and input vcd file, then drives the simulation and writes the output
vcd file (`vcd_writer.rs`).

The simulation relies on two representations of the netlist.

First, a module representation (`module/`), which has nothing specific to
gadgets and masking. It represents modules as a composition of instances
(sub-modules, library gates `fv_cells.rs` - TODO rename, inputs) connected with
wires. The modules have input and output port (each port is a single-bit wide),
and there is a special treatment for the clock signal (we only support a single
clock signal). An important feature provided by the module representation is
the ordering of evaluation: within a single clock cycle, all gates must be
evaluated in an order that respects combinational dependencies, even in
presence of complicated dependencies across modules.

Then, the gadget representation (`gadget/`) incorporates domain-specific knowledge.
Each gadget corresponds to a module, but not all modules are gadgets.
There are two kinds of gadgets, pipeline gadgets and the top-level gadget.
The pipeline gadgets must have a strict pipeline architecture, and their
security property (PINI or OPINI) is assumed to hold (it is given as annotation
on the module), as well as the properties of their inputs and outputs
(share/random/control/clock, and pipeline stage of each port).
The top-level gadget is allowed to have any structure.
On this gadget, we have annotations to know which input/outputs are shares or
fresh randomness, and at which cycle they are valid.

The module and gadget representations are held together in the netlist (`netlist.rs`).

TODO: regroup in a `sim/` module ?
Next, the simulation starts with the parsing of the vcd file (`clk_vcd.rs`) to
get the value of each top-level input signal at all clock cycles (in addition
to any signal that describes validity information for these input signals).
The top-level simulator `top_sim.rs` generates the sequence of symbolic input
values and drives the simulation.

The symbolic simulation itself is implemented in `recsim.rs`, based on three
main datastructures.
The simulators, which are static and describe how to
perform the simulation (they are derived from the modules and gadgets,
exploiting the module dependency information to choose a (recursive) evaluation
order of all the instances).
Then, the simulation state represent the state of a module at a given clock cycle.
Finally, the global simulation state tracks information about the simulation
that is not confined to a single module (e.g., cycle count, which random bits
have been leaked, etc.).
There is a simulator/simulation state pair for each kind of instance: module
(including top-level gadget, in addition to the top-level simulator), pipeline
gadget, library gate, input, tie low/high.

The values in the symbolic simulation (`simulation.rs`, TODO rename) represent
the concrete value of the wire, whether it is deterministic (i.e., independent
of input shares and randomness), whether it has the same value as an input
randomness, and its sensitivity: of which secret share index may it depend
(`share_set.rs`) ? (with and without taking glitches into account).

Finally, other utilities: `wire_value.rs`: a 0/1 value, and `type_utils.rs`, to
work with a newtype pattern of index/vec/slice.
