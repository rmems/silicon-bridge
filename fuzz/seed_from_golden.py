#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Regenerate fuzz/corpus seeds from tests/golden UART and Q8.8 fixtures."""

from __future__ import annotations

import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = ROOT / "tests" / "golden"
UART = GOLDEN / "uart"
CORPUS = ROOT / "fuzz" / "corpus"


def parse_hex(hex_str: str) -> bytes:
    hex_str = hex_str.strip()
    if len(hex_str) % 2:
        raise ValueError(f"odd hex length: {hex_str!r}")
    return bytes.fromhex(hex_str)


def write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def dim_prefix(inputs: int, outputs: int, payload: bytes) -> bytes:
    # 0 means MAX_DENSE_CHANNELS (256) in the fuzz targets; 1..=255 are literal.
    def encode(n: int) -> int:
        if n == 256:
            return 0
        if not 1 <= n <= 255:
            raise ValueError(f"dimension {n} does not fit the u8 seed prefix")
        return n

    return bytes((encode(inputs), encode(outputs))) + payload


def main() -> None:
    v3 = json.loads((UART / "legacy_v3.json").read_text())
    dense_8 = json.loads((UART / "dense_8.json").read_text())
    dense_32 = json.loads((UART / "dense_32.json").read_text())
    dense_8x10 = json.loads((UART / "dense_8in_10out.json").read_text())

    frames = [
        ("v3", 16, 16, v3["tx_hex"], v3["rx_hex"], v3["stimuli_f32"]),
        (
            "dense_8",
            8,
            8,
            dense_8["tx_hex"],
            dense_8["rx_hex"],
            dense_8["stimuli_f32"],
        ),
        (
            "dense_32",
            32,
            32,
            dense_32["tx_hex"],
            dense_32["rx_hex"],
            [-1.0] + [0.0] * 30 + [1.0],
        ),
        (
            "dense_8in_10out",
            8,
            10,
            dense_8x10["tx_hex"],
            dense_8x10["rx_hex"],
            dense_8x10["stimuli_f32"],
        ),
    ]

    planned: list[tuple[Path, bytes]] = []
    for name, inputs, outputs, tx_hex, rx_hex, stimuli in frames:
        tx = parse_hex(tx_hex)
        rx = parse_hex(rx_hex)
        packed = b"".join(struct.pack("<f", float(v)) for v in stimuli)
        planned.extend(
            [
                (
                    CORPUS / "decode_response" / f"golden_{name}_rx",
                    dim_prefix(inputs, outputs, rx),
                ),
                (CORPUS / "decode_response" / f"golden_{name}_rx_raw", rx),
                (
                    CORPUS / "encode_stimuli" / f"golden_{name}_tx",
                    dim_prefix(inputs, outputs, tx),
                ),
                (
                    CORPUS / "encode_stimuli" / f"golden_{name}_stimuli",
                    dim_prefix(inputs, outputs, packed),
                ),
                (
                    CORPUS / "chunked_frames" / f"golden_{name}_rx",
                    dim_prefix(inputs, outputs, bytes([1]) + rx),
                ),
            ]
        )

    signed = json.loads((GOLDEN / "q88_signed.json").read_text())
    unsigned = json.loads((GOLDEN / "q88_unsigned.json").read_text())
    values = [float(case["value"]) for case in signed["cases"]]
    values.extend(float(case["value"]) for case in unsigned["cases"])
    values.extend(
        [
            -127.99,
            127.99,
            float("nan"),
            float("inf"),
            float("-inf"),
            -128.0,
            128.0,
        ]
    )
    packed_q88 = b"".join(struct.pack("<f", v) for v in values)
    planned.extend(
        [
            (CORPUS / "q88_codecs" / "golden_extrema_f32", packed_q88),
            (CORPUS / "q88_codecs" / "all_zero_i16", b"\x00\x00"),
            (CORPUS / "q88_codecs" / "i16_min", struct.pack("<h", -32768)),
            (CORPUS / "q88_codecs" / "i16_max", struct.pack("<h", 32767)),
            (CORPUS / "q88_codecs" / "uart_min", struct.pack("<h", -32765)),
            (CORPUS / "q88_codecs" / "uart_max", struct.pack("<h", 32765)),
        ]
    )

    for path, data in planned:
        write(path, data)

    print("wrote seeds under", CORPUS)


if __name__ == "__main__":
    main()
