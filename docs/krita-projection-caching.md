# Reference: Krita's Projection & Caching Architecture

How Krita caches composited layer results and avoids redundant work. All code references are from `krita/libs/image/`.

---

## Overview

Krita caches at three levels:

| Level | Stored in | Invalidated when |
|---|---|---|
| **Layer projection** | `KisSafeNodeProjectionStore` per layer | Layer content or effect masks change |
| **Group projection** | `KisGroupLayer::m_d->paintDevice` | Any child layer changes |
| **Image projection** | Root group's projection | Any visible layer changes |

A dirty rect propagates **upward** from the changed node to the root. Each parent group only recomposites within that rect. Sibling groups are untouched.

---

## 1. Dirty Propagation

Every layer change starts with `setDirty(rect)`:

```cpp
// kis_node.cpp:582-602

void KisNode::setDirty(const QVector<QRect> &rects)
{
    if(m_d->graphListener) {
        m_d->graphListener->requestProjectionUpdate(this, rects, KisProjectionUpdateFlag::None);
    }
}
```

The graph listener is `KisImage`, which queues an async update job. The rect describes *what region* changed — everything outside it is untouched in all caches.

---

## 2. The Merge Walker: Which Nodes Need Work?

`KisMergeWalker` traverses the layer tree starting from the dirty node and categorizes every node relative to it:

```cpp
// kis_node.h:99-106

enum PositionToFilthy {
    N_ABOVE_FILTHY      = 0x08,   // higher in stack — may depend on lower nodes
    N_FILTHY_PROJECTION = 0x20,   // projection stale but original is fine (masks)
    N_FILTHY            = 0x40,   // the node that changed
    N_BELOW_FILTHY      = 0x80    // lower in stack — already composited, skip
};
```

The walker builds a `leafStack` (bottom-to-top order) that the merger processes:

```cpp
// kis_merge_walker.cc:44-68

void KisMergeWalker::startTripImpl(KisProjectionLeafSP startLeaf, Flags flags)
{
    // The dirty node and everything above it
    visitHigherNode(startLeaf,
                    !flags.testFlag(NO_FILTHY) ? N_FILTHY : N_ABOVE_FILTHY);

    // Everything below the dirty node
    KisProjectionLeafSP prevLeaf = startLeaf->prevSibling();
    if(prevLeaf)
        visitLowerNode(prevLeaf);
}

void KisMergeWalker::visitHigherNode(KisProjectionLeafSP leaf, NodePosition pos)
{
    pos |= calculateNodePosition(leaf);
    registerChangeRect(leaf, pos);

    KisProjectionLeafSP nextLeaf = leaf->nextSibling();
    if (nextLeaf)
        visitHigherNode(nextLeaf, N_ABOVE_FILTHY);
    else if (leaf->parent())
        startTripImpl(leaf->parent(), DEFAULT);   // recurse into parent group

    registerNeedRect(leaf, pos, KisRenderPassFlag::None);
}

void KisMergeWalker::visitLowerNode(KisProjectionLeafSP leaf)
{
    NodePosition position = N_BELOW_FILTHY | calculateNodePosition(leaf);
    registerNeedRect(leaf, position, KisRenderPassFlag::None);

    KisProjectionLeafSP prevLeaf = leaf->prevSibling();
    if (prevLeaf)
        visitLowerNode(prevLeaf);
}
```

The key insight: `visitHigherNode` recurses **upward through parent groups**, so dirtying a leaf node automatically schedules recompositing of every ancestor group. But sibling subtrees that are entirely below the dirty node are `N_BELOW_FILTHY` — they provide their cached projection but don't recalculate.

---

## 3. The Async Merger: Compositing

`KisAsyncMerger::startMerge()` processes the walker's leaf stack:

