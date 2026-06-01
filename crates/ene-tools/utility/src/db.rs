use chrono::Utc;
use dashmap::DashMap;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, RwLock};
use thiserror::Error;

use crate::schema::todo_items;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Errors produced by the todo subsystem.
#[derive(Debug, Error)]
pub enum TodoError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] diesel::result::Error),
    #[error("Invalid status: {0} (expected pending|in_progress|completed|cancelled)")]
    InvalidStatus(String),
    #[error("Invalid priority: {0} (expected high|medium|low)")]
    InvalidPriority(String),
    #[error("Content must not be empty")]
    EmptyContent,
    #[error("Todo {0} not found in this session")]
    NotFound(i64),
    #[error("Parent todo {0} not found in this session")]
    ParentNotFound(i64),
    #[error("Cannot reparent todo {child} under its own descendant {parent}")]
    Cycle { child: i64, parent: i64 },
    #[error("DB not initialized — set_db_path was not called or failed")]
    DbNotInitialized,
    #[error("Invalid DB path: {0}")]
    InvalidPath(String),
}

const VALID_STATUSES: &[&str] = &["pending", "in_progress", "completed", "cancelled"];
const VALID_PRIORITIES: &[&str] = &["high", "medium", "low"];

/// A single todo item, either in-memory or freshly fetched from the DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub content: String,
    pub status: String,
    pub priority: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = todo_items)]
struct TodoRow {
    id: i32,
    #[allow(dead_code)]
    session_id: String,
    parent_id: Option<i32>,
    content: String,
    status: String,
    priority: String,
    created_at: String,
    updated_at: String,
}

