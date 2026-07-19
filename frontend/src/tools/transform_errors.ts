interface TransformCapabilityRejection {
    message?: string;
    operation?: string;
    endpointName?: string;
    endpointKind?: string;
}

export function describeTransformCapabilityRejection(error: unknown): string {
    const rejection = error as TransformCapabilityRejection | undefined;
    const endpoint = rejection?.endpointName ?? rejection?.endpointKind;
    const operation = rejection?.operation ?? 'transform';
    const reason = rejection?.message ?? String(error);
    const subject = endpoint ? `${endpoint} cannot ${operation}` : reason;
    const detail = endpoint && reason !== subject ? `: ${reason}` : '';
    return `${subject}${detail}. Unlink the mask to transform it independently.`;
}
