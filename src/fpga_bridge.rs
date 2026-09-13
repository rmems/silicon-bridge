// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA Spike Readback — UART Bridge to Basys3 Hardware
//!
//! Handles UART communication with Basys3 FPGA to send stimuli
//! and read back spike states using the SiliconBridge v3.0 protocol.
//!
//! Extracted from Eagle-Lander's Ship of Theseus neuromorphic core.
//!
//! ## Q8.8 convention used here: signed
//!
//! Host stimuli and membrane potentials on the wire are **signed** Q8.8
//! (`i16`, two's complement, big-endian) — the same convention `fpga_export`
//! writes into `.mem` parameter images. Both encode through
//! [`crate::encode_q88_signed`]; only the framing differs.
//!
//! | Aspect | This module (UART TX/RX) | `fpga_export` (`.mem`) |
//! |---|---|---|
//! | Encode with | [`crate::encode_q88_signed`] | [`crate::encode_q88_signed`] |
//! | Decode with | [`crate::q88_signed_to_f32`] | [`crate::q88_signed_to_f32`] |
//! | Raw type | `i16` (two's complement) | `i16` (two's complement) |
//! | Encoder input clamp | [`crate::STIMULUS_Q88_MIN`]`..=`[`crate::STIMULUS_Q88_MAX`] | same |
//! | Byte order | raw binary, big-endian (MSB first) | ASCII hex, one `{:04X}` word per line |
//! | Use it for | host stimuli, RX membrane potentials | weights, thresholds, decay rates |
//!
//! [`crate::encode_q88_unsigned`] / [`crate::q88_to_f32`] are an
//! unsigned-magnitude pair and are **not** the hardware convention on either
//! path: a stimulus encoded with them turns every inhibitory input into `0`.

use serialport::{SerialPort, SerialPortInfo};
use std::io::{Read, Write};
use std::time::Duration;

pub struct FpgaBridge {
    port: Box<dyn SerialPort>,
    active: bool,
}

impl FpgaBridge {
    /// Try to open FPGA connection on available USB ports
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Try common USB ports for Basys3
        let ports = ["/dev/ttyUSB0", "/dev/ttyUSB1", "/dev/ttyUSB2"];

        for port_name in &ports {
            match serialport::new(*port_name, 115_200)
                .timeout(Duration::from_millis(100))
                .open()
            {
                Ok(port) => {
                    println!("[fpga] Connected to FPGA on {}", port_name);
                    return Ok(FpgaBridge { port, active: true });
                }
                Err(_) => continue,
            }
        }

        Err("FPGA not found on any USB port".into())
    }

    /// Send neural stimuli to FPGA and read back spike states.
    ///
    /// Protocol (16-neuron SiliconBridge v3.0):
    ///   TX: 0xAA + 32 bytes (16 × signed Q8.8 stimuli, big-endian)
    ///   RX: 32 bytes (16 × signed Q8.8 potentials) + 2 bytes (spike flags) + 2 bytes (switches)
    ///
    /// Stimuli are encoded with [`crate::encode_q88_signed`] (`i16`) — **not**
    /// the unsigned-magnitude [`crate::encode_q88_unsigned`], which would flatten
    /// every inhibitory input to `0`. RX potentials use
    /// [`crate::q88_signed_to_f32`].
    ///
    /// Input is accepted as a dynamic slice; if fewer than 16 values are provided,
    /// remaining channels are zero-padded. If more are provided, only the first 16 are sent.
    pub fn process_stimuli(
        &mut self,
        stimuli: &[f32],
    ) -> Result<(Vec<f32>, Vec<bool>), Box<dyn std::error::Error>> {
        if !self.active {
            return Err("FPGA bridge not active".into());
        }

        // ENCODE SITE (signed Q8.8) — UART TX. Same encoder as the `.mem`
        // export path; see fpga_export's module docs for the shared contract.
        let mut tx_data = vec![0xAAu8]; // Sync byte
        for i in 0..16 {
            let s = stimuli.get(i).copied().unwrap_or(0.0);
            let q8_8 = crate::encode_q88_signed(s);
            tx_data.extend_from_slice(&q8_8.to_be_bytes());
        }

        // Send to FPGA
        self.port.write_all(&tx_data)?;
        self.port.flush()?;

        // Read response: 32 bytes potentials + 2 bytes spike flags + 2 bytes switches
        let mut rx_data = vec![0u8; 36];
        self.port.read_exact(&mut rx_data)?;

        // DECODE SITE (signed Q8.8, big-endian) — membrane potentials can be negative.
        let mut potentials = Vec::with_capacity(16);
        for i in 0..16 {
            let raw = i16::from_be_bytes([rx_data[i * 2], rx_data[i * 2 + 1]]);
            potentials.push(crate::q88_signed_to_f32(raw));
        }

        // Parse spike flags (16-bit, 1 per neuron)
        let spike_word = u16::from_be_bytes([rx_data[32], rx_data[33]]);
        let spikes = (0..16).map(|i| (spike_word & (1 << i)) != 0).collect();
        // rx_data[34..36] = switch state (available but unused here)

        Ok((potentials, spikes))
    }

    /// Check if FPGA is responsive
    pub fn ping(&mut self) -> bool {
        let test_stimuli = [0.1; 16];
        match self.process_stimuli(&test_stimuli) {
            Ok(_) => true,
            Err(_) => {
                self.active = false;
                false
            }
        }
    }

    /// Get connection status
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Find FPGA ports on the system
pub fn find_fpga_ports() -> Vec<SerialPortInfo> {
    match serialport::available_ports() {
        Ok(ports) => ports
            .into_iter()
            .filter(|p| p.port_name.contains("ttyUSB"))
            .collect(),
        Err(_) => Vec::new(),
    }
}
