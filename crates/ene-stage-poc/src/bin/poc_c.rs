#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

fn main() {
    if let Err(err) = ene_stage_poc::exp_c::run() {
        eprintln!("ene-stage-poc-c: {err}");
        std::process::exit(1);
    }
}
