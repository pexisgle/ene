#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

fn main() {
    if let Err(err) = ene_stage_poc::exp_d::run() {
        eprintln!("ene-stage-poc-d: {err}");
        std::process::exit(1);
    }
}
