//! Data model shared between tool binaries and the core DB server.
//!
//! This module defines the wire-level types used to describe and manipulate
//! tool databases: dynamically-typed values ([`DbValue`]) and rows ([`Row`]),
//! structured query filters ([`DbFilter`]) and ordering ([`DbOrderBy`]), and
//! schema declarations ([`DbSchema`], [`DbTable`], [`DbColumn`], [`DbIndex`],
//! [`DbType`]). All types serialize to JSON for transport over the DB IPC
//! protocol.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A single database row represented as a map of column names to values.
pub type Row = BTreeMap<String, DbValue>;

/// Builds a [`Row`] from `"column" => value` pairs.
///
/// Column keys accept anything implementing `Into<String>` and values
/// anything implementing `Into<DbValue>`, so plain literals work via the
/// [`From`] impls on [`DbValue`].
///
/// A `FromIterator` impl is not possible here because [`Row`] is a type alias
/// for a foreign type (the orphan rule), hence this macro.
///
/// # Examples
///
/// ```
/// use ene_plugin_db::{row, DbValue};
///
/// let row = row! {
///     "name" => DbValue::Text("Alice".into()),
///     "age" => DbValue::Int(30),
/// };
/// assert_eq!(row.len(), 2);
/// ```
#[macro_export]
macro_rules! row {
    () => {
        $crate::Row::new()
    };
    ($($column:expr => $value:expr),+ $(,)?) => {{
        let mut row = $crate::Row::new();
        $(
            row.insert($column.into(), $value.into());
        )+
        row
    }};
}

/// A dynamically-typed database value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbValue {
    /// SQL NULL.
    Null,
    /// Boolean value.
    Bool(bool),
    /// 64-bit signed integer.
    Int(i64),
    /// 64-bit floating point.
    Float(f64),
    /// UTF-8 text string.
    Text(String),
    /// Binary blob.
    Blob(Vec<u8>),
}

impl DbValue {
    /// Returns the contained integer if this value is [`Int`](Self::Int).
    pub const fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the contained text if this value is [`Text`](Self::Text).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }

    /// Returns the contained boolean if this value is [`Bool`](Self::Bool).
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the contained float if this value is [`Float`](Self::Float).
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the contained bytes if this value is [`Blob`](Self::Blob).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(v) => Some(v),
            _ => None,
        }
    }
}

impl From<bool> for DbValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i32> for DbValue {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<i64> for DbValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<f64> for DbValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<String> for DbValue {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<&str> for DbValue {
    fn from(v: &str) -> Self {
        Self::Text(v.to_string())
    }
}

impl From<Vec<u8>> for DbValue {
    fn from(v: Vec<u8>) -> Self {
        Self::Blob(v)
    }
}

impl From<&[u8]> for DbValue {
    fn from(v: &[u8]) -> Self {
        Self::Blob(v.to_vec())
    }
}

impl std::fmt::Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => write!(f, "NULL"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v}"),
            Self::Blob(v) => write!(f, "<blob {} bytes>", v.len()),
        }
    }
}

