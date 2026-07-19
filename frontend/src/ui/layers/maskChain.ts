export interface MaskLinkApi {
    setMaskLinkedToHost(request: { id: number; linked: boolean }): unknown;
}

export interface MaskLinkState {
    id: number;
    linkedToHost: boolean;
    editable: boolean;
}

export function toggleMaskLink(api: MaskLinkApi, mask: MaskLinkState): boolean {
    if (!mask.editable) return false;
    api.setMaskLinkedToHost({ id: mask.id, linked: !mask.linkedToHost });
    return true;
}
