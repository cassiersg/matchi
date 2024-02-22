`timescale 1ns/1ps
module tb
#
(
    parameter T = 2
)
();

localparam Td = T/2.0;

reg clk;
reg syn_rst;

reg dut_in_valid;
reg dut_in;
wire dut_out;

// Generate the clock
always #Td clk=~clk;


// Dut
test dut(
    .clk(clk),
    .rst(syn_rst),
    .in_valid(dut_in_valid),
    .in(dut_in),
    .out(dut_out)
);

initial begin
    `ifdef DUMPFILE
        // Open dumping file
        $dumpfile(`DUMPFILE);
        $dumpvars(0,tb);
    `endif

    dut_in = 1;
    clk = 1;
    dut_in_valid = 0;
    syn_rst = 1;
    #T;
    #T;
    syn_rst = 0;
    #T;
    dut_in_valid = 1;

    #(4*T);
    $finish;
end

endmodule
