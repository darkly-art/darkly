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

pub use compiler::{ExecStep, ExecutionPlan, InputSlot};
pub use graph::{
    exposed_port_key, Connection, ExposedPortMeta, FindTerminalError, Graph, GraphError, NodeId,
    NodeInstance, PortDef, PortDir, PortRef, UnitType,
};
pub use layout::NodeLayout;
pub use registration::NodeRegistration;

/// Trait implemented by the wire-type enum of each domain (e.g. `BrushWireType`).
///
/// `WireKind` defines what data types can flow along wires and how
/// type-compatibility is checked at connect time.
pub trait WireKind:
    Copy + Eq + std::hash::Hash + std::fmt::Debug + serde::Serialize + for<'de> serde::Deserialize<'de>
{
    /// Returns `true` if a wire of type `from` can connect to a port
    /// expecting type `to`.  This allows implicit coercions (e.g.
    /// Int → Float) without requiring explicit conversion nodes.
    fn compatible(from: Self, to: Self) -> bool;

    /// Returns `true` if an upstream wire may drive an input of this type
    /// per-dab. Defaulted to `true` — a new wire type is wirable unless it
    /// opts out (branch/data shapes that resolve at compile time). Consumers
    /// call this; they never `matches!` on the variant (type-owned dispatch).
    fn is_wirable(self) -> bool {
        true
    }

    /// Returns `true` if a user may *expose* an input of this type as a
    /// brush-bar control (a scrub, toggle, dropdown, …). Orthogonal to
    /// [`is_wirable`](Self::is_wirable): a value can be user-facing without
    /// being per-dab wirable (an enum dropdown) and vice versa. A type is
    /// exposable only when the properties panel has a widget that can
    /// render and edit it, so this is an explicit allow-list per domain —
    /// a new type is *not* exposable until its widget exists, which keeps
    /// an un-renderable control from being surfaced into a dead end.
    /// Defaulted to `true` for domains that don't distinguish; consumers
    /// call this and never `matches!` on the variant (type-owned dispatch).
    fn is_user_exposable(self) -> bool {
        true
    }
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
        /// A compile-time data shape used to exercise the wirability guard —
        /// same type on both ends (so the type check passes) but not wirable.
        Data,
    }

    impl WireKind for TestWireKind {
        fn compatible(from: Self, to: Self) -> bool {
            from == to
        }

        fn is_wirable(self) -> bool {
            !matches!(self, Self::Data)
        }

        /// `Data` is a compile-time shape with no editing widget, so it also
        /// exercises the exposability guard: same type on both ends (type
        /// check passes) but neither wirable nor user-exposable.
        fn is_user_exposable(self) -> bool {
            !matches!(self, Self::Data)
        }
    }
}
