# Utility Tools (`ene-tools-utility`)

**Binary:** `ene-tools-utility` | **Stateful:** Yes (TodoStore)

Provides helper tools for user interaction and task management.

## Tools

### `question`

Asks the user one or more clarifying questions.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | string[] | Yes | List of questions to ask the user |

**Use when:** Requirements are unclear, context is missing, or user confirmation is needed.

**Keywords:** question, ask, clarify, confirm

**Category:** Utility

---

### `todo`

Manages a session-scoped task list.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Complete updated todo list |

Each todo item:

| Field | Type | Required | Values |
|-------|------|----------|--------|
| `content` | string | Yes | Task description |
| `status` | string | No | `pending`, `in_progress`, `completed`, `cancelled` |
| `priority` | string | No | `high`, `medium`, `low` |

**State:** Persistent per session via `TodoStore` (DashMap-based in-memory). The store is cleared when the tool binary restarts.

**Keywords:** todo, task, track, plan

**Category:** Utility

---

### `get_current_time`

Returns the current system date and time.

| Parameter | Type |
|-----------|------|
| (none) | - |

**Output format:** `2026-05-26 14:30:00`

**Keywords:** time, date

**Category:** Utility

---

### `get_system_info`

Returns basic OS and architecture information.

| Parameter | Type |
|-----------|------|
| (none) | - |

**Output format:** `OS: linux, Architecture: x86_64`

**Keywords:** system, os, platform

**Category:** Utility
