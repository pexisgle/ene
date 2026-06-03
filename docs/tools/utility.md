# Utility Tools (`ene-tools-utility`)

**Binary:** `ene-tools-utility` | **Stateful:** Yes (TodoStore)

Provides helper tools for user interaction and task management.

## Tools

### `utility.question`

Asks the user one or more clarifying questions.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | string[] | Yes | List of questions to ask the user |

**Use when:** Requirements are unclear, context is missing, or user confirmation is needed.

**Category:** Utility

---

### `utility.todo_list`

Display the current task list.

| Parameter | Type | Description |
|-----------|------|-------------|
| (none) | — | — |

---

### `utility.todo_add`

Add tasks to the session-scoped todo list.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Tasks to add |

Each todo item:

| Field | Type | Required | Values |
|-------|------|----------|--------|
| `content` | string | Yes | Task description |
| `status` | string | No | `pending`, `in_progress`, `completed`, `cancelled` |
| `priority` | string | No | `high`, `medium`, `low` |

---

### `utility.todo_update`

Update existing tasks.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Updated todo items |

---

### `utility.todo_complete`

Mark tasks as completed.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Tasks to mark complete |

---

### `utility.todo_delete`

Remove tasks from the list.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Tasks to remove |

**State:** Persistent per session via `TodoStore` (DashMap-based in-memory). The store is cleared when the tool binary restarts.

---

### `utility.get_current_time`

Returns the current system date and time.

| Parameter | Type |
|-----------|------|
| (none) | - |

**Output format:** `2026-05-26 14:30:00`

**Category:** Utility

---

### `utility.get_system_info`

Returns basic OS and architecture information.

| Parameter | Type |
|-----------|------|
| (none) | - |

**Output format:** `OS: linux, Architecture: x86_64`

**Category:** Utility
