// SPDX-License-Identifier: MIT OR Apache-2.0
//! FPGA Synthesis Metrics — Vivado Report Parser
//!
//! Parses **WNS** and **TNS** from Vivado timing summary reports
//! (`report_timing_summary`, e.g. `*_timing_summary_routed.rpt`) and **LUT
//! utilization** from Vivado utilization reports (`report_utilization`, e.g.
//! `*_utilization_placed.rpt`) for CI gating.
//!
//! Timing metrics are required — a report without a parsable `WNS(ns)` row
//! yields `None`. TNS and LUT utilization are optional: when a report omits
//! them the corresponding field stays at `0.0` instead of failing the parse.
//!
//! [`FpgaMetrics::synthesis_ok`] is derived from the timing summary, never
//! assumed: see [`FpgaMetrics::synthesis_ok_from_report`] for the rule.
//!
//! Extracted from Eagle-Lander's SpikingInferenceEngine (engine.rs).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// FPGA synthesis and implementation metrics parsed from Vivado reports.
///
/// Timing fields come from the post-route timing summary
/// (`<top>_timing_summary_routed.rpt`) and
/// [`lut_utilization`](FpgaMetrics::lut_utilization) from the post-place
/// utilization report (`<top>_utilization_placed.rpt`), both written to a
/// Vivado implementation run directory such as `<project>.runs/impl_1/`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct FpgaMetrics {
    /// Worst Negative Slack in nanoseconds.
    /// Negative value = timing violation. Positive = margin.
    pub wns_ns: f32,
    /// Total Negative Slack in nanoseconds, summed over all failing endpoints.
    /// `0.0` when timing is met — and also when the report has no TNS column.
    ///
    /// `#[serde(default)]` so metrics serialized before this field existed
    /// still deserialize.
    #[serde(default)]
    pub tns_ns: f32,
    /// LUT resource utilization (0.0–1.0).
    /// `0.0` when no utilization report was available.
    pub lut_utilization: f32,
    /// `true` only when the timing summary parsed completely and timing is
    /// met: WNS is finite and `>= 0`, TNS (when the column is present) is
    /// exactly `0`, and the report does not carry Vivado's
    /// `Timing constraints are not met.` banner.
    ///
    /// Derived by [`FpgaMetrics::synthesis_ok_from_report`]; `false` for a
    /// truncated data row or a failed run. A missing or unparsable report
    /// makes the loaders return `None` rather than metrics with this field
    /// set. This is a timing-closure verdict read from the routed timing
    /// summary — it does not inspect Vivado run logs.
    pub synthesis_ok: bool,
}

impl FpgaMetrics {
    /// Parse the WNS from a Vivado timing summary report text.
    ///
    /// Looks for the `WNS(ns)` column header row and extracts the first value
    /// of the data row beneath it, skipping blank lines and the rule of dashes
    /// Vivado prints under the headers.
    /// Returns `None` if the file format is not recognized.
    ///
    /// ```
    /// use silicon_bridge::FpgaMetrics;
    ///
    /// let report = "\
    ///     WNS(ns)      TNS(ns)
    ///     -------      -------
    ///       2.345        0.000
    /// ";
    /// assert_eq!(FpgaMetrics::parse_from_report(report), Some(2.345));
    /// ```
    pub fn parse_from_report(report_text: &str) -> Option<f32> {
        let (_header, data_row) = timing_summary_rows(report_text)?;
        data_row.split_whitespace().next()?.parse::<f32>().ok()
    }

