#include "ene_grammar.h"
#include "json-schema-to-grammar.h"

#include <cstdlib>
#include <cstring>
#include <exception>
#include <string>

#include <nlohmann/json.hpp>

extern "C" char * ene_json_schema_to_grammar(const char * schema_json, int force_gbnf) {
    if (schema_json == nullptr) {
        return nullptr;
    }
    try {
        const auto schema = nlohmann::ordered_json::parse(schema_json);
        const std::string grammar = json_schema_to_grammar(schema, force_gbnf != 0);
        char * out = static_cast<char *>(std::malloc(grammar.size() + 1));
        if (out == nullptr) {
            return nullptr;
        }
        std::memcpy(out, grammar.c_str(), grammar.size() + 1);
        return out;
    } catch (const std::exception &) {
        return nullptr;
    } catch (...) {
        return nullptr;
    }
}

extern "C" void ene_json_schema_grammar_free(char * ptr) {
    std::free(ptr);
}
