use super::net::*;

use common::config::BagType::*;

#[test]
fn hard_drop_deterministic() {
    run_recorded_test("hard-drop-deterministic.json", Deterministic, false);
}

#[test]
fn hard_drop_deterministic2() {
    run_recorded_test("hard-drop-deterministic2.json", Deterministic, false);
}

#[test]
fn hard_drop_727() {
    run_recorded_test("hard-drop-727.json", FixedSeed(727), false);
}

#[test]
fn hard_drop_hold_0() {
    run_recorded_test("hard-drop-hold-0.json", FixedSeed(0), false);
}

#[test]
fn score_deterministic2() {
    run_recorded_test("score-deterministic2.json", Deterministic, true);
}

#[test]
fn score_deterministic3() {
    run_recorded_test("score-deterministic3.json", Deterministic, true);
}

#[test]
fn score_deterministic4() {
    run_recorded_test("score-deterministic4.json", Deterministic, true);
}
