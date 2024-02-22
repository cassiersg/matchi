module test(
    input wire clk,
    input wire rst,
    input wire in_valid,
    input wire in,
    output wire out
);

localparam B=3;
reg [B:0] state;

always @(posedge clk) begin
    if (rst) state <= (B+1)'b0;
    else state <= {state[1:B-1], in, 1'b0};
end

assign out = state[B];

endmodule
