//! Compile-check: verifies the `ene_tool::prelude` re-exports resolve.

use ene_tool::prelude::*;

#[test]
fn prelude_imports_compile() {
    // Reference a few types to ensure they are actually in scope.
    let _name = ToolName::new("smoke.test");
    let _err = ToolError::InvalidArguments {
        message: String::new(),
    };
    // DeriveToolSpec and DeriveToolAction are proc-macro derives; their
    // presence is verified by the `use` statement compiling successfully.
    let _: fn() = || {
        let _ = std::any::type_name::<SandboxConfigData>();
    };
}