    /// Parse the TNS from a Vivado timing summary report text.
    ///
    /// TNS is the second column of the same data row as WNS
    /// (`WNS(ns)  TNS(ns)  ...`), read only after confirming the second header
    /// column really is `TNS(ns)` — a report whose second column is something
    /// else (`WHS(ns)`, an endpoint count) yields `None` rather than a
    /// fabricated value. Returns `None` when the report has no such column,
    /// which callers should treat as "not reported" rather than as a failure.
    ///
    /// ```
    /// use silicon_bridge::FpgaMetrics;
    ///
    /// let report = "\
    ///     WNS(ns)      TNS(ns)
    ///     -------      -------
    ///      -0.427       -1.882
    /// ";
    /// assert_eq!(FpgaMetrics::parse_tns_from_report(report), Some(-1.882));
    /// ```
    pub fn parse_tns_from_report(report_text: &str) -> Option<f32> {
        let (header, data_row) = timing_summary_rows(report_text)?;
        // `WNS(ns)` and `TNS(ns)` are single-token headers, so the second
        // whitespace token names the column this reads.
        if header.split_whitespace().nth(1)? != "TNS(ns)" {
            return None;
        }
        data_row.split_whitespace().nth(1)?.parse::<f32>().ok()
    }

    /// Derive the [`synthesis_ok`](FpgaMetrics::synthesis_ok) verdict from a
    /// Vivado timing summary report text. Fails closed: returns `true` only
    /// when every check below passes, and never panics.
    ///
    /// 1. The `Design Timing Summary` block parses: a `WNS(ns)` header row
    ///    followed by a data row. When Vivado's column rule (`-------  ...`)
    ///    is present, the data row must have at least one value per column —
    ///    a row cut short by a truncated file is rejected.
    /// 2. WNS is finite and `>= 0.0`. An unconstrained design, which Vivado
    ///    reports as `inf`, is not a pass.
    /// 3. If the second header column is `TNS(ns)`, its value parses, is
    ///    finite, and equals `0.0`. A report without a TNS column is judged
    ///    on WNS alone.
    /// 4. The text does not contain `Timing constraints are not met.`, which
    ///    Vivado prints when any setup, hold, or pulse-width check fails.
    ///
    /// ```
    /// use silicon_bridge::FpgaMetrics;
    ///
    /// let met = "\
    ///     WNS(ns)      TNS(ns)
    ///     -------      -------
    ///       1.873        0.000
    /// ";
    /// assert!(FpgaMetrics::synthesis_ok_from_report(met));
    ///
    /// let failed = "\
    ///     WNS(ns)      TNS(ns)
    ///     -------      -------
    ///      -0.427      -12.906
    /// ";
    /// assert!(!FpgaMetrics::synthesis_ok_from_report(failed));
    /// assert!(!FpgaMetrics::synthesis_ok_from_report(""));
    /// ```
    pub fn synthesis_ok_from_report(report_text: &str) -> bool {
        let Some(block) = timing_summary_block(report_text) else {
            return false;
        };
        let mut values = block.data_row.split_whitespace();
        if let Some(rule) = block.rule {
            let columns = rule.split_whitespace().count();
            if block.data_row.split_whitespace().count() < columns {
                return false;
            }
        }
        let Some(Ok(wns)) = values.next().map(str::parse::<f32>) else {
            return false;
        };
        if !(wns.is_finite() && wns >= 0.0) {
            return false;
        }
        if block.header.split_whitespace().nth(1) == Some("TNS(ns)") {
            let Some(Ok(tns)) = values.next().map(str::parse::<f32>) else {
                return false;
            };
            if !(tns.is_finite() && tns == 0.0) {
                return false;
            }
        }
        !report_text.contains(TIMING_NOT_MET_BANNER)
    }

