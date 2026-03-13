use crate::gpu::params::ParamDef;

/// What each tool module returns from its `register()` function.
/// Contains metadata for the tool system. Follows the same auto-discovery
/// convention as `FilterRegistration` and `VeilRegistration`.
pub struct ToolRegistration {
    pub type_id: &'static str,
    pub params: &'static [ParamDef],
}
