
WORK ?= work

FULLVERIF ?= ../fullverif-check/target/debug/fullverif
YOSYS ?= yosys
IVERILOG ?= iverilog
VVP ?= vvp

TB_MODULE=tb
TB_PATH=$(TB_MODULE).v
MAIN_MODULE=test
# signal starting the first simulation cycle (i.e. latency == 0 for the main module), name in the testbench
IN_VALID=dut.in_valid
# clock signal (in the testbench)
CLOCK=clk
# name of the instance of the main module in the testbench
DUT=dut

## workdir
HDL_DIR=$(WORK)/hdl
VCD_PATH=$(WORK)/a.vcd
SIM_PATH=$(WORK)/beh-simu
SYNTH_BASE=$(WORK)/${MAIN_MODULE}_synth
JSON=$(SYNTH_BASE).json

# tvs
TV_IN=$(TV)_in.rsp
TV_OUT=$(TV)_out.rsp

all: fullverif

$(JSON):
	-rm -r $(HDL_DIR)
	mkdir -p $(HDL_DIR)
	cp test.v $(HDL_DIR)/test.v
	OUT_DIR=$(WORK) MAIN_MODULE=$(MAIN_MODULE) IMPLEM_DIR=$(HDL_DIR) $(YOSYS) -c msk_presynth.tcl

$(VCD_PATH): $(JSON)
	$(IVERILOG) \
		-y $(HDL_DIR) \
		-I $(HDL_DIR) \
		-s $(TB_MODULE) \
		-o $(SIM_PATH) \
		-D DUMPFILE=\"$(VCD_PATH)\" \
		-D FULLVERIF=1 \
		$(SYNTH_BASE).v $(TB_PATH)
	$(VVP) $(SIM_PATH)

fullverif: $(VCD_PATH) $(JSON)
	$(FULLVERIF) --json $(JSON) --vcd $(VCD_PATH) --tb $(TB_MODULE) --gname $(MAIN_MODULE) --in-valid $(IN_VALID) --clock $(CLOCK) --dut $(DUT) --output-vcd $(WORK)/out.vcd

.PHONY: fullverif