    /// Parse LUT utilization (0.0–1.0) from a Vivado **utilization** report.
    ///
    /// The source is `report_utilization` output — canonically
    /// `<project>.runs/impl_1/<top>_utilization_placed.rpt` — *not* the timing
    /// summary, which carries no resource numbers. The value is the `Util%`
    /// cell of the `Slice LUTs` row of the *Slice Logic* table (`CLB LUTs` on
    /// UltraScale devices), converted from percent to a fraction. Nested rows
    /// such as `LUT as Logic` are ignored. Returns `None` when the text has no
    /// such row, or when its `Util%` cell is blank or outside 0–100 — the
    /// column is never shifted to a neighboring cell to produce a value.
    ///
    /// ```
    /// use silicon_bridge::FpgaMetrics;
    ///
    /// let report = "\
    /// | Site Type   | Used | Fixed | Available | Util% |
    /// | Slice LUTs  | 3182 |     0 |     20800 | 15.30 |
    /// ";
    /// let lut = FpgaMetrics::parse_lut_utilization(report).unwrap();
    /// assert!((lut - 0.153).abs() < 1e-6);
    /// ```
    pub fn parse_lut_utilization(report_text: &str) -> Option<f32> {
        for line in report_text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') {
                continue;
            }
            // Strip the border pipes, then keep every cell — including empty
            // ones — so a blank cell cannot shift the `Util%` column.
            let cells: Vec<&str> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let Some((site_type, data_cells)) = cells.split_first() else {
                continue;
            };
            // Vivado marks rows influenced by fixed cells with a trailing `*`.
            let site_type = site_type.trim_end_matches('*').trim_end();
            if !site_type.eq_ignore_ascii_case("Slice LUTs")
                && !site_type.eq_ignore_ascii_case("CLB LUTs")
            {
                continue;
            }
            // `Util%` is the last cell; the column count varies by Vivado
            // version (some emit an extra `Prohibited` column).
            let Some(util_percent) = data_cells.last() else {
                continue;
            };
            let Ok(percent) = util_percent.parse::<f32>() else {
                continue;
            };
            if (0.0..=100.0).contains(&percent) {
                return Some(percent / 100.0);
            }
        }
        None
    }

    /// Load metrics from a Vivado implementation run directory.
    ///
    /// Reads `<impl_dir>/<top_module>_timing_summary_routed.rpt` for WNS/TNS
    /// and `<impl_dir>/<top_module>_utilization_placed.rpt` for LUT
    /// utilization — the names Vivado's default implementation strategy writes
    /// into a run directory such as `<project>.runs/impl_1`. Returns `None`
    /// only when the timing summary is missing or unparsable; a missing
    /// utilization report leaves `lut_utilization` at `0.0`.
    ///
    /// ```no_run
    /// use silicon_bridge::FpgaMetrics;
    ///
    /// let metrics = FpgaMetrics::load_from_impl_run("my_proj.runs/impl_1", "Basys3_Top");
    /// ```
    pub fn load_from_impl_run(impl_dir: impl AsRef<Path>, top_module: &str) -> Option<Self> {
        let impl_dir = impl_dir.as_ref();
        Self::load_from_reports(
            impl_dir.join(format!("{top_module}_timing_summary_routed.rpt")),
            impl_dir.join(format!("{top_module}_utilization_placed.rpt")),
        )
    }

    /// Load metrics from a custom timing summary report path.
    ///
    /// WNS is required; TNS falls back to `0.0` when the report has no TNS
    /// column. LUT utilization is read from this same file only if it also
    /// contains a `report_utilization` table (concatenated reports) — use
    /// [`FpgaMetrics::load_from_reports`] to read it from its own file.
    pub fn load_from_path(report_path: impl AsRef<Path>) -> Option<Self> {
        let text = std::fs::read_to_string(report_path).ok()?;
        Self::from_report_texts(&text, None)
    }

    /// Load metrics from a timing summary report plus a utilization report.
    ///
    /// The utilization report is best-effort: when it is missing or has no LUT
    /// row, `lut_utilization` stays `0.0` and the timing metrics are still
    /// returned.
    pub fn load_from_reports(
        timing_report_path: impl AsRef<Path>,
        utilization_report_path: impl AsRef<Path>,
    ) -> Option<Self> {
        let timing_text = std::fs::read_to_string(timing_report_path).ok()?;
        let utilization_text = std::fs::read_to_string(utilization_report_path).ok();
        Self::from_report_texts(&timing_text, utilization_text.as_deref())
    }

    /// Build metrics from already-loaded report texts, degrading gracefully
    /// when the optional TNS column or LUT row is absent.
    fn from_report_texts(timing_text: &str, utilization_text: Option<&str>) -> Option<Self> {
        let wns_ns = Self::parse_from_report(timing_text)?;
        let tns_ns = Self::parse_tns_from_report(timing_text).unwrap_or(0.0);
        let lut_utilization = utilization_text
            .and_then(Self::parse_lut_utilization)
            .or_else(|| Self::parse_lut_utilization(timing_text))
            .unwrap_or(0.0);
        Some(Self {
            wns_ns,
            tns_ns,
            lut_utilization,
            synthesis_ok: Self::synthesis_ok_from_report(timing_text),
        })
    }
}

