#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

fn main() {
    if let Err(err) = ene_stage_poc::run(ene_stage_poc::PocMode::Baseline) {
        eprintln!("ene-stage-poc-baseline: {err}");
        std::process::exit(1);
    }
}
