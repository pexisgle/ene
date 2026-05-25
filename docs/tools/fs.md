# Filesystem Tools (`ene-tools-fs`)

**Binary:** `ene-tools-fs` | **Stateful:** Yes (Sandbox + UndoManager)

Provides filesystem operations, shell execution, and undo. All file operations respect sandbox configuration.

## Tools

### `filesystem`

Unified mega-tool for file operations. Action-based dispatch.

| Action | Parameters | Description |
|--------|-----------|-------------|
| `read` | `filePath`*, `offset?`, `limit?` | Read file with optional line range, 50KB limit |
| `write` | `filePath`*, `content`* | Write/create file, 1MB limit, undo backup |
| `edit` | `filePath`*, `oldString`*, `newString`*, `replaceAll?` | Text replacement with 9 matching strategies, undo support |
| `delete` | `path`*, `recursive?` | Delete file or directory |
| `glob` | `pattern`*, `path?` | Pattern-based file search |
| `grep` | `pattern`*, `path?`, `include?` | Content-based regex search |
| `patch` | `patchText`* | Apply unified diff, multi-file undo as single entry |

**Keywords:** file, read, write, edit, delete, search, glob, grep, patch, directory, replace

**Category:** Filesystem

---

### `shell`

Executes shell commands with security enforcement.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | Yes | - | Shell command to execute |
| `description` | string | Yes | - | 5-10 word description of what the command does |
| `timeout` | integer | No | 120000 | Timeout in milliseconds |
| `workdir` | string | No | Current dir | Working directory |

**Security:**
- All commands are checked against `blocked_commands` patterns
- Output limited to `max_shell_output_bytes` (50KB) and `max_shell_output_lines` (2000)
- 120-second timeout by default
- Use `workdir` parameter instead of `cd &&` patterns

**Keywords:** shell, command, execute, terminal, bash

**Category:** Shell

---

### `undo`

Reverts the most recent file operation.

| Parameter | Type |
|-----------|------|
| (none) | - |

**Behavior:**
- Reverts write, edit, delete, and patch operations
- Can be called multiple times to undo multiple operations
- Shell operations cannot be undone
- Uses a SQLite-backed undo stack with zlib compression

**Keywords:** undo, revert, rollback

**Category:** Utility

## Edit Strategies

The `edit` action in `filesystem` uses 9 matching strategies applied in order:

1. `trimmed_boundary` — Trim whitespace around boundaries
2. `simple` — Exact match
3. `whitespace_normalized` — Collapse whitespace differences
4. `escape_normalized` — Normalize escape sequences
5. `line_trimmed` — Trim whitespace from each line
6. `multi_occurrence` — Handle multiple occurrences
7. `indentation_flexible` — Ignore indentation differences
8. `context_aware` — Match using surrounding context
9. `block_anchor` — Anchor matching with code block detection

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
