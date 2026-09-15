// SPDX-License-Identifier: MIT OR Apache-2.0
// WeightRam.sv
// Canonical source: spikenaut-core-sv/rtl
// Synaptic weight RAM – single-port, synchronous read/write
//
// Vendored for silicon-bridge #53 from rmems/silicon-hdl
// commit d45163f38ac1cd88f8a3918e3793a08ace85e132
// (https://github.com/rmems/silicon-hdl/blob/d45163f38ac1cd88f8a3918e3793a08ace85e132/spikenaut-core-sv/rtl/WeightRam.sv).
// gh-14 5u3.6 (P1): added rst_n + dout reset (for sim safety + post-config).
// Optional INIT_FILE: Q8.8 hex via $readmemh (sim + Vivado BRAM init).
// Prefer SV `parameter string` over bare untyped `parameter INIT_FILE = "NONE"`:
// some tools size untyped string defaults narrowly and path overrides misbehave
// (free-runner Verilator tb_WeightRam_init). Default "NONE" = no load.
// $fopen/$error/$fatal are simulation-only; synthesis keeps $readmemh (UG901).

module WeightRam #(
    parameter int    ADDR_WIDTH = 10,
    parameter int    DATA_WIDTH = 16,
    parameter string INIT_FILE  = "NONE"
)(
    input  logic                  clk,
    input  logic                  rst_n,
    input  logic                  we,
    input  logic [ADDR_WIDTH-1:0] addr,
    input  logic [DATA_WIDTH-1:0] din,
    output logic [DATA_WIDTH-1:0] dout
);

    (* ram_style = "block" *) logic [DATA_WIDTH-1:0] mem [0:(2**ADDR_WIDTH)-1];

    // Elaboration/time-0 init from Spikenaut-style .mem (one hex word per line).
    // Paths are relative to the simulator/synth working directory (repo root in CI).
    // $readmemh loads min(file lines, mem depth); pair INIT_FILE size with ADDR_WIDTH.
    initial begin
        if (INIT_FILE != "NONE" && INIT_FILE != "") begin
`ifndef SYNTHESIS
            begin : init_file_check
                int fd;
                fd = $fopen(INIT_FILE, "r");
                if (fd == 0) begin
                    $error("WeightRam: INIT_FILE '%s' not found or cannot be opened", INIT_FILE);
                    $fatal(1);
                end
                $fclose(fd);
            end
`endif
            $readmemh(INIT_FILE, mem);
        end
    end

    // Synchronous reset on dout (to use rst_n port and provide known value in sim).
    // Read-first semantics preserved (original contract): on write cycle, dout gets
    // old mem value. gh-14 5u3.6 + Devin feedback on semantics change and unused port.
    // Note: BRAM inference with sync reset on output depends on Vivado settings;
    // attribute is a hint.
    always_ff @(posedge clk) begin
        if (!rst_n) begin
            dout <= '0;
        end else begin
            if (we)
                mem[addr] <= din;
            dout <= mem[addr];
        end
    end

endmodule
