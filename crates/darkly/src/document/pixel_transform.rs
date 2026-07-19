//! Type-owned pixel transform capabilities and relationship planning.
//!
//! Transform consumers operate on immutable descriptors and a fixed relationship
//! snapshot. Entity kinds and relationship registrations own all variant-specific
//! knowledge; the engine never classifies layer/filter ids itself.

use std::collections::{HashSet, VecDeque};

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

pub static RASTER_PIXEL_TRANSFORM_SEMANTICS: PixelTransformSemantics = PixelTransformSemantics {
    format: wgpu::TextureFormat::Rgba8Unorm,
    uncovered_value: PixelValue::Transparent,
    bounds_policy: PixelTransformBoundsPolicy::AlphaContent,
    sampling: PixelTransformSampling::PremultipliedRgba,
};

pub static REVEAL_MASK_PIXEL_TRANSFORM_SEMANTICS: PixelTransformSemantics =
    PixelTransformSemantics {
        format: wgpu::TextureFormat::R8Unorm,
        uncovered_value: PixelValue::White,
        bounds_policy: PixelTransformBoundsPolicy::DocumentExtent,
        sampling: PixelTransformSampling::SingleChannel,
    };

pub trait PixelTransformEntity {
    fn pixel_transform_semantics(&self) -> Option<&'static PixelTransformSemantics>;
}

impl PixelTransformEntity for Entity {
    fn pixel_transform_semantics(&self) -> Option<&'static PixelTransformSemantics> {
        match self {
            Entity::Node(LayerNode::Layer(Layer::Raster(_))) => {
                Some(&RASTER_PIXEL_TRANSFORM_SEMANTICS)
            }
            Entity::Filter(filter) => match filter.kind {
                FilterKind::Mask(_) => Some(&REVEAL_MASK_PIXEL_TRANSFORM_SEMANTICS),
                FilterKind::Selection(_) => None,
            },
            Entity::Node(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransformRelationshipFingerprint {
    pub registration: &'static str,
    pub owner: LayerId,
    pub endpoint_a: LayerId,
    pub endpoint_b: LayerId,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformRelationshipSnapshot {
    pub targets: Vec<LayerId>,
    pub relationships: Vec<TransformRelationshipFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransformPlanError {
    Missing(LayerId),
    NotEditable(LayerId),
    Unsupported(LayerId),
}

pub struct TransformRelationshipRegistration {
    pub type_id: &'static str,
    pub enumerate: fn(&Document) -> Vec<TransformRelationshipFingerprint>,
}

fn mask_relationships(doc: &Document) -> Vec<TransformRelationshipFingerprint> {
    let mut relationships = Vec::new();
    for (id, entity) in &doc.entities {
        let Entity::Filter(filter) = entity else {
            continue;
        };
        let FilterKind::Mask(mask) = &filter.kind else {
            continue;
        };
        let Some(host) = doc.parent_of(id) else {
            continue;
        };
        relationships.push(TransformRelationshipFingerprint {
            registration: "mask-host",
            owner: id,
            endpoint_a: host,
            endpoint_b: id,
            enabled: mask.linked_to_host,
        });
    }
    relationships.sort_by_key(|relationship| {
        (
            relationship.endpoint_a.to_ffi(),
            relationship.endpoint_b.to_ffi(),
        )
    });
    relationships
}

pub static MASK_TRANSFORM_RELATIONSHIP: TransformRelationshipRegistration =
    TransformRelationshipRegistration {
        type_id: "mask-host",
        enumerate: mask_relationships,
    };

static TRANSFORM_RELATIONSHIPS: [&TransformRelationshipRegistration; 1] =
    [&MASK_TRANSFORM_RELATIONSHIP];

fn registrations() -> &'static [&'static TransformRelationshipRegistration] {
    &TRANSFORM_RELATIONSHIPS
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
        initiator: LayerId,
    ) -> Result<TransformRelationshipSnapshot, TransformPlanError> {
        self.plan_pixel_transform_with(initiator, registrations())
    }

    fn plan_pixel_transform_with(
        &self,
        initiator: LayerId,
        registrations: &[&TransformRelationshipRegistration],
    ) -> Result<TransformRelationshipSnapshot, TransformPlanError> {
        if !self.entities.contains_key(initiator) {
            return Err(TransformPlanError::Missing(initiator));
        }

        let relationships: Vec<_> = registrations
            .iter()
            .flat_map(|registration| (registration.enumerate)(self))
            .collect();
        let mut targets = vec![initiator];
        let mut seen = HashSet::from([initiator]);
        let mut queue = VecDeque::from([initiator]);
        let mut visited_relationships = HashSet::new();

        while let Some(entity) = queue.pop_front() {
            for (index, relationship) in relationships.iter().enumerate() {
                if !relationship.enabled
                    || visited_relationships.contains(&index)
                    || (relationship.endpoint_a != entity && relationship.endpoint_b != entity)
                {
                    continue;
                }
                visited_relationships.insert(index);
                let other = if relationship.endpoint_a == entity {
                    relationship.endpoint_b
                } else {
                    relationship.endpoint_a
                };
                if seen.insert(other) {
                    targets.push(other);
                    queue.push_back(other);
                }
            }
        }

        for &id in &targets {
            if !self.entities.contains_key(id) {
                return Err(TransformPlanError::Missing(id));
            }
            if !self.is_node_editable(id) {
                return Err(TransformPlanError::NotEditable(id));
            }
            if self.pixel_transform_semantics(id).is_none() {
                return Err(TransformPlanError::Unsupported(id));
            }
        }

        let relationships = visited_relationships
            .into_iter()
            .map(|index| relationships[index])
            .collect();

        Ok(TransformRelationshipSnapshot {
            targets,
            relationships,
        })
    }

    pub fn validate_pixel_transform_snapshot(
        &self,
        initiator: LayerId,
        snapshot: &TransformRelationshipSnapshot,
    ) -> bool {
        self.plan_pixel_transform(initiator)
            .is_ok_and(|current| current == *snapshot)
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
        assert_eq!(
            doc.plan_pixel_transform(host).unwrap().targets,
            [host, mask]
        );
        assert_eq!(
            doc.plan_pixel_transform(mask).unwrap().targets,
            [mask, host]
        );
    }

    #[test]
    fn unlinked_endpoints_transform_independently() {
        let (doc, host, mask) = document_with_mask(false);
        assert_eq!(doc.plan_pixel_transform(host).unwrap().targets, [host]);
        assert_eq!(doc.plan_pixel_transform(mask).unwrap().targets, [mask]);
    }

    #[test]
    fn linked_unsupported_host_rejects_the_whole_plan() {
        let mut doc = Document::new(32, 32);
        let host = doc.add_group(None);
        let mask = doc.add_mask_filter(host).unwrap();
        assert_eq!(
            doc.plan_pixel_transform(mask),
            Err(TransformPlanError::Unsupported(host))
        );
    }
}