impl From<TodoRow> for TodoItem {
    fn from(r: TodoRow) -> Self {
        Self {
            id: r.id as i64,
            parent_id: r.parent_id.map(|p| p as i64),
            content: r.content,
            status: r.status,
            priority: r.priority,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Per-session todo storage with an in-memory cache and SQLite persistence.
///
/// The cache (`DashMap<session_id, Vec<TodoItem>>`) is invalidated on every
/// write so subsequent reads always re-hydrate from the DB. This mirrors
/// `ene-tools/fs/src/utils/undo_manager::UndoManager` and gives us a single
/// `Mutex<Option<SqliteConnection>>` for the actual database handle.
pub struct TodoDb {
    cache: DashMap<String, Vec<TodoItem>>,
    conn: Mutex<Option<SqliteConnection>>,
    session_id: RwLock<String>,
}

impl Default for TodoDb {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoDb {
    /// Creates a new `TodoDb` with no DB file opened. Calls requiring the DB
    /// will fail with [`TodoError::DbNotInitialized`] until
    /// [`set_db_path`](Self::set_db_path) succeeds.
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
            conn: Mutex::new(None),
            session_id: RwLock::new("default".to_string()),
        }
    }

    /// Opens the SQLite database at `path` (creating it if missing), enables
    /// WAL mode, and runs the embedded migrations. The connection handle is
    /// stored and used for all subsequent operations.
    pub fn set_db_path(&self, path: &Path) -> Result<(), TodoError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| TodoError::InvalidPath(format!("{:?}", path)))?;
        let mut conn = SqliteConnection::establish(path_str)
            .map_err(|e| TodoError::InvalidPath(format!("establish({path_str}): {e}")))?;
        conn.batch_execute("PRAGMA journal_mode=WAL;")
            .map_err(|e| TodoError::InvalidPath(format!("PRAGMA WAL: {e}")))?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| TodoError::InvalidPath(format!("migrations: {e}")))?;
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn);
        // Drop any cached data from a previous DB file.
        self.cache.clear();
        Ok(())
    }

    /// Updates the current session ID. All subsequent operations are scoped
    /// to this session until a new ID is set.
    pub fn set_session_id(&self, session_id: &str) {
        let mut guard = self.session_id.write().unwrap_or_else(|e| e.into_inner());
        *guard = session_id.to_string();
    }

    /// Returns the current session ID.
    pub fn current_session_id(&self) -> String {
        self.session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn invalidate_cache(&self) {
        let sid = self.current_session_id();
        self.cache.remove(&sid);
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Option<SqliteConnection>>, TodoError> {
        Ok(self.conn.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn load_session_from_db(&self, session_id: &str) -> Result<Vec<TodoItem>, TodoError> {
        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;

        let rows: Vec<TodoRow> = todo_items::table
            .filter(todo_items::session_id.eq(session_id))
            .order((todo_items::parent_id.asc(), todo_items::id.asc()))
            .select(TodoRow::as_select())
            .load(conn)?;

        let items: Vec<TodoItem> = rows.into_iter().map(TodoItem::from).collect();
        self.cache
            .insert(session_id.to_string(), items.clone());
        Ok(items)
    }

    /// Returns the current session's todo items, loading from the DB on the
    /// first access for the session.
    pub fn list(&self) -> Result<Vec<TodoItem>, TodoError> {
        let sid = self.current_session_id();
        if let Some(cached) = self.cache.get(&sid) {
            return Ok(cached.clone());
        }
        self.load_session_from_db(&sid)
    }

    /// Adds a new todo. If `parent_id` is `Some`, the parent must exist in the
    /// current session. Returns the new row's ID.
    pub fn add(
        &self,
        parent_id: Option<i64>,
        content: &str,
        priority: &str,
    ) -> Result<TodoItem, TodoError> {
        if content.trim().is_empty() {
            return Err(TodoError::EmptyContent);
        }
        if !VALID_PRIORITIES.contains(&priority) {
            return Err(TodoError::InvalidPriority(priority.to_string()));
        }

        let sid = self.current_session_id();
        let now = Utc::now().to_rfc3339();

        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;

        if let Some(pid) = parent_id {
            // Parent must exist in the same session.
            let exists: bool = diesel::select(diesel::dsl::exists(
                todo_items::table
                    .filter(todo_items::id.eq(pid as i32))
                    .filter(todo_items::session_id.eq(&sid)),
            ))
            .get_result(conn)?;
            if !exists {
                return Err(TodoError::ParentNotFound(pid));
            }
        }

        let new_parent: Option<i32> = parent_id.map(|p| p as i32);
        diesel::insert_into(todo_items::table)
            .values((
                todo_items::session_id.eq(&sid),
                todo_items::parent_id.eq(new_parent),
                todo_items::content.eq(content),
                todo_items::status.eq("pending"),
                todo_items::priority.eq(priority),
                todo_items::created_at.eq(&now),
                todo_items::updated_at.eq(&now),
            ))
            .execute(conn)?;

        let new_id: i32 = diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
            "last_insert_rowid()",
        ))
        .get_result(conn)?;
        drop(guard);

        self.invalidate_cache();
        self.fetch_one(new_id as i64)
    }

    /// Partial update. Any `None` field is left unchanged. `parent_id` is a
    /// tri-state: `None` skips the column, `Some(None)` detaches the item
    /// (becomes a root), `Some(Some(id))` reparents under the given ID.
    pub fn update(
        &self,
        id: i64,
        content: Option<&str>,
        status: Option<&str>,
        priority: Option<&str>,
        parent_id: Option<Option<i64>>,
    ) -> Result<TodoItem, TodoError> {
        if let Some(c) = content
            && c.trim().is_empty()
        {
            return Err(TodoError::EmptyContent);
        }
        if let Some(s) = status
            && !VALID_STATUSES.contains(&s)
        {
            return Err(TodoError::InvalidStatus(s.to_string()));
        }
        if let Some(p) = priority
            && !VALID_PRIORITIES.contains(&p)
        {
            return Err(TodoError::InvalidPriority(p.to_string()));
        }

        let sid = self.current_session_id();
        let now = Utc::now().to_rfc3339();

        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;

        // Make sure the row exists in this session.
        let exists: bool = diesel::select(diesel::dsl::exists(
            todo_items::table
                .filter(todo_items::id.eq(id as i32))
                .filter(todo_items::session_id.eq(&sid)),
        ))
        .get_result(conn)?;
        if !exists {
            return Err(TodoError::NotFound(id));
        }

        if let Some(new_parent_opt) = parent_id {
            if let Some(new_parent) = new_parent_opt {
                // Validate the new parent exists in the same session.
                let parent_exists: bool = diesel::select(diesel::dsl::exists(
                    todo_items::table
                        .filter(todo_items::id.eq(new_parent as i32))
                        .filter(todo_items::session_id.eq(&sid)),
                ))
                .get_result(conn)?;
                if !parent_exists {
                    return Err(TodoError::ParentNotFound(new_parent));
                }
                // Cycle check: new_parent must not be a descendant of `id`.
                if is_descendant(conn, &sid, new_parent, id)? {
                    return Err(TodoError::Cycle {
                        child: id,
                        parent: new_parent,
                    });
                }
                diesel::update(todo_items::table.filter(todo_items::id.eq(id as i32)))
                    .set((
                        todo_items::parent_id.eq(Some(new_parent as i32)),
                        todo_items::updated_at.eq(&now),
                    ))
                    .execute(conn)?;
            } else {
                // Detach.
                diesel::update(todo_items::table.filter(todo_items::id.eq(id as i32)))
                    .set((
                        todo_items::parent_id.eq::<Option<i32>>(None),
                        todo_items::updated_at.eq(&now),
                    ))
                    .execute(conn)?;
            }
        }

        if let Some(c) = content {
            diesel::update(todo_items::table.filter(todo_items::id.eq(id as i32)))
                .set((todo_items::content.eq(c), todo_items::updated_at.eq(&now)))
                .execute(conn)?;
        }
        if let Some(s) = status {
            diesel::update(todo_items::table.filter(todo_items::id.eq(id as i32)))
                .set((todo_items::status.eq(s), todo_items::updated_at.eq(&now)))
                .execute(conn)?;
        }
        if let Some(p) = priority {
            diesel::update(todo_items::table.filter(todo_items::id.eq(id as i32)))
                .set((todo_items::priority.eq(p), todo_items::updated_at.eq(&now)))
                .execute(conn)?;
        }
        drop(guard);

        self.invalidate_cache();
        self.fetch_one(id)
    }

    /// Marks a todo and all its descendants as `completed`. Returns the IDs
    /// of all rows updated (the root plus every cascaded descendant).
    pub fn complete(&self, id: i64) -> Result<Vec<i64>, TodoError> {
        let sid = self.current_session_id();
        let now = Utc::now().to_rfc3339();
        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;

        let exists: bool = diesel::select(diesel::dsl::exists(
            todo_items::table
                .filter(todo_items::id.eq(id as i32))
                .filter(todo_items::session_id.eq(&sid)),
        ))
        .get_result(conn)?;
        if !exists {
            return Err(TodoError::NotFound(id));
        }

        // Gather all ids in the subtree rooted at `id` (inclusive), BFS.
        let mut all_ids: Vec<i32> = vec![id as i32];
        let mut frontier: Vec<i32> = vec![id as i32];
        while !frontier.is_empty() {
            let children: Vec<i32> = todo_items::table
                .filter(todo_items::session_id.eq(&sid))
                .filter(todo_items::parent_id.eq_any(&frontier))
                .select(todo_items::id)
                .load(conn)?;
            frontier = children.clone();
            all_ids.extend(children);
        }

        diesel::update(
            todo_items::table.filter(todo_items::id.eq_any(&all_ids)),
        )
        .set((
            todo_items::status.eq("completed"),
            todo_items::updated_at.eq(&now),
        ))
        .execute(conn)?;
        drop(guard);

        self.invalidate_cache();
        Ok(all_ids.into_iter().map(|i| i as i64).collect())
    }

    /// Soft-deletes a todo by setting `status='cancelled'`. Descendants are
    /// not cascaded — to cancel a whole sub-tree, call this for each item
    /// individually. The row is kept in the DB for history.
    pub fn delete(&self, id: i64) -> Result<TodoItem, TodoError> {
        self.update(id, None, Some("cancelled"), None, None)
    }

    /// Soft-cancels every todo in the current session. Returns the number of
    /// rows affected.
    pub fn clear(&self) -> Result<usize, TodoError> {
        let sid = self.current_session_id();
        let now = Utc::now().to_rfc3339();
        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;
        let n = diesel::update(todo_items::table.filter(todo_items::session_id.eq(&sid)))
            .set((
                todo_items::status.eq("cancelled"),
                todo_items::updated_at.eq(&now),
            ))
            .execute(conn)?;
        drop(guard);
        self.invalidate_cache();
        Ok(n)
    }

    fn fetch_one(&self, id: i64) -> Result<TodoItem, TodoError> {
        let sid = self.current_session_id();
        let mut guard = self.lock_conn()?;
        let conn = guard.as_mut().ok_or(TodoError::DbNotInitialized)?;
        let row: TodoRow = todo_items::table
            .filter(todo_items::id.eq(id as i32))
            .filter(todo_items::session_id.eq(&sid))
            .select(TodoRow::as_select())
            .first(conn)?;
        Ok(TodoItem::from(row))
    }
}

/// Returns `true` if `candidate` is in the subtree rooted at `root` (excluding
/// `root` itself, so a candidate equal to root is "not a descendant").
fn is_descendant(
    conn: &mut SqliteConnection,
    session_id: &str,
    candidate: i64,
    root: i64,
) -> Result<bool, TodoError> {
    let mut frontier: Vec<i32> = vec![root as i32];
    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    while !frontier.is_empty() {
        let children: Vec<i32> = todo_items::table
            .filter(todo_items::session_id.eq(session_id))
            .filter(todo_items::parent_id.eq_any(&frontier))
            .select(todo_items::id)
            .load(conn)?;
        let mut next = Vec::new();
        for c in children {
            if c as i64 == candidate {
                return Ok(true);
            }
            if seen.insert(c) {
                next.push(c);
            }
        }
        frontier = next;
    }
    Ok(false)
}

/// Formats a todo list as a human-readable tree (for debug / CLI display).
/// LLM-facing responses should use the JSON `Vec<TodoItem>` from
/// [`TodoDb::list`] instead.
#[allow(dead_code)]
pub fn format_tree(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No todos.".to_string();
    }
    let mut roots: Vec<&TodoItem> = items.iter().filter(|i| i.parent_id.is_none()).collect();
    roots.sort_by_key(|i| i.id);
    let mut out = String::new();
    for root in &roots {
        render_node(&mut out, items, root, 0);
    }
    out
}

