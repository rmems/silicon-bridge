// SPDX-License-Identifier: MIT OR Apache-2.0
// tb_readmemh_contract.sv
//
// Simulation evidence for silicon-bridge #53. Instantiates the vendored
// silicon-hdl WeightRam (unsigned `logic [15:0]` storage) and inspects
// $readmemh addressing plus the signed accumulate / sign-extend idiom used
// by LifNeuron / OutputLayer (GH#73). File storage type alone does not
// settle the arithmetic.
//
// Peer HDL pin: rmems/silicon-hdl@d45163f38ac1cd88f8a3918e3793a08ace85e132
// This is HDL simulation evidence, not board-measured parity.

`timescale 1ns/1ps

module tb_readmemh_contract;

    localparam int DATA_WIDTH = 16;
    localparam int CLK_PERIOD = 10;

    logic clk;
    logic rst_n;
    int   errors;

    // Generic 4×6 hidden weights: 24 words fit ADDR_WIDTH=5 (32 entries).
    WeightRam #(
        .ADDR_WIDTH (5),
        .DATA_WIDTH (DATA_WIDTH),
        .INIT_FILE  ("tests/golden/generic_4x6/parameters_weights.mem")
    ) u_generic (
        .clk   (clk),
        .rst_n (rst_n),
        .we    (1'b0),
        .addr  ('0),
        .din   ('0),
        .dout  ()
    );

    // Synthetic 16×16 hidden weights: 256 words, ADDR_WIDTH=8.
    WeightRam #(
        .ADDR_WIDTH (8),
        .DATA_WIDTH (DATA_WIDTH),
        .INIT_FILE  ("tests/golden/spikenaut_16/parameters_weights.mem")
    ) u_hidden (
        .clk   (clk),
        .rst_n (rst_n),
        .we    (1'b0),
        .addr  ('0),
        .din   ('0),
        .dout  ()
    );

    // Exporter K×N readout (class-major). Not what OutputLayer indexes.
    WeightRam #(
        .ADDR_WIDTH (6),
        .DATA_WIDTH (DATA_WIDTH),
        .INIT_FILE  ("tests/golden/spikenaut_16/parameters_output_weights.mem")
    ) u_readout_kxn (
        .clk   (clk),
        .rst_n (rst_n),
        .we    (1'b0),
        .addr  ('0),
        .din   ('0),
        .dout  ()
    );

    // Consumer N×K readout (neuron * NUM_CLASSES + class).
    WeightRam #(
        .ADDR_WIDTH (6),
        .DATA_WIDTH (DATA_WIDTH),
        .INIT_FILE  ("tests/golden/spikenaut_16/hdl_readout_neuron_major.mem")
    ) u_readout_nxk (
        .clk   (clk),
        .rst_n (rst_n),
        .we    (1'b0),
        .addr  ('0),
        .din   ('0),
        .dout  ()
    );

    initial clk = 1'b0;
    always #(CLK_PERIOD/2) clk = ~clk;

    // LifNeuron / OutputLayer idiom: sign-extend both operands to a guard
    // width, add, saturate if the guard bit and sign bit disagree.
    function automatic logic signed [15:0] sat_add(
        input logic signed [15:0] acc,
        input logic signed [15:0] word
    );
        logic signed [16:0] acc_wide;
        logic signed [16:0] word_wide;
        logic signed [16:0] sum_wide;
        begin
            acc_wide  = acc;
            word_wide = word;
            sum_wide  = acc_wide + word_wide;
            if (sum_wide[16] != sum_wide[15])
                sat_add = sum_wide[16] ? 16'h8000 : 16'h7FFF;
            else
                sat_add = sum_wide[15:0];
        end
    endfunction

    task automatic check(input bit cond, input string msg);
        if (!cond) begin
            errors = errors + 1;
            $display("FAIL: %s", msg);
        end
    endtask

    task automatic check_hex(
        input logic [15:0] actual,
        input logic [15:0] expected,
        input string msg
    );
        if (actual !== expected) begin
            errors = errors + 1;
            $display("FAIL: %s (got 0x%04h, expected 0x%04h)", msg, actual, expected);
        end
    endtask

    initial begin
        logic signed [15:0] acc;
        logic signed [15:0] word;
        int c;

        errors = 0;
        rst_n  = 1'b0;
        repeat (2) @(negedge clk);
        rst_n  = 1'b1;
        @(negedge clk);

        // --- Generic 4×6: row-major addressing ---
        check_hex(u_generic.mem[0],  16'h0080, "generic addr 0 (n0,c0) = 0.5");
        check_hex(u_generic.mem[1],  16'hFF00, "generic addr 1 (n0,c1) = -1.0");
        check_hex(u_generic.mem[5],  16'h0000, "generic addr 5 (n0,c5) = 0");
        check_hex(u_generic.mem[6],  16'h8000, "generic addr 6 (n1,c0) = -128; column-major would put 8000 at addr 1");
        check_hex(u_generic.mem[7],  16'h7FFF, "generic addr 7 (n1,c1) = signed max");
        check_hex(u_generic.mem[11], 16'hFE00, "generic addr 11 (n1,c5) = -2.0");

        // Signed interpretation of the Dale-I word: $signed(FF00) = -256, not 65280.
        word = $signed(u_generic.mem[1]);
        check(word === -16'sd256, "FF00 must sign-extend to -256, not 65280");

        // Neuron 0, all six channels on, no leak: 0.5-1+0.25+1-0.5+0 = 0.25.
        acc = 16'sd0;
        for (c = 0; c < 6; c++) begin
            acc = sat_add(acc, $signed(u_generic.mem[c]));
        end
        check_hex(acc, 16'h0040, "signed accumulate of neuron 0 must be 0.25 (0040)");

        // The same words read as unsigned magnitudes cannot produce 0040.
        begin
            int unsigned unsigned_sum;
            unsigned_sum = 0;
            for (c = 0; c < 6; c++)
                unsigned_sum = unsigned_sum + int'(u_generic.mem[c]);
            check(unsigned_sum != 64, "unsigned sum of neuron 0 must not collapse to the signed 0.25 result");
            check(unsigned_sum > 16'hFF00, "unsigned misread of FF00 must dominate the sum");
        end

        // --- Spikenaut-shaped hidden 16×16 ---
        check_hex(u_hidden.mem[0],   16'h0100, "hidden [0][0] diagonal 1.0");
        check_hex(u_hidden.mem[1],   16'hFF00, "hidden [0][1] Dale-I -1.0");
        check_hex(u_hidden.mem[16],  16'hFF80, "hidden [1][0] -0.5");
        check_hex(u_hidden.mem[240], 16'h8000, "hidden [15][0] -128");
        check_hex(u_hidden.mem[255], 16'h7FFF, "hidden [15][15] signed max");

        // Neuron 0 with channels 0 and 1 on: 1.0 + (-1.0) = 0.
        acc = sat_add($signed(u_hidden.mem[0]), $signed(u_hidden.mem[1]));
        check_hex(acc, 16'h0000, "1.0 + (-1.0) must cancel; unsigned FF00 cannot");

        // Neuron 15 channel 0 is inhibitory minimum; adding it to 0 subtracts.
        acc = sat_add(16'sd0, $signed(u_hidden.mem[240]));
        check_hex(acc, 16'h8000, "signed min weight must remain 8000 after a 0+w accumulate");

        // --- Readout layouts: exporter K×N vs HDL N×K ---
        check_hex(u_readout_kxn.mem[0],  16'hFF00, "K×N [class0][n0] = -1.0");
        check_hex(u_readout_kxn.mem[17], 16'h0080, "K×N [class1][n1] = 0.5");
        check_hex(u_readout_kxn.mem[47], 16'h0100, "K×N [class2][n15] = 1.0");
        check_hex(u_readout_kxn.mem[4],  16'h0000, "K×N addr 4 is class0/n4, not class1/n1");

        check_hex(u_readout_nxk.mem[0],  16'hFF00, "N×K [n0][class0] = -1.0");
        check_hex(u_readout_nxk.mem[4],  16'h0080, "N×K addr = 1*3+1 = class1 of neuron 1");
        check_hex(u_readout_nxk.mem[47], 16'h0100, "N×K [n15][class2] = 1.0");

        // OutputLayer addressing against the neuron-major image: fire only n1.
        acc = 16'sd0;
        for (c = 0; c < 3; c++)
            if (c == 1)
                acc = sat_add(acc, $signed(u_readout_nxk.mem[1*3 + c]));
        check_hex(acc, 16'h0080, "HDL neuron-major: neuron 1 class 1 must contribute +0.5");

        // The same address on the exporter image is not class 1 of neuron 1.
        check(u_readout_kxn.mem[1*3 + 1] !== 16'h0080,
              "exporter K×N is not OutputLayer neuron-major; mismatch must stay visible");

        if (errors == 0) begin
            $display("TB_READMEMH_CONTRACT: ALL TESTS PASSED");
            $display("peer silicon-hdl commit d45163f38ac1cd88f8a3918e3793a08ace85e132");
            $display("evidence: HDL simulation (Icarus/Verilator), not board-measured parity");
            $finish;
        end else begin
            $display("TB_READMEMH_CONTRACT: %0d TEST(S) FAILED", errors);
            $fatal(1, "TB_READMEMH_CONTRACT: testbench FAILED");
        end
    end

endmodule
