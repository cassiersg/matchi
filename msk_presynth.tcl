# Sample script for running yosys for fullVerif
# Invoked as
#  yosys -c msk_presynth.tcl
# Settings through environment variables:
# * IMPLEM_DIR: directory where all the implementation files are.
# We are looking in this directory for the main module and for all sub-modules.
# * MAIN_MODULE: name of the main module, which should be contained in the $MAIN_MODULE.v file.
# * FULLVERIF_LIB_DIR: directory containing the fullVerif *.v files.
# * OUT_DIR: directory where output files are written.

set IMPLEM_DIR $::env(IMPLEM_DIR)
set MAIN_MODULE $::env(MAIN_MODULE)
set FULLVERIF_LIB_DIR $::env(FULLVERIF_LIB_DIR)
set OUT_DIR $::env(OUT_DIR)

set MAIN_PATH $IMPLEM_DIR/$MAIN_MODULE.v
set TECHMAP_PATH $FULLVERIF_LIB_DIR/techmap.v
set LIB [file normalize [info script]]

# Read verilog, load sub-modules and build the hierarchy.
yosys verilog_defaults -add -I$IMPLEM_DIR -I$FULLVERIF_LIB_DIR
yosys read_verilog $MAIN_PATH
yosys hierarchy -check -libdir $IMPLEM_DIR -libdir $FULLVERIF_LIB_DIR -top $MAIN_MODULE;

# Remove verilog high-level constructs, in favor of netlists
yosys proc;
# Map yosys RTL library to yosys Gate library.
yosys techmap
# Map gates to our "fv_cells" library.
yosys dfflibmap -liberty $LIB/fv_lib.lib
yosys abc -liberty $LIB/fv_lib.lib

# Flatten all user-level module whose check strategy is 'flatten'
yosys setattr -mod -set keep_hierarchy 1 *;
yosys setattr -mod -unset keep_hierarchy A:fv_strat=flatten;
yosys flatten;

# Output the result in verilog (for simulation) and json (for analysis).
yosys write_json $OUT_DIR/${MAIN_MODULE}_synth.json

yosys write_verilog -norename $OUT_DIR/${MAIN_MODULE}_synth.v;
yosys write_verilog -noattr -norename $OUT_DIR/${MAIN_MODULE}_synth_noattr.v;
yosys write_verilog -attr2comment -norename $OUT_DIR/${MAIN_MODULE}_synth_attr2comment.v;
