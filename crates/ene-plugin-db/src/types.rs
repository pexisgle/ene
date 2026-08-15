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
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl DbValue {
    pub const fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }

    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

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
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Box<Self>),
    Eq {
        column: String,
        value: DbValue,
    },
    Ne {
        column: String,
        value: DbValue,
    },
    Lt {
        column: String,
        value: DbValue,
    },
    Le {
        column: String,
        value: DbValue,
    },
    Gt {
        column: String,
        value: DbValue,
    },
    Ge {
        column: String,
        value: DbValue,
    },
    In {
        column: String,
        values: Vec<DbValue>,
    },
    Like {
        column: String,
        pattern: String,
    },
    IsNull {
        column: String,
    },
    IsNotNull {
        column: String,
    },
}

impl DbFilter {
    pub fn eq(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Eq {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn ne(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Ne {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn lt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Lt {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn le(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Le {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn gt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Gt {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn ge(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        Self::Ge {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn is_null(column: impl Into<String>) -> Self {
        Self::IsNull {
            column: column.into(),
        }
    }

    pub fn is_not_null(column: impl Into<String>) -> Self {
        Self::IsNotNull {
            column: column.into(),
        }
    }

    pub fn in_list(
        column: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<DbValue>>,
    ) -> Self {
        Self::In {
            column: column.into(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }

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
    Asc,
    Desc,
}

/// An ORDER BY clause element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbOrderBy {
    pub column: String,
    pub direction: DbOrderDirection,
}

impl DbOrderBy {
    pub fn asc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            direction: DbOrderDirection::Asc,
        }
    }

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
    Integer,
    Real,
    #[default]
    Text,
    Blob,
    /// Stored as INTEGER 0/1.
    Boolean,
}

/// Column definition for schema declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DbColumn {
    pub name: String,
    #[serde(default)]
    pub ty: DbType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default)]
    pub auto_increment: bool,
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
    #[serde(default)]
    pub columns: Vec<DbColumn>,
}

/// Index definition for schema declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DbIndex {
    pub name: String,
    pub table: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

/// Complete schema declaration for a tool's database tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DbSchema {
    /// Tool prefix (e.g., "fs_", "utility_"). All table names must start with this.
    pub prefix: String,
    #[serde(default)]
    pub tables: Vec<DbTable>,
    #[serde(default)]
    pub indexes: Vec<DbIndex>,
}
