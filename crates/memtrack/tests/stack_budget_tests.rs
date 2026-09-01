//! The stack copy budget is a frozen rodata constant, so the verifier's cost of
//! the capture program scales with it. Loading at the default proves nothing
//! about the maximum; both must load.
use memtrack::{BpfVariant, MemtrackBpf, TrackerOptions};
use rstest::rstest;

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case(8192)]
#[case(u32::MAX)]
#[test_log::test]
fn skeleton_loads_at_stack_budget(#[case] budget: u32) {
    for variant in [BpfVariant::Legacy, BpfVariant::Token] {
        let options = TrackerOptions::builder()
            .variant(Some(variant))
            .stack_budget(budget)
            .build();
        MemtrackBpf::new(&options).unwrap_or_else(|e| {
            panic!("{variant:?} skeleton failed to load at budget {budget}: {e:#}")
        });
    }
}
