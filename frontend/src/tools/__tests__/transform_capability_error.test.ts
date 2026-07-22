import { describe, expect, it } from 'vitest';
import { describeTransformCapabilityRejection } from '../transform_errors';

describe('transform capability rejection feedback', () => {
    it('surfaces a structured endpoint and operation actionably', () => {
        expect(describeTransformCapabilityRejection({
            endpointName: 'Group “Sketches”',
            operation: 'apply a destructive pixel transform',
            message: 'groups have no transformable pixel surface',
        })).toBe(
            'Group “Sketches” cannot apply a destructive pixel transform: groups have no transformable pixel surface. ' +
            'Unlink the mask to transform it independently.',
        );
    });

    it('preserves the existing engine error message and adds recovery guidance', () => {
        expect(describeTransformCapabilityRejection({
            kind: 'engine_error',
            message: 'Linked mask transform requires a transformable host',
        })).toBe(
            'Linked mask transform requires a transformable host. ' +
            'Unlink the mask to transform it independently.',
        );
    });
});
