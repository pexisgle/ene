# Tool Core System Specifications & Overview

Ene features an isolated, secure tool ecosystem that allows the companion to interact safely with the host operating system. To guarantee sandbox isolation and prevent resource exhaustion, tools run in separate processes. This document outlines the module roles, dependency boundaries, and the two-layer ABI layout.

---

## 1. Tool Crate Directory

The tool ecosystem is partitioned into the following 6 internal crates:

| Crate | Path | Responsibility & Dependencies |
|---|---|---|
| `ene-plugin-proto` | `crates/ene-plugin-proto` | **Wire Protocol Layer**: Serialized IPC messages, socket transport, `ToolProvider` trait, and sandbox configuration data structures. |
| `ene-plugin-host` | `crates/ene-plugin-host` | **Orchestration Layer**: Subprocess life cycles, authorization environment variable provisioning, and MCP (Model Context Protocol) registry mapping. |
| `ene-tool-rag` | `crates/ene-tool-rag` | **Tool Retrieval (RAG) Layer**: Semantic vector indices, HyDE query expansion, and LLM-driven rerank logic. |
| `ene-tool-common`| `crates/ene-tool-common`| **Tool Utility Layer**: Reusable tools helper libraries (e.g. HTML-to-markdown translation). |
| `ene-tool-derive`| `crates/ene-tool-derive`| **Procedural Macros**: Generates schema definitions from code: `#[derive(ToolSpec)]`. |
| `ene-tool-db` | `crates/ene-tool-db` | **Database Proxy**: IPC-based CRUD wrapper client for tool database access. |

---

## 2. Two-Layer ABI Design

To maintain strict domain boundaries, the tool system uses a clear separation between the Wire layer and the Host layer:

1.  **Wire Layer (`ene-plugin-proto`)**:
    -   The interface implemented by the tool executable.
    -   The `ToolProvider` trait operates on this layer, passing raw arguments and returning response strings.
2.  **Host Layer (`ene-plugin-host`)**:
    -   Linked into the host actor runtime (`ene-runtime`).
    -   The `ToolRegistry` trait operates on this layer, grouping IPC tools, built-in tools, and remote MCP servers into a single interface.

---

## 3. Name Collision Fail-Closed Policy

If multiple tools register under the same name (e.g., `fs.write`), the LLM might target the wrong tool, posing security risks. Ene prevents this by raising a fatal error during registration:
*   `HostRegistry` throws `ToolError::DuplicateName` if a collision is found.
*   `CompositeToolRegistry` raises a fatal `ToolHostError::DuplicateToolName` at boot time.
