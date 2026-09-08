fn main() {
    println!("cargo:rerun-if-changed=ui");
    if slint_build::compile("ui/lib.slint").is_err() {
        std::process::exit(1);
    }
}