```cpp
// kis_async_merger.cpp:172-270 (simplified)

void KisAsyncMerger::startMerge(KisBaseRectsWalker &walker, bool notifyClones) {
    KisMergeWalker::LeafStack &leafStack = walker.leafStack();

    while(!leafStack.isEmpty()) {
        KisMergeWalker::JobItem item = leafStack.pop();
        KisProjectionLeafSP currentLeaf = item.m_leaf;
        QRect applyRect = item.m_applyRect;

        if (!m_currentProjection) {
            setupProjection(currentLeaf, applyRect, useTempProjections);
        }

        if(item.m_position & KisMergeWalker::N_FILTHY) {
            // The changed node: recalculate original + masks → projection
            currentLeaf->accept(originalVisitor);
            currentLeaf->projectionPlane()->recalculate(applyRect, ...);
        }
        else if(item.m_position & KisMergeWalker::N_ABOVE_FILTHY) {
            // Above the dirty node — only recalculate if it depends on lower
            // nodes (e.g. adjustment layers). Otherwise just composite its
            // existing cached projection.
            if(currentLeaf->dependsOnLowerNodes()) {
                currentLeaf->accept(originalVisitor);
                currentLeaf->projectionPlane()->recalculate(applyRect, ...);
            }
        }
        // N_BELOW_FILTHY: nothing — projection already cached from last time

        // Composite this node's projection onto the running accumulator
        compositeWithProjection(currentLeaf, applyRect);

        if(item.m_position & KisMergeWalker::N_TOPMOST) {
            // Last node in this group — write result to parent's projection
            writeProjection(currentLeaf, useTempProjections, applyRect);
            resetProjection();
        }
    }
}
```

The compositing step uses `KisPainter` to blit a layer's projection onto the accumulator with the layer's blend mode and opacity:

```cpp
// kis_layer_projection_plane.cpp:56-80

void KisLayerProjectionPlane::applyImpl(KisPainter *painter, const QRect &rect, ...)
{
    KisPaintDeviceSP device = m_d->layer->projection();
    if (!device) return;

    QRect needRect = rect & device->extent();
    if(needRect.isEmpty()) return;

    painter->setCompositeOpId(m_d->layer->compositeOpId());
    painter->setOpacityU8(m_d->layer->projectionLeaf()->opacity());
    painter->bitBlt(needRect.topLeft(), device, needRect);
}
```

---

## 4. Layer Projection Cache

Each layer's projection is the result of applying its effect masks to its original data. `KisLayer::projection()` returns the cached version:

```cpp
// kis_layer.cc:826-831

KisPaintDeviceSP KisLayer::projection() const
{
    KisPaintDeviceSP originalDevice = original();

    return needProjection() || hasEffectMasks() ?
        m_d->safeProjection->getDeviceLazy(originalDevice) : originalDevice;
}
```

If the layer has no effect masks and doesn't need a projection, it returns its `original()` directly — no copy, no extra memory.

---

## 5. Group Projection: The "Oblige Child" Optimization

A group layer composites its children into its own `paintDevice`. But when a group has a single trivial child, it skips the composite entirely:

```cpp
// kis_group_layer.cc:231-268

KisPaintDeviceSP KisGroupLayer::tryObligeChild() const
{
    KisLayer *child = onlyMeaningfulChild();

    if (child) {
        KisPaintDeviceSP projection = child->projection();
        if (child->channelFlags().isEmpty() &&
                projection &&
                child->visible() &&
                (child->compositeOpId() == COMPOSITE_OVER ||
                 child->compositeOpId() == COMPOSITE_ALPHA_DARKEN ||
                 child->compositeOpId() == COMPOSITE_COPY) &&
                child->opacity() == OPACITY_OPAQUE_U8 &&
                *projection->colorSpace() == *colorSpace() &&
                !child->layerStyle()) {

            quint8 defaultOpacity =
                    m_d->paintDevice->defaultPixel().opacityU8();

            if(defaultOpacity == OPACITY_TRANSPARENT_U8) {
                return projection;   // use child's projection as group's own
            }
        }
    }

    return 0;   // can't optimize, must composite normally
}
```

Conditions: single visible child, normal/copy blend mode, full opacity, same colorspace, no layer style, transparent group background. When met, the child's projection **is** the group's projection.

The merger checks this via `lazyDestinationForSubtreeComposition()`:

```cpp
// kis_group_layer.cc:285-292

KisPaintDeviceSP KisGroupLayer::lazyDestinationForSubtreeComposition() const
{
    KisPaintDeviceSP originalDev;
    bool ownsOriginal = false;
    std::tie(originalDev, ownsOriginal) = originalImpl();

    return ownsOriginal ? originalDev : nullptr;
    //      ^^^^^^^^^^^                   ^^^^^^^
    //      normal case: composite        obliged: skip composite,
    //      children into this device     child's projection IS group's
}
```

