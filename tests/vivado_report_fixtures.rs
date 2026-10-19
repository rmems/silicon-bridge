// SPDX-License-Identifier: MIT OR Apache-2.0
//! `FpgaMetrics` against recorded-shape Vivado 7-series (Artix-7 xc7a35t,
//! Basys 3) `report_timing_summary` + `report_utilization` fixtures, plus a
//! property test that arbitrary text never panics the parsers (#104 / RM-2037).
//!
//! Fixtures live in `tests/fixtures/vivado/<case>/`, named as Vivado writes
//! them into an implementation run directory, so each case goes through
//! [`FpgaMetrics::load_from_impl_run`] exactly like a CI gate would.

use std::io::Write;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use silicon_bridge::FpgaMetrics;

const TOP: &str = "Basys3_Top";

fn fixture_dir(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vivado")
        .join(case)
}

fn fixture_text(case: &str, file: &str) -> String {
    std::fs::read_to_string(fixture_dir(case).join(file)).expect("fixture readable")
}

fn timing_text(case: &str) -> String {
    fixture_text(case, &format!("{TOP}_timing_summary_routed.rpt"))
}

fn load(case: &str) -> Option<FpgaMetrics> {
    FpgaMetrics::load_from_impl_run(fixture_dir(case), TOP)
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "expected {expected}, got {actual}"
    );
}

// (a) Timing met.
#[test]
fn timing_met_reports_synthesis_ok() {
    let metrics = load("timing_met").expect("timing_met fixture must parse");
    assert_close(metrics.wns_ns, 1.873);
    assert_close(metrics.tns_ns, 0.0);
    assert_close(metrics.lut_utilization, 0.153);
    assert!(metrics.synthesis_ok);
    assert!(FpgaMetrics::synthesis_ok_from_report(&timing_text(
        "timing_met"
    )));
}

// (b) Timing failed: negative WNS and TNS, `Timing constraints are not met.`
#[test]
fn timing_failed_reports_not_ok() {
    let metrics = load("timing_failed").expect("timing_failed fixture must parse");
    assert_close(metrics.wns_ns, -0.427);
    assert_close(metrics.tns_ns, -12.906);
    assert_close(metrics.lut_utilization, 0.8292);
    assert!(!metrics.synthesis_ok);
}

#[test]
fn timing_failed_is_rejected_by_each_check_independently() {
    let text = timing_text("timing_failed");
    // Without the banner, negative WNS/TNS alone must still fail.
    let no_banner = text.replace("Timing constraints are not met.", "");
    assert!(!FpgaMetrics::synthesis_ok_from_report(&no_banner));
    // With clean numbers, the banner alone must still fail.
    let clean_numbers = text.replace("     -0.427      -12.906", "      0.427        0.000");
    assert_ne!(clean_numbers, text, "fixture data row changed shape");
    assert!(!FpgaMetrics::synthesis_ok_from_report(&clean_numbers));
}

// (c) Empty / truncated.
#[test]
fn empty_reports_yield_no_metrics() {
    assert!(load("empty").is_none());
    assert!(!FpgaMetrics::synthesis_ok_from_report(&timing_text(
        "empty"
    )));
}

#[test]
fn report_truncated_after_the_column_rule_yields_no_metrics() {
    assert!(load("truncated").is_none());
    assert!(!FpgaMetrics::synthesis_ok_from_report(&timing_text(
        "truncated"
    )));
}

#[test]
fn report_truncated_mid_data_row_is_not_ok() {
    let text = fixture_text(
        "truncated",
        &format!("{TOP}_timing_summary_routed_midrow.rpt"),
    );
    // WNS and TNS survive the cut, so the value parsers still answer ...
    assert_close(FpgaMetrics::parse_from_report(&text).expect("WNS"), 1.873);
    assert_close(FpgaMetrics::parse_tns_from_report(&text).expect("TNS"), 0.0);
    // ... but a row shorter than its column rule is not a verdict.
    assert!(!FpgaMetrics::synthesis_ok_from_report(&text));

    let path = fixture_dir("truncated").join(format!("{TOP}_timing_summary_routed_midrow.rpt"));
    let metrics = FpgaMetrics::load_from_path(path).expect("partial row still loads");
    assert!(!metrics.synthesis_ok);
}

// (d) CRLF line endings.
#[test]
fn crlf_fixture_really_uses_crlf() {
    let text = timing_text("crlf");
    assert!(text.contains("\r\n"));
    assert!(!timing_text("timing_met").contains('\r'));
}

