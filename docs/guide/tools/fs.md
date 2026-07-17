# Filesystem Tools (`ene-tool-fs`)

**Binary:** `ene-tool-fs` | **Stateful:** Yes (Sandbox + UndoManager via DB IPC)

Provides filesystem operations, shell execution, and undo. All file operations respect sandbox configuration.

## Tools

### `filesystem.read`

Read file contents or directory listings.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filePath` | string | Yes | Absolute path to file or directory |
| `offset` | integer | No | Line number to start reading from (1-indexed) |
| `limit` | integer | No | Maximum number of lines to read |

**Limit:** 50KB max read size.

---

### `filesystem.write`

Write or create a file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filePath` | string | Yes | Absolute path to the file |
| `content` | string | Yes | Content to write |

**Limit:** 1MB max write size. Creates parent directories automatically. Undo backup created.

---

### `filesystem.edit`

Targeted text replacement with 9 matching strategies.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filePath` | string | Yes | Absolute path to the file |
| `oldString` | string | Yes | Text to replace |
| `newString` | string | Yes | Replacement text |
| `replaceAll` | boolean | No | Replace all occurrences (default: false) |

**Matching strategies** (applied in order):
1. `trimmed_boundary` — Trim whitespace around boundaries
2. `simple` — Exact match
3. `whitespace_normalized` — Collapse whitespace differences
4. `escape_normalized` — Normalize escape sequences
5. `line_trimmed` — Trim whitespace from each line
6. `multi_occurrence` — Handle multiple occurrences
7. `indentation_flexible` — Ignore indentation differences
8. `context_aware` — Match using surrounding context
9. `block_anchor` — Anchor matching with code block detection

---

### `filesystem.delete`

Delete a file or directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Absolute path to file or directory |
| `recursive` | boolean | No | Delete directories recursively |

---

### `filesystem.glob`

Pattern-based file search.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | Yes | Glob pattern (e.g. `**/*.rs`) |
| `path` | string | No | Directory to search in (defaults to current) |

---

### `filesystem.grep`

Content-based regex search.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | Yes | Regex pattern to search for |
| `path` | string | No | Directory to search in |
| `include` | string | No | File pattern filter (e.g. `*.rs`) |

---

### `filesystem.patch`

Apply a unified diff patch.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `patchText` | string | Yes | Full unified diff text |

**Behavior:** Multi-file patches are grouped as a single undo entry.

---

### `shell.execute`

Execute shell commands with security enforcement.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | Yes | — | Shell command to execute |
| `description` | string | Yes | — | 5-10 word description of what the command does |
| `timeout` | integer | No | 120000 | Timeout in milliseconds |
| `workdir` | string | No | Current dir | Working directory |

**Security:** Commands checked against `blocked_commands` patterns. Output limited to 50KB / 2000 lines.

---

### `utility.undo`

Revert the most recent file operation.

| Parameter | Type | Description |
|-----------|------|-------------|
| (none) | — | — |

**Behavior:** Reverts write, edit, delete, and patch operations. Shell operations cannot be undone. Undo stack is persisted via the per-tool DB IPC server with zlib compression.

## Category

All filesystem tools: `Filesystem` | Shell: `Shell` | Undo: `Utility`

## Sandbox Integration

All file operations pass through `Sandbox::check_readable()` or `Sandbox::check_writable()`:

```
Request → Sandbox enabled?
  ├── No → Direct execution
  └── Yes
       ├── Path normalization
       ├── Directory allowlist check → reject if not in allowed
       ├── Shell: blocked_commands pattern check → reject if matched
       └── Execute with size/output limits
```
