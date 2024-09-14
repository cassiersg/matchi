
WORK ?= work
SMAESH_ROOT ?= ./SMAesH

MATCHI ?= ../matchi/target/debug/matchi
YOSYS ?= yosys
IVERILOG ?= iverilog
VVP ?= vvp

HDL_ROOT_DIR=$(SMAESH_ROOT)/hdl
TB_DIR=.
TB_MODULE=tb
TB_PATH=$(TB_MODULE).v
MAIN_MODULE=x
# name of the instance of the main module in the testbench
DUT=dut

## workdir
HDL_DIR=$(WORK)/hdl
VCD_PATH=$(WORK)/a.vcd
SIM_PATH=$(WORK)/beh-simu
SYNTH_BASE=$(WORK)/${MAIN_MODULE}_synth
JSON=$(SYNTH_BASE).json

all: matchi

$(JSON):
	-rm -r $(HDL_DIR)
	mkdir -p $(HDL_DIR)
	cp x.v $(HDL_DIR)/
	OUT_DIR=$(WORK) MAIN_MODULE=$(MAIN_MODULE) IMPLEM_DIR=$(HDL_DIR) $(YOSYS) -c ../synth.tcl

$(VCD_PATH): $(JSON)
	$(IVERILOG) \
		-y $(HDL_DIR) \
		-y $(TB_DIR) \
		-I $(HDL_DIR) \
		-I $(TB_DIR) \
		-s $(TB_MODULE) \
		-o $(SIM_PATH) \
		-D DUMPFILE=\"$(VCD_PATH)\" \
		-D CORE_SYNTHESIZED=1 \
		-D RUN_AM=1 \
		-D MATCHI=1 \
		$(SYNTH_BASE).v $(TB_PATH)
	$(VVP) $(SIM_PATH)

matchi: $(VCD_PATH) $(JSON)
	$(MATCHI) --json $(JSON) --vcd $(VCD_PATH) --gname $(MAIN_MODULE) --dut $(TB_MODULE).$(DUT) --output-vcd $(WORK)/out.vcd

.PHONY: matchi 
