/**
 * Global toggle for the About modal. The hamburger menu writes here to open it.
 */
class AboutState {
    open = $state(false);
}

export const about = new AboutState();
