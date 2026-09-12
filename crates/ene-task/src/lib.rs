//! Task ownership: Task identity, the owner-decided revision order, and the
//! reference pair that carries both.
//!
//! [`TaskId`] is opaque and never reused. [`TaskRevision`] is the order of
//! the Task owner's decisions about that identity; the two travel together
//! as a [`TaskRef`] boundary token and are never compared across different
//! tasks. A Task purpose change is a forward step of this revision, never a
//! change of another lifecycle's generation.
//!
//! Only this identity/revision contract lives here today. The durable
//! creation boundary is one atomic unit — current Task, initial revision,
//! initial context, and workspace association — and arrives with its own
//! slice once those premise types are fixed. This crate never depends on the
//! store, and it never imports another domain's newtype: cross-domain
//! identities arrive as owner-defined premises.

mod task;

pub use task::{TaskId, TaskRef, TaskRevision};
