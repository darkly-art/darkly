/**
 * Complete `RowNode` fixtures for the layer-row component tests.
 *
 * `RowNode` is derived from the generated `LayerInfo`
 * (`Exclude<LayerInfo, { type: 'divider' }>`), which means every capability
 * flag is required rather than optional the way the old hand-written prop type
 * had them. That is the point (a new flag reaches the row automatically), but
 * it makes a partial object literal a type error, and four test files were
 * each writing their own partial one.
 *
 * So the defaults live here, once, and a test overrides only the fields its
 * assertion is actually about. When `LayerInfo` grows a field, this file is the
 * only place that has to learn it.
 */
import type { LayerInfo, ModifierInfo } from '../../../engine/protocol_gen';
import type { RowNode } from '../rowNode';

export const maskModifier: ModifierInfo = {
    id: 42,
    kind: 'mask',
    name: 'Mask',
    visible: true,
    locked: false,
    linkedToHost: true,
    editable: true,
};

/** Fields every variant carries, so each builder below only adds its own. */
const common = {
    name: 'Layer',
    visible: true,
    locked: false,
    editable: true,
    canHaveMask: true,
    canRename: true,
    canBecomeSmartObject: false,
    opacity: 1,
    blendMode: 'normal',
    modifiers: [] as ModifierInfo[],
};

type Raster = Extract<LayerInfo, { type: 'raster' }>;
type Void = Extract<LayerInfo, { type: 'void' }>;
type Group = Extract<LayerInfo, { type: 'group' }>;

export function rasterNode(overrides: Partial<Raster> = {}): RowNode {
    return {
        ...common,
        type: 'raster',
        id: 1,
        name: 'Raster',
        paintable: true,
        hasThumbnail: true,
        icon: 'fa6-solid:image',
        kindName: 'Raster Layer',
        bounds: { origin: { x: 0, y: 0 }, width: 16, height: 16 },
        ...overrides,
    } as RowNode;
}

export function voidNode(overrides: Partial<Void> = {}): RowNode {
    return {
        ...common,
        type: 'void',
        id: 7,
        name: 'Void',
        paintable: false,
        hasThumbnail: false,
        icon: 'fa6-solid:wand-magic-sparkles',
        kindName: 'Void Layer',
        voidType: 'noise',
        params: [],
        ...overrides,
    } as RowNode;
}

export function groupNode(overrides: Partial<Group> = {}): RowNode {
    return {
        ...common,
        type: 'group',
        id: 2,
        name: 'Group',
        // A group owns no pixels of its own, so it is not paintable. The row's
        // Flatten offer has to check container-ness first because of this.
        paintable: false,
        hasThumbnail: false,
        icon: 'fa6-solid:folder',
        kindName: 'Group',
        collapsed: false,
        passthrough: false,
        children: [],
        ...overrides,
    } as RowNode;
}
