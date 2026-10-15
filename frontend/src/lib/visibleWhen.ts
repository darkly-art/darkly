/**
 * The `visible_when` rule shared by brush-node ports (`PortDef`) and effect,
 * void and shape parameters (`ParamInfo`): `[name, allowed]` shows a control
 * only while the named sibling's integer value is in `allowed`. A UI
 * affordance only; the engine still stores and reads a hidden value.
 */

/** True when there is no rule, the named sibling is absent, or its value is
 *  allowed. `rule` is typed structurally so both generated types fit without a
 *  hand-written alias. */
export function isVisibleWhen(
    rule: readonly [string, readonly number[]] | null | undefined,
    valueOf: (name: string) => unknown,
): boolean {
    if (!rule) return true;
    const [name, allowed] = rule;
    const v = valueOf(name);
    if (v === undefined) return true;
    return allowed.includes(Number(v));
}
