use crate::document::layer_kind::LayerKindRegistration;

pub const TYPE_ID: &str = "group";

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Group",
    }
}
