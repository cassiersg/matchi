(* fv_prop = "affine", fv_strat="isolate", fv_order=d *)
module MSKreg #(parameter d=1, parameter count=1) (clk, in, out);

(* fv_type = "clock" *)   input clk;
(* fv_type = "sharing", fv_latency = 0, fv_count=count *) input  [count*d-1:0] in;
(* fv_type = "sharing", fv_latency = 1, fv_count=count *) output [count*d-1:0] out;

reg [count*d-1:0] state;

always @(posedge clk)
    state <= in;

assign out = state;

endmodule
