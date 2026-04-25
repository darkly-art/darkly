/**
 * Global toggle for the Settings modal. The action registry dispatches
 * `openSettings` into this; the hamburger menu also writes here.
 */
class SettingsState {
    open = $state(false);
}

export const settings = new SettingsState();