/// Banner Vivado's `report_timing_summary` prints under the design summary
/// when any timing check fails (`All user specified timing constraints are
/// met.` otherwise).
const TIMING_NOT_MET_BANNER: &str = "Timing constraints are not met.";

/// The `Design Timing Summary` rows, each trimmed.
struct TimingSummaryBlock<'a> {
    /// The `WNS(ns)  TNS(ns)  ...` column header row.
    header: &'a str,
    /// The rule of dashes under the header, when Vivado printed one; it has
    /// exactly one dash group per column.
    rule: Option<&'a str>,
    /// The first data row beneath the header.
    data_row: &'a str,
}

/// Locate the header and data rows of a Vivado timing summary, both trimmed.
///
/// Scans for the `WNS(ns)` column header, then pairs it with the first line
/// beneath it that is neither blank nor a column rule.
fn timing_summary_rows(report_text: &str) -> Option<(&str, &str)> {
    timing_summary_block(report_text).map(|block| (block.header, block.data_row))
}

/// Locate the header, optional column rule, and data row of a Vivado timing
/// summary. The first rule after the header is kept; blank lines are skipped.
fn timing_summary_block(report_text: &str) -> Option<TimingSummaryBlock<'_>> {
    let mut header = None;
    let mut rule = None;
    for line in report_text.lines() {
        let trimmed = line.trim();
        let Some(header) = header else {
            if trimmed.starts_with("WNS(ns)") {
                header = Some(trimmed);
            }
            continue;
        };
        if trimmed.is_empty() {
            continue;
        }
        if is_column_rule(trimmed) {
            rule.get_or_insert(trimmed);
            continue;
        }
        return Some(TimingSummaryBlock {
            header,
            rule,
            data_row: trimmed,
        });
    }
    None
}

