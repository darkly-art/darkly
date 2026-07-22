export interface MaskLinkApi {
    setMaskLinkedToHost(request: { id: number; linked: boolean }): unknown;
}

export interface MaskLinkState {
    id: number;
    linkedToHost: boolean;
    editable: boolean;
}

export function requestedMaskLink(mask: MaskLinkState): boolean | null {
    return mask.editable ? !mask.linkedToHost : null;
}

export function toggleMaskLink(api: MaskLinkApi, mask: MaskLinkState): boolean {
    const linked = requestedMaskLink(mask);
    if (linked === null) return false;
    api.setMaskLinkedToHost({ id: mask.id, linked });
    return true;
}
