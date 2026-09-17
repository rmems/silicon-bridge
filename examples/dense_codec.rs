// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure UART codec demo: no `uart` feature, no serialport, no live stimuli.
//!
//! SiliconBridge v3.0 (16-channel) is an **example layout** with golden-byte
//! evidence (`tests/golden/uart/`). Other [`DenseQ88Layout::dense`] sizes are
//! host codecs only — they need matching FPGA firmware. Changing host channel
//! counts does not reconfigure a board. This example never opens a port.
//!
//! ```text
//! cargo run --example dense_codec
//! ```

use silicon_bridge::{
    DENSE_Q88_SYNC, DenseQ88Layout, SILICON_BRIDGE_V3_RX_LEN, SILICON_BRIDGE_V3_TX_LEN,
    decode_response, encode_stimuli,
};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let v3 = DenseQ88Layout::silicon_bridge_v3();
    assert_eq!(v3.input_channels(), 16);
    assert_eq!(v3.output_neurons(), 16);
    assert_eq!(v3.tx_len()?, SILICON_BRIDGE_V3_TX_LEN);
    assert_eq!(v3.rx_len()?, SILICON_BRIDGE_V3_RX_LEN);

    let stimuli = vec![0.1; 16];
    let tx = encode_stimuli(&v3, &stimuli)?;
    assert_eq!(tx[0], DENSE_Q88_SYNC);
    assert_eq!(tx.len(), SILICON_BRIDGE_V3_TX_LEN);
    println!(
        "v3 TX {} bytes (sync {:02X}); not sent to any device",
        tx.len(),
        tx[0]
    );

    // Host-only dense 8: golden bytes exist (#53). Firmware that only speaks
    // 16/16 will not accept this frame. Matching RTL is required.
    let dense8 = DenseQ88Layout::dense(8, 8)?;
    let tx8 = encode_stimuli(&dense8, &[0.0; 8])?;
    println!(
        "dense 8/8 TX {} bytes — host codec only; do not treat as board support",
        tx8.len()
    );

    // Decode a zeroed v3-length buffer to show the API. A correct length is
    // not proof of a fresh FPGA reply (unframed RX; see crate docs).
    let rx = vec![0u8; SILICON_BRIDGE_V3_RX_LEN];
    let decoded = decode_response(&v3, &rx)?;
    assert_eq!(decoded.potentials.len(), 16);
    println!(
        "decoded a {}-byte buffer in software; ordinary tests/examples never open UART",
        rx.len()
    );
    Ok(())
}
