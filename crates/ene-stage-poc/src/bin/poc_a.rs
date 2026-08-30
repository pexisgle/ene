#![expect(clippy::print_stderr, reason = "fatal error path for a PoC binary")]

fn main() {
    if let Err(err) = ene_stage_poc::run(ene_stage_poc::PocMode::Composition) {
        eprintln!("ene-stage-poc-a: {err}");
        std::process::exit(1);
    }
}
