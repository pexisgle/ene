# `ToolSpec` & `ToolAction` Derive Macro Specs (`ene-tool-derive`)

The `ene-tool-derive` crate contains procedural macros that compile structs into function schema maps (`ToolSpec`) and execution interfaces (`ToolAction`) for LLM interactions.

---

## 1. Macro Entry Points (`lib.rs`)

#### `derive_tool_spec`
*   **Signature**: `pub fn derive_tool_spec(input: TokenStream) -> TokenStream`
*   **Description**: The entry point for `#[derive(ToolSpec)]`. Parses the syntax tree, resolves container and field attributes, and emits token streams implementing spec metadata properties.

#### `derive_tool_action`
*   **Signature**: `pub fn derive_tool_action(input: TokenStream) -> TokenStream`
*   **Description**: The entry point for `#[derive(ToolAction)]`. Implements parameter deserialization and delegates execution to custom `run` functions.

#### `tool_action`
*   **Signature**: `pub fn tool_action(attr: TokenStream, input: TokenStream) -> TokenStream`
*   **Description**: Attribute macro helper facilitating action declarations.

---

## 2. Macro Expansion Logic (`lib.rs`)

#### `expand_tool_spec`
*   **Signature**: `fn expand_tool_spec(ast: &DeriveInput) -> syn::Result<TokenStream2>`
*   **Process**:
    1.  Parses container attributes (`#[tool(...)]`).
    2.  Resolves struct fields and extracts schema properties via `collect_field_instructions`.
    3.  Builds the output JSON Schema using `schemars`, forcing `additionalProperties: false`.
    4.  Emits `TOOL_NAME` and `DISPLAY_NAME` constants.
    5.  Implements the metadata trait getters.

#### `expand_tool_action_derive`
*   **Signature**: `fn expand_tool_action_derive(ast: &DeriveInput) -> syn::Result<TokenStream2>`
*   **Description**: Implements the execution pipeline for structs decorated with `#[derive(ToolAction)]`.

#### `expand_tool_action`
*   **Signature**: `fn expand_tool_action(item: &mut syn::ItemImpl, args_ty: &syn::Type)`
*   **Description**: Helper generating trait implementations.

#### `collect_field_instructions`
*   **Signature**: `fn collect_field_instructions(fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>) -> syn::Result<Vec<TokenStream2>>`
*   **Description**: Scans struct fields, evaluates parameter properties (`#[arg(...)]`), doc comments, and filters out skipped parameters.

#### `apply_serde_attrs`
*   **Signature**: `fn apply_serde_attrs(f: &syn::Field, instr: &mut FieldInstr)`
*   **Description**: Maps `#[serde(...)]` settings (such as renaming or default parameters) to ensure tool schemas align with deserializers.

#### `emit_field`
*   **Signature**: `fn emit_field(instr: &FieldInstr) -> TokenStream2`
*   **Description**: Emits JSON Schema builders for a field.

---

## 3. Attribute Parsers & Token Builders (`attr.rs`)

The `attr` module decodes parameters using `darling`.

#### `is_hidden`
*   **Signature**: `pub const fn is_hidden(&self) -> bool`
*   **Description**: Returns `true` if the tool is flagged to be hidden from search schemas.

#### `has_tool_skip`
*   **Signature**: `pub fn has_tool_skip(field: &syn::Field) -> bool`
*   **Description**: Returns `true` if a field is decorated with `#[tool(skip)]`.

#### `full_name`
*   **Signature**: `pub fn full_name(&self) -> String`
*   **Description**: Combines namespaces and names (e.g. `fs.read`).

#### `display_name_value`
*   **Signature**: `pub fn display_name_value(&self, _default: String) -> String`
*   **Description**: Extracts the tool's display name.

#### `summary_value`
*   **Signature**: `pub fn summary_value(&self) -> darling::Result<String>`
*   **Description**: Resolves summary values.

#### `description_value`
*   **Signature**: `pub fn description_value(&self) -> String`
*   **Description**: Resolves descriptions.

#### `category_path` / `side_effects_path`
*   **Signature**: `pub fn category_path(&self) -> TokenStream2` (same pattern for side effects)
*   **Description**: Emits token values for categories and side-effects.

#### `keywords_list` / `string_list` / `related_list`
*   **Signature**: `pub fn keywords_list(&self, kind: &str) -> Vec<String>` (same patterns)
*   **Description**: Normalizes attribute arrays.

#### `version_tokens`
*   **Signature**: `pub fn version_tokens(&self) -> TokenStream2`
*   **Description**: Resolves version inputs into token values.

#### `examples_value`
*   **Signature**: `pub fn examples_value(&self) -> TokenStream2`
*   **Description**: Formats example arrays.

#### `args_const_ident`
*   **Signature**: `pub fn args_const_ident(&self, struct_ident: &syn::Ident) -> syn::Ident`
*   **Description**: Generates constant identifiers.

#### `parse_version`
*   **Signature**: `fn parse_version(s: &str) -> Option<(u32, u32, u32)>`
*   **Description**: Parses semver version strings into major, minor, and patch numbers.

#### `title_case`
*   **Signature**: `fn title_case(s: &str) -> String`
*   **Description**: Converts strings to title case.

#### `path_token`
*   **Signature**: `fn path_token(name: &str, default_module: &str) -> TokenStream2`
*   **Description**: Resolves module paths.
