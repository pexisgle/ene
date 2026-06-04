# Utility Tools (`ene-tools-utility`)

**Binary:** `ene-tools-utility` | **Stateful:** Yes (TodoDb — SQLite-backed, session-scoped)

Provides helper tools for user interaction and task management.

## Tools

### `utility.question`

Asks the user one or more clarifying questions. Pauses tool execution and waits for interactive user input.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | QuestionItem[] | Yes | List of questions with options |

Each `QuestionItem`:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `question` | string | Yes | The question text |
| `options` | string[] | Yes | Selectable options |
| `allow_free_text` | boolean | No | Allow the user to type a free-form answer |

**Behavior:** On first call, returns `ToolError::UserInputRequired` which causes the stream to emit `EneEvent::UserInputRequired`. The consumer displays an interactive dialog. When the user responds, the host injects `_user_answers` into the args and re-calls the tool, which returns the formatted answers.

**Use when:** Requirements are unclear, context is missing, or user confirmation is needed.

**Category:** Utility

---

### `utility.todo_list`

Display the current task list for the active session.

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
| `parent_id` | integer | No | Parent todo ID for hierarchy |

---

### `utility.todo_update`

Update existing tasks. Supports tri-state `parent_id`: absent = skip, `null` = detach from parent, integer = reparent.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Updated todo items |

---

### `utility.todo_complete`

Mark tasks as completed. Completing a parent cascades to all descendants (BFS).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Tasks to mark complete |

---

### `utility.todo_delete`

Soft-delete tasks (sets status to `cancelled`).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | object[] | Yes | Tasks to remove |

**State:** Persistent per session via `TodoDb` (SQLite with embedded migrations, WAL mode). Each session's todos are isolated by `session_id`. Survives tool binary restarts within the same session. Supports parent/child hierarchy with cycle detection.

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
