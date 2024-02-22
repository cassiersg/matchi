
WORK ?= work
SMAESH_ROOT ?= ./SMAesH

FULLVERIF ?= ../fullverif-check/target/debug/fullverif
YOSYS ?= yosys
IVERILOG ?= iverilog
VVP ?= vvp

HDL_ROOT_DIR=$(SMAESH_ROOT)/hdl
TB_MODULE=tb_aes_enc128_32bits_hpc2
TB_DIR=$(HDL_ROOT_DIR)/tb
TB_PATH=$(TB_DIR)/$(TB_MODULE).v
MAIN_MODULE=MSKaes_32bits_core
# signal starting the first simulation cycle (i.e. latency == 0 for the main module), name in the testbench
IN_VALID=dut.aes_valid_in
# clock signal (in the testbench)
CLOCK=clk
# name of the instance of the main module in the testbench
DUT=dut.aes_core

## workdir
HDL_DIR=$(WORK)/hdl
VCD_PATH=$(WORK)/a.vcd
SIM_PATH=$(WORK)/beh-simu
SYNTH_BASE=$(WORK)/${MAIN_MODULE}_synth
JSON=$(SYNTH_BASE).json

# tvs
BEH_SIMU_DIR=$(SMAESH_ROOT)/beh_simu
TV=$(BEH_SIMU_DIR)/tvs/generated/TV_ECBGFSbox128
TV_IN=$(TV)_in.rsp
TV_OUT=$(TV)_out.rsp

all: fullverif

$(JSON):
	-rm -r $(HDL_DIR)
	OUT_DIR=$(HDL_DIR) $(HDL_ROOT_DIR)/gather_sources.sh aes_enc128_32bits_hpc2
	OUT_DIR=$(WORK) MAIN_MODULE=$(MAIN_MODULE) IMPLEM_DIR=$(HDL_DIR) $(YOSYS) -c msk_presynth.tcl

$(VCD_PATH): $(JSON)
	make -C $(BEH_SIMU_DIR) tv
	$(IVERILOG) \
		-y $(HDL_DIR) \
		-y $(TB_DIR) \
		-I $(HDL_DIR) \
		-I $(TB_DIR) \
		-s $(TB_MODULE) \
		-o $(SIM_PATH) \
		-D TV_IN=\"$(TV_IN)\" \
		-D TV_OUT=\"$(TV_OUT)\" \
		-D DUMPFILE=\"$(VCD_PATH)\" \
		-D CORE_SYNTHESIZED=1 \
		-D RUN_AM=1 \
		-D FULLVERIF=1 \
		$(SYNTH_BASE).v $(TB_PATH)
	$(VVP) $(SIM_PATH)

fullverif: $(VCD_PATH) $(JSON)
	$(FULLVERIF) --json $(JSON) --vcd $(VCD_PATH) --tb $(TB_MODULE) --gname $(MAIN_MODULE) --in-valid $(IN_VALID) --clock $(CLOCK) --dut $(DUT) --output-vcd $(WORK)/out.vcd

.PHONY: fullverif
