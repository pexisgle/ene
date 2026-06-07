use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single database row represented as a map of column names to values.
pub type Row = BTreeMap<String, DbValue>;

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
    /// Returns the value as an `i64` if it is an integer.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            DbValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the value as a `&str` if it is text.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            DbValue::Text(v) => Some(v),
            _ => None,
        }
    }

    /// Returns the value as a `bool` if it is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            DbValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the value as an `f64` if it is a float.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            DbValue::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the value as a byte slice if it is a blob.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            DbValue::Blob(v) => Some(v),
            _ => None,
        }
    }
}

impl From<bool> for DbValue {
    fn from(v: bool) -> Self {
        DbValue::Bool(v)
    }
}

impl From<i32> for DbValue {
    fn from(v: i32) -> Self {
        DbValue::Int(i64::from(v))
    }
}

impl From<i64> for DbValue {
    fn from(v: i64) -> Self {
        DbValue::Int(v)
    }
}

impl From<f64> for DbValue {
    fn from(v: f64) -> Self {
        DbValue::Float(v)
    }
}

impl From<String> for DbValue {
    fn from(v: String) -> Self {
        DbValue::Text(v)
    }
}

impl From<&str> for DbValue {
    fn from(v: &str) -> Self {
        DbValue::Text(v.to_string())
    }
}

impl From<Vec<u8>> for DbValue {
    fn from(v: Vec<u8>) -> Self {
        DbValue::Blob(v)
    }
}

impl From<&[u8]> for DbValue {
    fn from(v: &[u8]) -> Self {
        DbValue::Blob(v.to_vec())
    }
}

/// A structured filter expression for database queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbFilter {
    /// Matches all rows.
    Always,
    /// Logical AND of multiple filters.
    And(Vec<DbFilter>),
    /// Logical OR of multiple filters.
    Or(Vec<DbFilter>),
    /// Logical NOT of a filter.
    Not(Box<DbFilter>),
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
    /// Creates an equality filter.
    #[must_use]
    pub fn eq(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Eq {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates a not-equal filter.
    #[must_use]
    pub fn ne(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Ne {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates a less-than filter.
    #[must_use]
    pub fn lt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Lt {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates a less-than-or-equal filter.
    #[must_use]
    pub fn le(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Le {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates a greater-than filter.
    #[must_use]
    pub fn gt(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Gt {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates a greater-than-or-equal filter.
    #[must_use]
    pub fn ge(column: impl Into<String>, value: impl Into<DbValue>) -> Self {
        DbFilter::Ge {
            column: column.into(),
            value: value.into(),
        }
    }

    /// Creates an IS NULL filter.
    #[must_use]
    pub fn is_null(column: impl Into<String>) -> Self {
        DbFilter::IsNull {
            column: column.into(),
        }
    }

    /// Creates an IS NOT NULL filter.
    #[must_use]
    pub fn is_not_null(column: impl Into<String>) -> Self {
        DbFilter::IsNotNull {
            column: column.into(),
        }
    }

    /// Combines two filters with AND, flattening nested ANDs.
    #[must_use]
    pub fn and(self, other: DbFilter) -> Self {
        match (self, other) {
            (DbFilter::And(mut a), DbFilter::And(b)) => {
                a.extend(b);
                DbFilter::And(a)
            }
            (DbFilter::And(mut a), b) => {
                a.push(b);
                DbFilter::And(a)
            }
            (a, DbFilter::And(mut b)) => {
                b.insert(0, a);
                DbFilter::And(b)
            }
            (a, b) => DbFilter::And(vec![a, b]),
        }
    }

    /// Combines two filters with OR, flattening nested ORs.
    #[must_use]
    pub fn or(self, other: DbFilter) -> Self {
        match (self, other) {
            (DbFilter::Or(mut a), DbFilter::Or(b)) => {
                a.extend(b);
                DbFilter::Or(a)
            }
            (DbFilter::Or(mut a), b) => {
                a.push(b);
                DbFilter::Or(a)
            }
            (a, DbFilter::Or(mut b)) => {
                b.insert(0, a);
                DbFilter::Or(b)
            }
            (a, b) => DbFilter::Or(vec![a, b]),
        }
    }

    /// Returns all column names referenced by this filter.
    pub fn columns_referenced(&self) -> Vec<&str> {
        let mut cols = Vec::new();
        self.collect_columns(&mut cols);
        cols
    }

    fn collect_columns<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            DbFilter::Always => {}
            DbFilter::And(filters) | DbFilter::Or(filters) => {
                for f in filters {
                    f.collect_columns(out);
                }
            }
            DbFilter::Not(f) => f.collect_columns(out),
            DbFilter::Eq { column, .. }
            | DbFilter::Ne { column, .. }
            | DbFilter::Lt { column, .. }
            | DbFilter::Le { column, .. }
            | DbFilter::Gt { column, .. }
            | DbFilter::Ge { column, .. }
            | DbFilter::In { column, .. }
            | DbFilter::Like { column, .. }
            | DbFilter::IsNull { column }
            | DbFilter::IsNotNull { column } => {
                out.push(column);
            }
        }
    }
}

/// Sort direction for ORDER BY clauses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DbOrderDirection {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

/// An ORDER BY clause element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DbOrderBy {
    /// Column name to sort by.
    pub column: String,
    /// Sort direction.
    pub direction: DbOrderDirection,
}

impl DbOrderBy {
    /// Creates an ascending ORDER BY clause.
    #[must_use]
    pub fn asc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            direction: DbOrderDirection::Asc,
        }
    }

    /// Creates a descending ORDER BY clause.
    #[must_use]
    pub fn desc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            direction: DbOrderDirection::Desc,
        }
    }
}

/// SQLite column type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