When `nullptr` is returned, the merger sets `m_currentProjection = nullptr`, which makes `compositeWithProjection()` a no-op — the entire group composite is skipped.

---

## 6. Projection Store: Memory Pooling

`KisSafeNodeProjectionStore` avoids allocation churn by pooling projection devices:

```cpp
// KisSafeNodeProjectionStore.cpp (simplified)

KisPaintDeviceSP getDeviceLazy(KisPaintDeviceSP prototype) {
    if(!m_projection ||
       *m_projection->colorSpace() != *prototype->colorSpace()) {

        if (!m_cleanProjections.isEmpty()) {
            m_projection = m_cleanProjections.takeLast();   // reuse
            m_projection->makeCloneFromRough(prototype, prototype->extent());
        } else {
            m_projection = new KisPaintDevice(*prototype);  // allocate
        }
    }
    return m_projection;
}

bool releaseDevice() {
    if (m_projection) {
        m_dirtyProjections.append(m_projection);   // defer cleanup
        m_projection = 0;
    }
}

void recycleProjectionsInSafety() {
    // runs in exclusive job context (no concurrent access)
    Q_FOREACH (DeviceSP projection, m_dirtyProjections) {
        projection->clear();
        m_cleanProjections.append(projection);     // recycle
    }
    m_dirtyProjections.clear();
}
```

Three pools: `m_projection` (active), `m_dirtyProjections` (released, awaiting cleanup), `m_cleanProjections` (cleared, ready for reuse). Cleanup runs in an exclusive job context to avoid the ABA problem (a projection being reused while another thread still reads the old one).

---

## 7. Putting It All Together

**Painting a stroke on Layer 3 inside Group A (sibling of Group B):**

```
Root
├── Group B          ← N_ABOVE_FILTHY (no dependsOnLower → just composite cached)
│   ├── Layer 5      ← N_BELOW_FILTHY in Group B's scope (not visited at all)
│   └── Layer 4      ← N_BELOW_FILTHY in Group B's scope
├── Group A          ← parent, recalculate (recomposite children within dirty rect)
│   ├── Layer 3  ★   ← N_FILTHY (recalculate projection: original + masks)
│   ├── Layer 2      ← N_BELOW_FILTHY (provide cached projection, no recalc)
│   └── Layer 1      ← N_BELOW_FILTHY
└── Background       ← N_BELOW_FILTHY
```

What happens:
1. Layer 3 calls `setDirty(strokeRect)`
2. Walker categorizes nodes, recurses up through Group A to Root
3. Merger processes bottom-to-top within Group A:
   - Layer 1, 2: `N_BELOW_FILTHY` — composite their **cached** projections (no recalc)
   - Layer 3: `N_FILTHY` — recalculate projection, then composite
   - Group A top reached → write result to Group A's `paintDevice`
4. At Root level:
   - Background: `N_BELOW_FILTHY` — cached
   - Group A: composited in step 3, apply to root
   - Group B: `N_ABOVE_FILTHY`, doesn't depend on lower → composite its **cached** group projection (Layer 4 and 5 are never touched)
5. Root projection updated — only within `strokeRect`

**Group B's entire subtree was never visited.** Its cached group projection was composited as-is.

---

## Comparison to Our Compositor

| | Krita | Ours (current) |
|---|---|---|
| **Cache granularity** | Per-layer + per-group projections | Single composite-cache texture for all layers |
| **Cache key** | Each node owns its projection device | `cache_valid_through: Option<usize>` — "layers 0..=N are baked in" |
| **Invalidation** | Dirty rect propagates up the tree; only ancestors recomposite | Any change below cached layer → `cache_valid_through = None` → recomposite all |
| **Sibling groups** | Untouched sibling groups use cached projection | N/A (flat layer list) |
| **Memory** | Pool of `KisPaintDevice`s per node, lazy alloc, recycled | One extra GPU texture |
| **Threading** | Async merger with exclusive cleanup jobs | Single-threaded WASM render loop |
| **Best case** | O(depth-of-dirty-node) groups recomposited | O(1) if painting topmost layer |
| **Worst case** | All ancestor groups recomposite (dirty rect only) | All layers recomposite (full canvas) |

Our single-cache approach is optimal for the common case (painting on the topmost layer) and has near-zero overhead. Per-group caching becomes valuable with deep group hierarchies where changes to one group shouldn't force recompositing unrelated sibling groups.
