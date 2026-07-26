#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/// Convert a JSON Schema document to a GBNF grammar string.
///
/// On success returns a heap-allocated NUL-terminated C string that the
/// caller must free with [`ene_json_schema_grammar_free`].
/// On failure returns `NULL`.
char * ene_json_schema_to_grammar(const char * schema_json, int force_gbnf);

void ene_json_schema_grammar_free(char * ptr);

#ifdef __cplusplus
}
#endif