#[test]
fn crlf_reports_parse_like_lf_reports() {
    let crlf = load("crlf").expect("CRLF fixture must parse");
    let lf = load("timing_met").expect("LF fixture must parse");
    assert_close(crlf.wns_ns, lf.wns_ns);
    assert_close(crlf.tns_ns, lf.tns_ns);
    assert_close(crlf.lut_utilization, lf.lut_utilization);
    assert!(crlf.synthesis_ok);
}

// (e) Utilization report without the Slice Logic section.
#[test]
fn missing_utilization_section_keeps_timing_verdict() {
    let metrics = load("missing_utilization").expect("timing summary must parse");
    assert_close(metrics.wns_ns, 1.873);
    assert_close(metrics.lut_utilization, 0.0);
    // LUT usage is not a timing verdict: a met design stays ok.
    assert!(metrics.synthesis_ok);
    assert!(
        FpgaMetrics::parse_lut_utilization(&fixture_text(
            "missing_utilization",
            &format!("{TOP}_utilization_placed.rpt")
        ))
        .is_none()
    );
}

#[test]
fn missing_utilization_file_keeps_timing_verdict() {
    let dir = fixture_dir("timing_met");
    let metrics = FpgaMetrics::load_from_reports(
        dir.join(format!("{TOP}_timing_summary_routed.rpt")),
        dir.join("does_not_exist.rpt"),
    )
    .expect("timing summary must parse");
    assert_close(metrics.lut_utilization, 0.0);
    assert!(metrics.synthesis_ok);
}

/// Text built from report fragments, so cases reach the header / rule / data
/// row paths instead of being rejected on the first line.
fn report_like() -> impl Strategy<Value = String> {
    let fragment = prop_oneof![
        Just("WNS(ns)".to_owned()),
        Just("TNS(ns)".to_owned()),
        Just("WHS(ns)".to_owned()),
        Just("-------".to_owned()),
        Just("|".to_owned()),
        Just("Slice LUTs".to_owned()),
        Just("CLB LUTs*".to_owned()),
        Just("Timing constraints are not met.".to_owned()),
        Just("inf".to_owned()),
        Just("NaN".to_owned()),
        Just("-0.000".to_owned()),
        any::<f32>().prop_map(|v| v.to_string()),
        Just(" ".to_owned()),
        Just("\t".to_owned()),
        Just("\n".to_owned()),
        Just("\r\n".to_owned()),
        Just("\r".to_owned()),
        "\\PC{0,8}",
    ];
    proptest::collection::vec(fragment, 0..48).prop_map(|parts| parts.concat())
}

fn exercise_parsers(text: &str) {
    let wns = FpgaMetrics::parse_from_report(text);
    let _ = FpgaMetrics::parse_tns_from_report(text);
    if let Some(lut) = FpgaMetrics::parse_lut_utilization(text) {
        assert!(
            (0.0..=1.0).contains(&lut),
            "LUT fraction {lut} out of range"
        );
    }
    if FpgaMetrics::synthesis_ok_from_report(text) {
        // A pass always rests on a parsed, finite, non-negative WNS.
        let wns = wns.expect("synthesis_ok without a parsable WNS");
        assert!(wns.is_finite() && wns >= 0.0, "synthesis_ok with WNS {wns}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_strings_never_panic_the_parsers(text in any::<String>()) {
        exercise_parsers(&text);
    }

    #[test]
    fn report_like_strings_never_panic_the_parsers(text in report_like()) {
        exercise_parsers(&text);
    }

    #[test]
    fn arbitrary_report_files_never_panic_the_loaders(
        timing in report_like(),
        utilization in any::<String>(),
    ) {
        let mut timing_file = tempfile::NamedTempFile::new().expect("temp file");
        timing_file.write_all(timing.as_bytes()).expect("write");
        let mut util_file = tempfile::NamedTempFile::new().expect("temp file");
        util_file.write_all(utilization.as_bytes()).expect("write");

        let loaded = FpgaMetrics::load_from_reports(timing_file.path(), util_file.path());
        let by_path = FpgaMetrics::load_from_path(timing_file.path());
        prop_assert_eq!(loaded.is_some(), by_path.is_some());
        if let Some(metrics) = loaded {
            prop_assert_eq!(
                metrics.synthesis_ok,
                FpgaMetrics::synthesis_ok_from_report(&timing)
            );
        }
    }
}
