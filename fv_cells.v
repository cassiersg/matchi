module BUF(input wire A, output wire Y);
assign Y = A;
endmodule

module NOT(input wire A, output wire Y);
assign Y = ~A;
endmodule

module AND(input wire A, input wire B, output wire Y);
assign Y = (A & B);
endmodule

module NAND(input wire A, input wire B, output wire Y);
assign Y = ~(A & B);
endmodule

module OR(input wire A, input wire B, output wire Y);
assign Y = (A | B);
endmodule

module NOR(input wire A, input wire B, output wire Y);
assign Y = ~(A | B);
endmodule

module XOR(input wire A, input wire B, output wire Y);
assign Y = (A ^ B);
endmodule

module XNOR(input wire A, input wire B, output wire Y);
assign Y = ~(A ^ B);
endmodule

module MUX(input wire A, input wire B, input wire S, output wire Y);
assign Y = S ? B  :A;
endmodule

module DFF(input wire C, input wire D, output reg Q);
always @(posedge C) Q <= D;
endmodule
