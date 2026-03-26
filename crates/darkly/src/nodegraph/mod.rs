//! Domain-agnostic node graph infrastructure.
//!
//! Provides a generic graph data structure, topological compiler, and
//! registration types parameterised by a `WireKind` trait.  No GPU,
//! no brush concepts — this is pure data plumbing, fully testable with
//! `cargo test`.

mod compiler;
mod graph;
mod layout;
mod registration;

pub use compiler::{ExecStep, ExecutionPlan};
pub use graph::{Connection, Graph, GraphError, NodeId, NodeInstance, PortDef, PortDir, PortRef};
pub use registration::NodeRegistration;

/// Trait implemented by the wire-type enum of each domain (e.g. `BrushWireType`).
///
/// `WireKind` defines what data types can flow along wires and how
/// type-compatibility is checked at connect time.
pub trait WireKind: Copy + Eq + std::hash::Hash + std::fmt::Debug + serde::Serialize + for<'de> serde::Deserialize<'de> {
    /// Returns `true` if a wire of type `from` can connect to a port
    /// expecting type `to`.  This allows implicit coercions (e.g.
    /// Int → Float) without requiring explicit conversion nodes.
    fn compatible(from: Self, to: Self) -> bool;
}

pub use compiler::compile;

#[cfg(test)]
pub(crate) mod tests {
    use super::WireKind;
    use serde::{Deserialize, Serialize};

    /// Minimal wire-kind enum for unit testing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub enum TestWireKind {
        Scalar,
        Color,
    }

    impl WireKind for TestWireKind {
        fn compatible(from: Self, to: Self) -> bool {
            from == to
        }
    }
}