/// `true` if `trimmed` is a rule of dashes underlining the column headers
/// rather than a data row.
fn is_column_rule(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '-' || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Timing summary in the shape Vivado emits, column rule included.
    const TIMING_SUMMARY_WITH_RULE: &str = "\
------------------------------------------------------------------
| Tool Version : Vivado v2022.2 (64-bit)
| Design       : Basys3_Top
------------------------------------------------------------------

Design Timing Summary
---------------------

    WNS(ns)      TNS(ns)  TNS Failing Endpoints  TNS Total Endpoints
    -------      -------  ---------------------  -------------------
      2.345       -1.882                      3                 1234
";

    /// `report_utilization` excerpt (`*_utilization_placed.rpt`).
    const UTILIZATION_REPORT: &str = "\
1. Slice Logic
--------------

+----------------------------+------+-------+-----------+-------+
|          Site Type         | Used | Fixed | Available | Util% |
+----------------------------+------+-------+-----------+-------+
| Slice LUTs                 | 3182 |     0 |     20800 | 15.30 |
|   LUT as Logic             | 3050 |     0 |     20800 | 14.66 |
|   LUT as Memory            |  132 |     0 |      9600 |  1.38 |
| Slice Registers            | 1024 |     0 |     41600 |  2.46 |
+----------------------------+------+-------+-----------+-------+
";

    /// Assert two `f32` values agree to within float round-trip noise.
    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    /// Write `contents` to a temp file and hand back the handle plus its path.
    fn report_file(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(contents.as_bytes()).expect("write");
        file.flush().expect("flush");
        file
    }

    #[test]
    fn column_rule_is_skipped_before_the_data_row() {
        let wns =
            FpgaMetrics::parse_from_report(TIMING_SUMMARY_WITH_RULE).expect("WNS row not parsed");
        assert_close(wns, 2.345);
    }

    #[test]
    fn parses_tns_from_timing_summary() {
        let tns = FpgaMetrics::parse_tns_from_report(TIMING_SUMMARY_WITH_RULE)
            .expect("TNS column not parsed");
        assert_close(tns, -1.882);
    }

    #[test]
    fn parses_zero_tns_when_timing_is_met() {
        let report = "    WNS(ns)      TNS(ns)\n      2.345        0.000\n";
        assert_close(
            FpgaMetrics::parse_tns_from_report(report).expect("TNS not parsed"),
            0.0,
        );
    }

    #[test]
    fn absent_tns_column_does_not_break_wns() {
        let report = "    WNS(ns)\n      2.345\n";
        assert_close(
            FpgaMetrics::parse_from_report(report).expect("WNS not parsed"),
            2.345,
        );
        assert!(FpgaMetrics::parse_tns_from_report(report).is_none());
    }

    #[test]
    fn non_numeric_tns_returns_none_without_losing_wns() {
        let report = "    WNS(ns)      TNS(ns)\n      2.345          N/A\n";
        assert_close(
            FpgaMetrics::parse_from_report(report).expect("WNS not parsed"),
            2.345,
        );
        assert!(FpgaMetrics::parse_tns_from_report(report).is_none());
    }

    #[test]
    fn second_column_that_is_not_tns_is_not_read_as_tns() {
        // A variant summary whose second column is hold slack, not TNS.
        let report = "    WNS(ns)      WHS(ns)\n      2.345        0.123\n";
        assert_close(
            FpgaMetrics::parse_from_report(report).expect("WNS not parsed"),
            2.345,
        );
        assert!(FpgaMetrics::parse_tns_from_report(report).is_none());
    }

    #[test]
    fn tns_without_header_returns_none() {
        assert!(FpgaMetrics::parse_tns_from_report("      2.345       -1.882\n").is_none());
        assert!(FpgaMetrics::parse_tns_from_report("").is_none());
    }

    #[test]
    fn column_rule_alone_after_header_returns_none() {
        let report = "    WNS(ns)      TNS(ns)\n    -------      -------\n";
        assert!(FpgaMetrics::parse_from_report(report).is_none());
        assert!(FpgaMetrics::parse_tns_from_report(report).is_none());
    }

    #[test]
    fn parses_negative_wns() {
        let report = "WNS(ns)      TNS(ns)\n  -0.427        -1.882\n";
        let wns = FpgaMetrics::parse_from_report(report).expect("negative WNS not parsed");
        assert_close(wns, -0.427);
    }

    #[test]
    fn skips_blank_lines_between_header_and_data() {
        let report = "    WNS(ns)      TNS(ns)\n\n   \n\n      1.500        0.000\n";
        let wns = FpgaMetrics::parse_from_report(report).expect("data row after blanks not parsed");
        assert_close(wns, 1.5);
    }

    #[test]
    fn tolerates_tabs_and_trailing_whitespace() {
        let report = "\t WNS(ns) \t TNS(ns)  \n\t   0.875 \t 0.000  \n";
        let wns = FpgaMetrics::parse_from_report(report).expect("tab-separated row not parsed");
        assert_close(wns, 0.875);
    }

    #[test]
    fn header_without_data_row_returns_none() {
        let report = "    WNS(ns)      TNS(ns)\n";
        assert!(FpgaMetrics::parse_from_report(report).is_none());
    }

    #[test]
    fn missing_header_returns_none() {
        let report = "Design Timing Summary\n      2.345        0.000\n";
        assert!(FpgaMetrics::parse_from_report(report).is_none());
    }

    #[test]
    fn empty_input_returns_none() {
        assert!(FpgaMetrics::parse_from_report("").is_none());
        assert!(FpgaMetrics::parse_from_report("   \n\n\t\n").is_none());
    }

    #[test]
    fn garbage_input_returns_none() {
        assert!(FpgaMetrics::parse_from_report("not a vivado report at all").is_none());
    }

    #[test]
    fn non_numeric_first_token_returns_none() {
        let report = "    WNS(ns)      TNS(ns)\n        N/A          N/A\n";
        assert!(FpgaMetrics::parse_from_report(report).is_none());
    }

    #[test]
    fn load_from_path_returns_none_for_missing_file() {
        assert!(FpgaMetrics::load_from_path("does/not/exist.rpt").is_none());
    }

    #[test]
    fn load_from_path_returns_none_for_unparsable_report() {
        let file = report_file("no timing summary here\n");
        assert!(FpgaMetrics::load_from_path(file.path()).is_none());
    }

    #[test]
    fn parses_lut_utilization_from_utilization_report() {
        let lut = FpgaMetrics::parse_lut_utilization(UTILIZATION_REPORT).expect("LUT row missing");
        assert_close(lut, 0.153);
    }

    #[test]
    fn parses_lut_row_with_asterisk_and_extra_column() {
        // Newer Vivado emits a `Prohibited` column and stars constrained rows.
        let report = "\
| Site Type       | Used | Fixed | Prohibited | Available | Util% |
| Slice LUTs*     | 4160 |     0 |          0 |     20800 | 20.00 |
";
        assert_close(
            FpgaMetrics::parse_lut_utilization(report).expect("LUT row missing"),
            0.20,
        );
    }

    #[test]
    fn parses_clb_lut_row_for_ultrascale() {
        let report = "| CLB LUTs | 1040 | 0 | 20800 | 5.00 |\n";
        assert_close(
            FpgaMetrics::parse_lut_utilization(report).expect("CLB LUT row missing"),
            0.05,
        );
    }

    #[test]
    fn nested_lut_rows_alone_are_ignored() {
        let report = "\
|   LUT as Logic  | 3050 |     0 |     20800 | 14.66 |
|   LUT as Memory |  132 |     0 |      9600 |  1.38 |
";
        assert!(FpgaMetrics::parse_lut_utilization(report).is_none());
    }

    #[test]
    fn timing_report_has_no_lut_utilization() {
        assert!(FpgaMetrics::parse_lut_utilization(TIMING_SUMMARY_WITH_RULE).is_none());
    }

    #[test]
    fn non_numeric_lut_percentage_returns_none() {
        let report = "| Slice LUTs | 3182 | 0 | 20800 | n/a |\n";
        assert!(FpgaMetrics::parse_lut_utilization(report).is_none());
    }

    #[test]
    fn blank_lut_percentage_cell_does_not_shift_columns() {
        // The `Available` count must not be read as `Util%`.
        let report = "| Slice LUTs | 3182 | 0 | 20800 |   |\n";
        assert!(FpgaMetrics::parse_lut_utilization(report).is_none());
    }

    #[test]
    fn out_of_range_lut_percentage_returns_none() {
        let report = "| Slice LUTs | 3182 | 0 | 20800 | 20800 |\n";
        assert!(FpgaMetrics::parse_lut_utilization(report).is_none());
    }

    #[test]
    fn load_from_path_populates_tns_and_defaults_lut_to_zero() {
        let file = report_file(TIMING_SUMMARY_WITH_RULE);
        let metrics = FpgaMetrics::load_from_path(file.path()).expect("metrics not loaded");
        assert_close(metrics.wns_ns, 2.345);
        assert_close(metrics.tns_ns, -1.882);
        assert_close(metrics.lut_utilization, 0.0);
        // Negative TNS fails timing even though WNS is positive (#104).
        assert!(!metrics.synthesis_ok);
    }

    #[test]
    fn load_from_path_reads_lut_from_a_combined_report() {
        let combined = format!("{TIMING_SUMMARY_WITH_RULE}\n{UTILIZATION_REPORT}");
        let file = report_file(&combined);
        let metrics = FpgaMetrics::load_from_path(file.path()).expect("metrics not loaded");
        assert_close(metrics.wns_ns, 2.345);
        assert_close(metrics.lut_utilization, 0.153);
    }

    #[test]
    fn load_from_reports_populates_all_fields() {
        let timing = report_file(TIMING_SUMMARY_WITH_RULE);
        let utilization = report_file(UTILIZATION_REPORT);
        let metrics = FpgaMetrics::load_from_reports(timing.path(), utilization.path())
            .expect("metrics not loaded");
        assert_close(metrics.wns_ns, 2.345);
        assert_close(metrics.tns_ns, -1.882);
        assert_close(metrics.lut_utilization, 0.153);
        assert!(!metrics.synthesis_ok, "TNS -1.882 must not report a pass");
    }

    #[test]
    fn load_from_reports_tolerates_a_missing_utilization_report() {
        let timing = report_file(TIMING_SUMMARY_WITH_RULE);
        let metrics = FpgaMetrics::load_from_reports(timing.path(), "does/not/exist.rpt")
            .expect("timing metrics should survive a missing utilization report");
        assert_close(metrics.wns_ns, 2.345);
        assert_close(metrics.tns_ns, -1.882);
        assert_close(metrics.lut_utilization, 0.0);
    }

    #[test]
    fn load_from_reports_returns_none_without_a_timing_report() {
        let utilization = report_file(UTILIZATION_REPORT);
        assert!(FpgaMetrics::load_from_reports("does/not/exist.rpt", utilization.path()).is_none());
    }

    #[test]
    fn load_from_impl_run_reads_both_reports_for_the_top_module() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join("Top_timing_summary_routed.rpt"),
            TIMING_SUMMARY_WITH_RULE,
        )
        .expect("write timing report");
        std::fs::write(
            dir.path().join("Top_utilization_placed.rpt"),
            UTILIZATION_REPORT,
        )
        .expect("write utilization report");

        let metrics =
            FpgaMetrics::load_from_impl_run(dir.path(), "Top").expect("metrics not loaded");
        assert_close(metrics.wns_ns, 2.345);
        assert_close(metrics.tns_ns, -1.882);
        assert_close(metrics.lut_utilization, 0.153);
        assert!(FpgaMetrics::load_from_impl_run(dir.path(), "Other").is_none());
    }

    // macOS (APFS) rejects non-UTF-8 file names with EILSEQ, so this only runs on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn load_from_path_accepts_a_non_utf8_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(OsStr::from_bytes(b"timing_\xff.rpt"));
        assert!(path.to_str().is_none());
        std::fs::write(&path, TIMING_SUMMARY_WITH_RULE).expect("write timing report");

        let metrics = FpgaMetrics::load_from_path(&path).expect("metrics not loaded");
        assert_close(metrics.wns_ns, 2.345);
    }

    #[test]
    fn synthesis_ok_requires_non_negative_wns_and_zero_tns() {
        let met = "    WNS(ns)      TNS(ns)\n    -------      -------\n      0.000        0.000\n";
        assert!(FpgaMetrics::synthesis_ok_from_report(met));
        let negative_zero = "    WNS(ns)      TNS(ns)\n      0.125       -0.000\n";
        assert!(FpgaMetrics::synthesis_ok_from_report(negative_zero));
        let wns_only = "    WNS(ns)\n      0.125\n";
        assert!(FpgaMetrics::synthesis_ok_from_report(wns_only));

        assert!(!FpgaMetrics::synthesis_ok_from_report(
            TIMING_SUMMARY_WITH_RULE
        ));
        let negative_wns = "    WNS(ns)      TNS(ns)\n     -0.001        0.000\n";
        assert!(!FpgaMetrics::synthesis_ok_from_report(negative_wns));
        let negative_wns_alone = "    WNS(ns)\n     -0.001\n";
        assert!(!FpgaMetrics::synthesis_ok_from_report(negative_wns_alone));
    }

    #[test]
    fn synthesis_ok_rejects_non_finite_and_unparsable_values() {
        for row in [
            "inf 0.000",
            "NaN 0.000",
            "-inf 0.000",
            "1.0 NaN",
            "1.0 N/A",
            "N/A 0.0",
        ] {
            let report = format!("    WNS(ns)      TNS(ns)\n      {row}\n");
            assert!(
                !FpgaMetrics::synthesis_ok_from_report(&report),
                "row {row:?} must not pass"
            );
        }
        // TNS header present but the value is missing.
        assert!(!FpgaMetrics::synthesis_ok_from_report(
            "    WNS(ns)      TNS(ns)\n      1.000\n"
        ));
    }

    #[test]
    fn synthesis_ok_rejects_a_data_row_shorter_than_the_column_rule() {
        let report = "\
    WNS(ns)      TNS(ns)  TNS Failing Endpoints  TNS Total Endpoints
    -------      -------  ---------------------  -------------------
      1.873        0.000
";
        // The parsers still read WNS/TNS, but the verdict fails closed.
        assert_close(FpgaMetrics::parse_from_report(report).expect("WNS"), 1.873);
        assert!(!FpgaMetrics::synthesis_ok_from_report(report));
    }

    #[test]
    fn synthesis_ok_rejects_the_timing_not_met_banner() {
        // Hold violation: setup columns clean, Vivado still flags the run.
        let report = "    WNS(ns)      TNS(ns)\n      1.873        0.000\n\n\nTiming constraints are not met.\n";
        assert!(!FpgaMetrics::synthesis_ok_from_report(report));
    }

    #[test]
    fn synthesis_ok_is_false_without_a_timing_summary() {
        assert!(!FpgaMetrics::synthesis_ok_from_report(""));
        assert!(!FpgaMetrics::synthesis_ok_from_report(
            "not a vivado report"
        ));
        assert!(!FpgaMetrics::synthesis_ok_from_report(
            "    WNS(ns)      TNS(ns)\n"
        ));
        assert!(!FpgaMetrics::synthesis_ok_from_report(UTILIZATION_REPORT));
    }

    #[test]
    fn default_metrics_are_zeroed() {
        let metrics = FpgaMetrics::default();
        assert_close(metrics.wns_ns, 0.0);
        assert_close(metrics.tns_ns, 0.0);
        assert_close(metrics.lut_utilization, 0.0);
        assert!(!metrics.synthesis_ok);
    }

    #[test]
    fn metrics_round_trip_through_json() {
        let metrics = FpgaMetrics {
            wns_ns: 2.345,
            tns_ns: -1.882,
            lut_utilization: 0.153,
            synthesis_ok: true,
        };
        let json = serde_json::to_string(&metrics).expect("serialize");
        let decoded: FpgaMetrics = serde_json::from_str(&json).expect("deserialize");
        assert_close(decoded.wns_ns, 2.345);
        assert_close(decoded.tns_ns, -1.882);
        assert_close(decoded.lut_utilization, 0.153);
        assert!(decoded.synthesis_ok);
    }

    #[test]
    fn legacy_json_without_tns_still_deserializes() {
        // Payloads serialized before `tns_ns` existed must keep loading.
        let legacy = r#"{"wns_ns":2.345,"lut_utilization":0.153,"synthesis_ok":true}"#;
        let decoded: FpgaMetrics = serde_json::from_str(legacy).expect("legacy payload rejected");
        assert_close(decoded.wns_ns, 2.345);
        assert_close(decoded.tns_ns, 0.0);
        assert_close(decoded.lut_utilization, 0.153);
        assert!(decoded.synthesis_ok);
    }
}
