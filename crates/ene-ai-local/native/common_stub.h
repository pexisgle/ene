#pragma once

#include <sstream>
#include <string>
#include <vector>

// Minimal helpers formerly provided by llama.cpp common/common.h,
// enough for json-schema-to-grammar.cpp.

inline std::string string_join(const std::vector<std::string> & values,
                               const std::string & separator) {
    std::ostringstream out;
    for (size_t i = 0; i < values.size(); ++i) {
        if (i > 0) {
            out << separator;
        }
        out << values[i];
    }
    return out.str();
}

inline std::vector<std::string> string_split(const std::string & str,
                                             const std::string & delimiter) {
    std::vector<std::string> parts;
    size_t start = 0;
    size_t end = str.find(delimiter);

    while (end != std::string::npos) {
        parts.push_back(str.substr(start, end - start));
        start = end + delimiter.length();
        end = str.find(delimiter, start);
    }

    parts.push_back(str.substr(start));
    return parts;
}

inline std::string string_repeat(const std::string & str, size_t n) {
    if (n == 0) {
        return "";
    }

    std::string result;
    result.reserve(str.length() * n);
    for (size_t i = 0; i < n; ++i) {
        result += str;
    }
    return result;
}