/// A structured filter expression for database queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbFilter {
    /// Matches all rows.
    Always,
    /// Logical AND of multiple filters.
    And(Vec<Self>),
    /// Logical OR of multiple filters.
    Or(Vec<Self>),
    /// Logical NOT of a filter.
    Not(Box<Self>),
    /// Column equals value.
    Eq {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column not equals value.
    Ne {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column less than value.
    Lt {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column less than or equal to value.
    Le {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column greater than value.
    Gt {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column greater than or equal to value.
    Ge {
        /// Column name.
        column: String,
        /// Value to compare.
        value: DbValue,
    },
    /// Column value is in the given list.
    In {
        /// Column name.
        column: String,
        /// Values to match against.
        values: Vec<DbValue>,
    },
    /// Column matches a LIKE pattern.
    Like {
        /// Column name.
        column: String,
        /// LIKE pattern string.
        pattern: String,
    },
    /// Column is NULL.
    IsNull {
        /// Column name.
        column: String,
    },
    /// Column is not NULL.
    IsNotNull {
        /// Column name.
        column: String,
    },
}

impl DbFilter {
    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn eq(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Eq {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn ne(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Ne {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn lt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Lt {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn le(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Le {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn gt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Gt {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn ge(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Ge {
            column: column.into(),
            value: value.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn is_null(column: impl Into<String>) -> Self {
        Self::IsNull {
            column: column.into(),
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn is_not_null(column: impl Into<String>) -> Self {
        Self::IsNotNull {
            column: column.into(),
        }
    }

    /// Creates an `IN` filter matching `column` against the given values.
    pub fn in_list(
        column: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<DbValue>>,
    ) -> Self {
        Self::In {
            column: column.into(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }

    /// Creates a `LIKE` filter matching `column` against the given pattern.
    pub fn like(column: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::Like {
            column: column.into(),
            pattern: pattern.into(),
        }
    }

    /// Combines two filters with AND, flattening nested ANDs.
    ///
    /// `And(vec![])` is semantically equivalent to [`Always`](Self::Always)
    /// (SQL `TRUE`): zero predicates are trivially satisfied.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::And(mut a), Self::And(b)) => {
                a.extend(b);
                Self::And(a)
            }
            (Self::And(mut a), b) => {
                a.push(b);
                Self::And(a)
            }
            (a, Self::And(mut b)) => {
                b.insert(0, a);
                Self::And(b)
            }
            (a, b) => Self::And(vec![a, b]),
        }
    }

    /// Combines two filters with OR, flattening nested ORs.
    ///
    /// `Or(vec![])` is semantically equivalent to SQL `FALSE`: zero
    /// predicates are never satisfied.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Or(mut a), Self::Or(b)) => {
                a.extend(b);
                Self::Or(a)
            }
            (Self::Or(mut a), b) => {
                a.push(b);
                Self::Or(a)
            }
            (a, Self::Or(mut b)) => {
                b.insert(0, a);
                Self::Or(b)
            }
            (a, b) => Self::Or(vec![a, b]),
        }
    }

    /// Returns the set of column names referenced by this filter.
    ///
    /// Duplicates are removed and names are returned in sorted order.
    pub fn columns_referenced(&self) -> BTreeSet<&str> {
        let mut cols = BTreeSet::new();
        self.collect_columns(&mut cols);
        cols
    }

    fn collect_columns<'a>(&'a self, out: &mut BTreeSet<&'a str>) {
        match self {
            Self::Always => {}
            Self::And(filters) | Self::Or(filters) => {
                for f in filters {
                    f.collect_columns(out);
                }
            }
            Self::Not(f) => f.collect_columns(out),
            Self::Eq { column, .. }
            | Self::Ne { column, .. }
            | Self::Lt { column, .. }
            | Self::Le { column, .. }
            | Self::Gt { column, .. }
            | Self::Ge { column, .. }
            | Self::In { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => {
                out.insert(column);
            }
        }
    }
}

/// Sort direction for ORDER BY clauses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DbOrderDirection {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

/// An ORDER BY clause element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbOrderBy {
    /// Column name to sort by.
    pub column: String,
    /// Sort direction.
    pub direction: DbOrderDirection,
}

impl DbOrderBy {
    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn asc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            direction: DbOrderDirection::Asc,
        }
    }

    #[expect(missing_docs, reason = "SQL-like names are self-documenting")]
    pub fn desc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            direction: DbOrderDirection::Desc,
        }
    }
}

/// `SQLite` column type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DbType {
    /// 64-bit signed integer.
    Integer,
    /// 64-bit IEEE floating point.
    Real,
    /// UTF-8 text string.
    #[default]
    Text,
    /// Binary blob.
    Blob,
    /// Boolean (stored as INTEGER 0/1).
    Boolean,
}

/// Column definition for schema declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DbColumn {
    /// Column name.
    pub name: String,
    /// Column type.
    #[serde(default)]
    pub ty: DbType,
    /// Whether the column allows NULL.
    #[serde(default)]
    pub nullable: bool,
    /// Whether this column is a primary key.
    #[serde(default)]
    pub primary_key: bool,
    /// Whether this column auto-increments.
    #[serde(default)]
    pub auto_increment: bool,
    /// Whether this column has a UNIQUE constraint.
    #[serde(default)]
    pub unique: bool,
    /// Default value expression (as SQL literal).
    #[serde(default)]
    pub default: Option<DbValue>,
}

/// Table definition for schema declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DbTable {
    /// Table name (must start with the tool's prefix).
    pub name: String,
    /// Column definitions.
    #[serde(default)]
    pub columns: Vec<DbColumn>,
}

/// Index definition for schema declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DbIndex {
    /// Index name.
    pub name: String,
    /// Table name this index belongs to.
    pub table: String,
    /// Column names included in the index.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Whether this is a unique index.
    #[serde(default)]
    pub unique: bool,
}

/// Complete schema declaration for a tool's database tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DbSchema {
    /// Tool prefix (e.g., "fs_", "utility_"). All table names must start with this.
    pub prefix: String,
    /// Table definitions.
    #[serde(default)]
    pub tables: Vec<DbTable>,
    /// Index definitions.
    #[serde(default)]
    pub indexes: Vec<DbIndex>,
}
