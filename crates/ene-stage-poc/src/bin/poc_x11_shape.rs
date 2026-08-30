#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

fn main() {
    if let Err(err) = ene_stage_poc::exp_d2::run() {
        eprintln!("ene-stage-poc-x11-shape: {err}");
        std::process::exit(1);
    }
}