#[allow(dead_code)]
fn render_node(out: &mut String, items: &[TodoItem], node: &TodoItem, depth: usize) {
    let icon = match node.status.as_str() {
        "completed" => "[x]",
        "cancelled" => "[-]",
        "in_progress" => "[~]",
        _ => "[ ]",
    };
    let indent = "  ".repeat(depth);
    out.push_str(&format!(
        "{indent}{icon} {} [{}] (id={}, priority: {})\n",
        node.content, node.status, node.id, node.priority
    ));
    let mut children: Vec<&TodoItem> = items
        .iter()
        .filter(|i| i.parent_id == Some(node.id))
        .collect();
    children.sort_by_key(|i| i.id);
    for c in &children {
        render_node(out, items, c, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> TodoDb {
        let db = TodoDb::new();
        db.set_db_path(Path::new(":memory:")).unwrap();
        db.set_session_id("sess_test");
        db
    }

    #[test]
    fn add_and_list() {
        let db = fresh_db();
        let item = db.add(None, "Task A", "high").unwrap();
        assert_eq!(item.content, "Task A");
        assert_eq!(item.priority, "high");
        assert_eq!(item.status, "pending");
        assert!(item.parent_id.is_none());

        let list = db.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, item.id);
    }

    #[test]
    fn add_child_with_parent() {
        let db = fresh_db();
        let parent = db.add(None, "Parent", "medium").unwrap();
        let child = db.add(Some(parent.id), "Child", "low").unwrap();
        assert_eq!(child.parent_id, Some(parent.id));

        let tree = format_tree(&db.list().unwrap());
        assert!(tree.contains("Parent"));
        assert!(tree.contains("Child"));
    }

    #[test]
    fn add_with_invalid_parent_errors() {
        let db = fresh_db();
        let err = db.add(Some(9999), "Orphan", "low").unwrap_err();
        assert!(matches!(err, TodoError::ParentNotFound(9999)));
    }

    #[test]
    fn add_with_invalid_priority_errors() {
        let db = fresh_db();
        let err = db.add(None, "x", "wrong").unwrap_err();
        assert!(matches!(err, TodoError::InvalidPriority(_)));
    }

    #[test]
    fn add_with_empty_content_errors() {
        let db = fresh_db();
        let err = db.add(None, "  ", "low").unwrap_err();
        assert!(matches!(err, TodoError::EmptyContent));
    }

    #[test]
    fn update_partial_fields() {
        let db = fresh_db();
        let item = db.add(None, "x", "low").unwrap();
        let updated = db
            .update(item.id, Some("y"), Some("in_progress"), None, None)
            .unwrap();
        assert_eq!(updated.content, "y");
        assert_eq!(updated.status, "in_progress");
        assert_eq!(updated.priority, "low");
    }

    #[test]
    fn update_reparent_and_detach() {
        let db = fresh_db();
        let p1 = db.add(None, "p1", "high").unwrap();
        let p2 = db.add(None, "p2", "high").unwrap();
        let c = db.add(Some(p1.id), "c", "low").unwrap();

        // Reparent under p2.
        let updated = db
            .update(c.id, None, None, None, Some(Some(p2.id)))
            .unwrap();
        assert_eq!(updated.parent_id, Some(p2.id));

        // Detach.
        let updated = db
            .update(c.id, None, None, None, Some(None))
            .unwrap();
        assert!(updated.parent_id.is_none());
    }

    #[test]
    fn update_reparent_to_descendant_cycles() {
        let db = fresh_db();
        let parent = db.add(None, "p", "high").unwrap();
        let child = db.add(Some(parent.id), "c", "low").unwrap();
        let err = db
            .update(parent.id, None, None, None, Some(Some(child.id)))
            .unwrap_err();
        assert!(matches!(err, TodoError::Cycle { .. }));
    }

    #[test]
    fn update_invalid_status() {
        let db = fresh_db();
        let i = db.add(None, "x", "low").unwrap();
        let err = db.update(i.id, None, Some("nope"), None, None).unwrap_err();
        assert!(matches!(err, TodoError::InvalidStatus(_)));
    }

    #[test]
    fn complete_cascades_to_descendants() {
        let db = fresh_db();
        let root = db.add(None, "root", "high").unwrap();
        let c1 = db.add(Some(root.id), "c1", "low").unwrap();
        let c2 = db.add(Some(root.id), "c2", "low").unwrap();
        let gc = db.add(Some(c1.id), "gc", "low").unwrap();

        let cascaded = db.complete(root.id).unwrap();
        let mut want = vec![root.id, c1.id, c2.id, gc.id];
        want.sort();
        let mut got = cascaded.clone();
        got.sort();
        assert_eq!(got, want);

        let list = db.list().unwrap();
        for it in &list {
            assert_eq!(it.status, "completed");
        }
    }

    #[test]
    fn delete_soft_cancels() {
        let db = fresh_db();
        let i = db.add(None, "x", "low").unwrap();
        let after = db.delete(i.id).unwrap();
        assert_eq!(after.status, "cancelled");
        // Row still present.
        let list = db.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, "cancelled");
    }

    #[test]
    fn delete_does_not_cascade() {
        let db = fresh_db();
        let p = db.add(None, "p", "high").unwrap();
        let c = db.add(Some(p.id), "c", "low").unwrap();
        db.delete(p.id).unwrap();
        let list = db.list().unwrap();
        let c_after = list.iter().find(|i| i.id == c.id).unwrap();
        assert_eq!(c_after.status, "pending");
    }

    #[test]
    fn clear_session() {
        let db = fresh_db();
        db.add(None, "a", "low").unwrap();
        db.add(None, "b", "low").unwrap();
        let n = db.clear().unwrap();
        assert_eq!(n, 2);
        for it in &db.list().unwrap() {
            assert_eq!(it.status, "cancelled");
        }
    }

    #[test]
    fn cross_session_isolation() {
        let db = fresh_db();
        db.add(None, "sess1-item", "low").unwrap();
        db.set_session_id("sess_2");
        assert!(db.list().unwrap().is_empty());
        db.add(None, "sess2-item", "high").unwrap();
        let list = db.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].content, "sess2-item");
    }

    #[test]
    fn format_tree_indents_children() {
        let db = fresh_db();
        let p = db.add(None, "P", "high").unwrap();
        let c = db.add(Some(p.id), "C", "low").unwrap();
        let gc = db.add(Some(c.id), "GC", "low").unwrap();
        let tree = format_tree(&db.list().unwrap());
        let lines: Vec<&str> = tree.lines().collect();
        assert!(lines[0].starts_with("[ ] P"));
        assert!(lines[1].starts_with("  [ ] C"));
        assert!(lines[2].starts_with("    [ ] GC"));
        let gc_id_str = format!("id={}", gc.id);
        assert!(lines.iter().any(|l| l.contains(&gc_id_str)));
    }
}
