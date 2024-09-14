(* matchi_prop = "PINI", matchi_strat = "composite_top", matchi_arch="loopy", matchi_shares=2, matchi_active="active" *)
module x #(parameter d=2) (clk, rst, en, o_0, o_1, rng_0, done);
(* matchi_type = "clock" *) input clk;
(* matchi_type = "control" *) input rst;
(* matchi_type = "control" *) input en;
(* matchi_type = "share", matchi_active="done", matchi_share = 0 *) output o_0;
(* matchi_type = "share", matchi_active="done", matchi_share = 1 *) output o_1;
(* matchi_type = "random", matchi_active="active", matchi_count = 1, matchi_rnd_count = 64, matchi_rnd_lat_0 = 0 *) input rng_0;
(* matchi_type = "control" *) output reg done;

(* keep *) wire active = 1'b1; // FIXME: set this to 1 only when fresh randomness is actually supplied.
  reg en_d, en_d;
  always @(posedge clk) begin
      en_d <= en;
      done <= en_d;
  end

reg z, r_d, r_dd, r_ddd, r_dddd;
always @(posedge clk) begin
    r_d <= rng_0;
    r_dd <= r_d;
    r_ddd <= r_dd ^ z;
    r_dddd <= r_ddd;
    z <= 0;
end

assign o_0[0] = r_dddd;
assign o_1[0] = 0;

endmodule
