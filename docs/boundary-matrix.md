# silicon-bridge runtime / deployment boundary matrix

Planning document for GitHub [#3](https://github.com/rmems/silicon-bridge/issues/3)
and Linear [LIM-306](https://linear.app/rpd-34/issue/LIM-306) /
[LIM-589](https://linear.app/rpd-34/issue/LIM-589).

This is **documentation only** — no runtime behavior changes.

## Purpose

`silicon-bridge` is the **host-side deployment bridge** between trained SNN
parameters and FPGA hardware, owned under
[`rmems/silicon-bridge`](https://github.com/rmems/silicon-bridge)
(historically Limen-Neural). It:

1. Converts float SNN parameters to **signed two's-complement Q8.8**
   (`[-128, 127.99609375]`) and writes Vivado `$readmemh` `.mem` files — the
   signedness silicon-hdl's RTL reads (`spikenaut-core-sv/mem/README.md`,
   "Signedness contract (GH#73)"). UART stimuli keep a narrower ±127.99 clamp.
2. Optionally exchanges stimuli / spikes with the FPGA over **UART**
   (`uart` feature)
3. Parses **Vivado timing reports for WNS and TNS** (worst / total negative slack)
   and **LUT utilization** from `report_utilization` output for CI gating.
   `FpgaMetrics` fills `tns_ns` and `lut_utilization` when the reports carry them and
   falls back to `0.0` when they do not — a gate must therefore treat `0.0` as
   "not reported", not as "clean"

It is **not** a spiking runtime, training loop, or HDL library.

## Layer placement

| Layer | Role | Example repos |
|-------|------|---------------|
| Core SNN / traits | Neuron dynamics, shared trait contracts | `neuromod` |
| Sensory / extract | Continuous→spike, MoE→SNN parameters | `axon-encoder`, `engram-parser` |
| Topology / train | Connectivity, plasticity, offline training | `synaptic-mesh`, `plasticity-lab` |
| Reward / critic | Neuromodulator-style signals | `limbic-critic` |
| Runtime host | Headless inference orchestration | `brainstem-daemon` |
| **Deployment (this crate)** | **Q8.8 export, UART host, metrics** | **`silicon-bridge`** |
| Hardware RTL | SystemVerilog core / bridge / SoC | **`silicon-hdl`** |

```text
training / runtime crates
        │  float thresholds, weights, decay
        ▼
  silicon-bridge   ──.mem / UART──►  silicon-hdl (FPGA)
```

## Owns

| Area | Detail |
|------|--------|
| Fixed-point export | Q8.8 encode/decode helpers and parameter bundles |
| `.mem` generation | Hex lines for `$readmemh` (thresholds, weights, decay) |
| Export trait surface | Traits such as fixed-point encode / parameter export / mem write (for silicon-hdl alignment) |
| UART host client | SiliconBridge-style host protocol when `uart` is enabled |
| Timing metrics | Parse WNS / TNS from Vivado timing summary reports, and LUT % from utilization reports, for CI gates |
| Deployment metadata | Version/timestamp/size of exported parameter sets; named generic vs Spikenaut export profiles (#50) |

## Does not own

| Area | Owner |
|------|--------|
| LIF / HH / other neuron dynamics | `neuromod` |
| Encoding continuous signals to spikes | `axon-encoder` |
| MoE weight extraction | `engram-parser` |
| Online / offline training | `plasticity-lab` |
| Reward / risk modulators | `limbic-critic` |
| Process orchestration / daemon lifecycle | `brainstem-daemon` |
| SystemVerilog RTL, Vivado project trees | **`silicon-hdl`** (formerly informal “Spikenaut-Hardware”) |
| Domain adapters (trading, mining telemetry) | Out of org core; must not leak into this crate |
| Full SNN simulator | Out of scope |
| NIR HDF5 graph I/O | Deferred; shared crate (`nir-rs`) if/when wired — not reimplemented here |

## Allowed dependencies

Current and intentional:

| Crate | Why |
|-------|-----|
| `serde` / `serde_json` | Parameter metadata JSON |
| `chrono` | Export timestamps |
| `serialport` (optional) | UART feature only |

Do **not** list unused crates as allowed. `rand` was previously in `Cargo.toml` with
no `src/` imports; it is not an intentional dependency (see REVIEW.md).

Future (allowed when explicitly landed):

| Crate | Why |
|-------|-----|
| `nir-rs` ([Limen-Neural/nir-rs](https://github.com/Limen-Neural/nir-rs); not transferred) | NIR → Q8.8 mapping without local HDF5 reimplementation |

## Forbidden dependencies / content

- Hard dependency on domain products (trading, mining, HFT adapters)
- Embedding or vendoring silicon-hdl RTL
- Pulling full `neuromod` simulation stacks solely to re-export dynamics
- Secrets, DSNs, or machine-local paths in source
- Treating this crate as the org-wide “core SNN library”

## Boundaries vs sibling repos

### vs `neuromod`

- **neuromod**: core dynamics and shared trait *contracts* for neurons/networks.
- **silicon-bridge**: takes *already trained* parameters (floats) and prepares them for FPGA.
- Do not move LIF update loops into silicon-bridge.

### vs `limbic-critic`

- **limbic-critic**: modulator / reward vectors.
- **silicon-bridge**: does not interpret reward; may only receive numeric params if a pipeline feeds them as weights/thresholds elsewhere.

### vs `brainstem-daemon`

- **brainstem-daemon**: long-running inference host / orchestration.
- **silicon-bridge**: library used *by* hosts for FPGA export or UART; not a daemon.

### vs `silicon-hdl`

- **silicon-hdl**: single source of truth for RTL (`WeightRam`, `NeuronParamRam`, `SiliconBridge`, …). The **reference** companion SoC is Digilent Basys 3 / Artix-7 `XC7A35T-1CPG236C` (`spikenaut_soc_basys3_top`); silicon-hdl also ships an Artix-7 trainer XDC that is **not** the claimed demo.
- **silicon-bridge**: host tools and parameter formats that must **align** with those modules. Default `.mem` export is board-agnostic; UART v3 golden bytes match Basys 3 firmware.
- Alignment issues (widths, file names, UART frames) are coordinated across both repos; RTL changes land in silicon-hdl.

## Domain leaks, risks, sequencing

| Risk | Mitigation |
|------|------------|
| Old “Spikenaut-Hardware” naming | Prefer **silicon-hdl** in all new docs |
| Trait drift vs HDL RAM layout | Define export traits in silicon-bridge; HDL alignment issues track parity |
| NIR reimplementation per crate | Defer NIR; use shared `nir-rs` later |
| CI with `uart` / serialport | Feature-gate; avoid requiring `libudev` on all runners |
| Cargo `docs/` exclude | Boundary doc lives in git under `docs/` for humans; crate package may exclude it |

**Suggested sequence (planning):**

1. Stable export traits + silicon-hdl docs (GitHub #7)
2. HDL interface alignment issue on silicon-hdl
3. Optional NIR consumer after shared IR crate exists

## Wiki

GitHub wiki is **enabled** on [`rmems/silicon-bridge`](https://github.com/rmems/silicon-bridge/wiki).
Prefer this in-repo file and `README.md` for durable process documentation; treat
the wiki as optional narrative only. Local personal shelf clones of the wiki
are not part of the published process.

## Validation checklist

- [x] Purpose documented
- [x] Owns / does-not-own explicit
- [x] Allowed and forbidden dependencies listed
- [x] Core vs runtime vs deployment/hardware layers explicit
- [x] Risks and sequencing recorded
- [x] Linkable from Linear (this file path: `docs/boundary-matrix.md`)
- [x] Wiki story explicit (enabled under rmems; durable docs stay in-repo)
