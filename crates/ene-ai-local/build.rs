//! Build script: compile the JSON-schema → GBNF C++ shim used by local llama sampling.

fn main() {
    println!("cargo:rerun-if-changed=native/ene_grammar.cpp");
    println!("cargo:rerun-if-changed=native/ene_grammar.h");
    println!("cargo:rerun-if-changed=native/json-schema-to-grammar.cpp");
    println!("cargo:rerun-if-changed=native/json-schema-to-grammar.h");
    println!("cargo:rerun-if-changed=native/common_stub.h");

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("native/ene_grammar.cpp")
        .file("native/json-schema-to-grammar.cpp")
        .include("native")
        .warnings(false)
        .compile("ene_json_schema_grammar");
}
