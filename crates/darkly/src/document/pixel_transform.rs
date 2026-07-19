//! Type-owned pixel transform capabilities and direct relationship planning.

use crate::layer::{Layer, LayerId, LayerNode};

use super::{Document, Entity, FilterKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelValue {
    Transparent,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelTransformBoundsPolicy {
    AlphaContent,
    DocumentExtent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelTransformSampling {
    PremultipliedRgba,
    SingleChannel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelTransformSemantics {
    pub format: wgpu::TextureFormat,
    pub uncovered_value: PixelValue,
    pub bounds_policy: PixelTransformBoundsPolicy,
    pub sampling: PixelTransformSampling,
}

pub trait PixelTransformEntity {
    fn pixel_transform_semantics(&self) -> Option<&'static PixelTransformSemantics>;
}

impl PixelTransformEntity for Entity {
    fn pixel_transform_semantics(&self) -> Option<&'static PixelTransformSemantics> {
        match self {
            Entity::Node(LayerNode::Layer(Layer::Raster(_))) => {
                Some(&super::layer_kinds::raster::PIXEL_TRANSFORM_SEMANTICS)
            }
            Entity::Filter(filter) => match filter.kind {
                FilterKind::Mask(_) => Some(&super::filters::mask::PIXEL_TRANSFORM_SEMANTICS),
                FilterKind::Selection(_) => None,
            },
            Entity::Node(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelTransformTarget {
    pub node_id: LayerId,
    pub semantics: &'static PixelTransformSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformMembershipSnapshot {
    Independent {
        initiator_id: LayerId,
    },
    MaskHost {
        mask_id: LayerId,
        host_id: LayerId,
        linked: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PixelTransformPlan {
    pub initiator_id: LayerId,
    pub membership: TransformMembershipSnapshot,
    pub targets: Vec<PixelTransformTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PixelTransformOperation {
    DestructiveTransform,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct TransformCapabilityError {
    pub endpoint: LayerId,
    pub operation: PixelTransformOperation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransformPlanError {
    Missing(LayerId),
    NotEditable(LayerId),
    Unsupported(TransformCapabilityError),
}

impl Document {
    pub fn pixel_transform_semantics(
        &self,
        id: LayerId,
    ) -> Option<&'static PixelTransformSemantics> {
        self.entities.get(id)?.pixel_transform_semantics()
    }

    pub fn can_transform_pixels(&self, id: LayerId) -> bool {
        self.pixel_transform_semantics(id).is_some() && self.is_node_editable(id)
    }

    pub fn plan_pixel_transform(
        &self,
        initiator_id: LayerId,
    ) -> Result<PixelTransformPlan, TransformPlanError> {
        if !self.entities.contains_key(initiator_id) {
            return Err(TransformPlanError::Missing(initiator_id));
        }
        let membership = super::filters::mask::transform_membership(self, initiator_id);
        let ids = match membership {
            TransformMembershipSnapshot::Independent { .. } => vec![initiator_id],
            TransformMembershipSnapshot::MaskHost {
                mask_id,
                host_id,
                linked: true,
            } => vec![
                initiator_id,
                if initiator_id == mask_id {
                    host_id
                } else {
                    mask_id
                },
            ],
            TransformMembershipSnapshot::MaskHost { linked: false, .. } => vec![initiator_id],
        };
        let mut targets = Vec::with_capacity(ids.len());
        for node_id in ids {
            if !self.entities.contains_key(node_id) {
                return Err(TransformPlanError::Missing(node_id));
            }
            if !self.is_node_editable(node_id) {
                return Err(TransformPlanError::NotEditable(node_id));
            }
            let semantics = self.pixel_transform_semantics(node_id).ok_or_else(|| {
                TransformPlanError::Unsupported(TransformCapabilityError {
                    endpoint: node_id,
                    operation: PixelTransformOperation::DestructiveTransform,
                })
            })?;
            targets.push(PixelTransformTarget { node_id, semantics });
        }
        Ok(PixelTransformPlan {
            initiator_id,
            membership,
            targets,
        })
    }

    pub fn validate_pixel_transform_plan(&self, plan: &PixelTransformPlan) -> bool {
        self.plan_pixel_transform(plan.initiator_id)
            .is_ok_and(|current| current == *plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document_with_mask(linked: bool) -> (Document, LayerId, LayerId) {
        let mut doc = Document::new(32, 32);
        let host = doc.add_raster_layer(None);
        let mask = doc.add_mask_filter(host).unwrap();
        let FilterKind::Mask(mask_filter) = &mut doc.find_filter_mut(mask).unwrap().kind else {
            unreachable!()
        };
        mask_filter.linked_to_host = linked;
        (doc, host, mask)
    }

    #[test]
    fn linked_mask_expansion_is_symmetric_and_initiator_first() {
        let (doc, host, mask) = document_with_mask(true);
        let host_plan = doc.plan_pixel_transform(host).unwrap();
        let mask_plan = doc.plan_pixel_transform(mask).unwrap();
        assert_eq!(
            host_plan
                .targets
                .iter()
                .map(|target| target.node_id)
                .collect::<Vec<_>>(),
            [host, mask]
        );
        assert_eq!(
            mask_plan
                .targets
                .iter()
                .map(|target| target.node_id)
                .collect::<Vec<_>>(),
            [mask, host]
        );
        assert_eq!(host_plan.membership, mask_plan.membership);
    }

    #[test]
    fn unlinked_endpoints_transform_independently() {
        let (doc, host, mask) = document_with_mask(false);
        assert_eq!(doc.plan_pixel_transform(host).unwrap().targets.len(), 1);
        assert_eq!(doc.plan_pixel_transform(mask).unwrap().targets.len(), 1);
    }

    #[test]
    fn repeated_planning_has_canonical_membership() {
        let (doc, host, _) = document_with_mask(true);
        let expected = doc.plan_pixel_transform(host).unwrap().membership;
        for _ in 0..16 {
            assert_eq!(doc.plan_pixel_transform(host).unwrap().membership, expected);
        }
    }

    #[test]
    fn linked_unsupported_host_rejects_the_whole_plan_structurally() {
        let mut doc = Document::new(32, 32);
        let host = doc.add_group(None);
        let mask = doc.add_mask_filter(host).unwrap();
        assert_eq!(
            doc.plan_pixel_transform(mask),
            Err(TransformPlanError::Unsupported(TransformCapabilityError {
                endpoint: host,
                operation: PixelTransformOperation::DestructiveTransform,
            }))
        );
    }
}
