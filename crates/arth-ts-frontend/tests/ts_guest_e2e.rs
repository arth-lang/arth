use std::path::PathBuf;

use arth_ts_frontend::load_ts_guest_package;
use arth_vm::run_program;

/// End-to-end test for a TS guest program packaged and run on the Arth VM.
///
/// This test assumes that `examples/ts-guest-counter/main.ts` has been
/// compiled with `arth-ts package` into `target/ts-guest/main.tsguest.json`.
/// It exercises manifest loading, VM bytecode decoding, and execution.
#[test]
fn ts_guest_counter_runs_on_vm() {
    // Resolve manifest produced by `arth-ts package`.
    let manifest = PathBuf::from("target/ts-guest/main.tsguest.json");
    if !manifest.exists() {
        eprintln!(
            "skipping ts_guest_counter_runs_on_vm: manifest {} not found.\n\
             Run `arth-ts package examples/ts-guest-counter/main.ts --out-dir target/ts-guest` \
             before running this test.",
            manifest.display()
        );
        return;
    }

    let (_meta, program) =
        load_ts_guest_package(&manifest).expect("failed to load TS guest package");

    let code = run_program(&program);

    // We only assert that the guest exits successfully; the printed
    // logs are part of the example's behavior rather than the test.
    assert_eq!(code, 0, "TS guest program exited with non-zero code");
}
