import type { ParamInfo } from '../paramSchema';

/** A typed `ParamInfo` fixture: fills the fields every `ParamInfo` carries but
 *  a case doesn't exercise, so each fixture states only what it is testing.
 *  Keeping fixtures typed as the generated `ParamInfo` is the point: the tests
 *  using it are the guard that the generated type still satisfies every panel
 *  helper. */
export function param(p: Partial<ParamInfo> & Pick<ParamInfo, 'kind' | 'name' | 'default'>): ParamInfo {
    return {
        label: null,
        description: null,
        widget: 'auto',
        unit: 'Raw',
        min: null,
        max: null,
        value: null,
        options: null,
        display: { min: null, max: null, default: null, unit: '' },
        visible_when: null,
        ...p,
    };
}
