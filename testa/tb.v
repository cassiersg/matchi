`timescale 1ns / 1ps

module tb();
  localparam d = 2;

  reg clk, rst, en;
  parameter HP = 5;         // Half Period
  parameter FP = 2*(HP);    // Full Period
  always #HP clk = ~clk;

  wire o_0, o_1;
  reg rnd_0;
  wire done;

  x #(2) dut(.clk(clk), .rst(rst), .en(en), .o_0(o_0), .o_1(o_1), .rng_0(rnd_0), .done(done));

  initial
  begin
    `ifdef DUMPFILE
        // Open dumping file
        $dumpfile(`DUMPFILE);
        $dumpvars(0,tb);
    `endif

    $display("Simulation started.");
    clk = 0;
    rst=0;
    en = 0;

    #FP;

    rst = 1;
    en = 0;


    #(3*FP);
    @(posedge clk);

    rst = 0;
    @(posedge clk);
    en = 1;
    rnd_0 = $random;

    @(posedge clk);
    #1;
    en = 0;

    @(posedge clk);
    #(4*FP);
    $display("Simulation finished.");
    $finish(); // Finish simulation.
  end
endmodule
