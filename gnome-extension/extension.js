import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

/** Written by install.sh: the directory holding the current build of the modules. */
const CURRENT_BUILD = 'current-build';

export default class WattcostExtension extends Extension {
    enable() {
        this._enabled = true;
        this._addIndicator().catch(error => console.error(`wattcost: ${error.message}`));
    }

    disable() {
        this._enabled = false;
        this._indicator?.destroy();
        this._indicator = null;
    }

    async _addIndicator() {
        const {WattcostIndicator} = await import(`${this._modulesUri()}/indicator.js`);
        if (!this._enabled || this._indicator)
            return;
        this._indicator = new WattcostIndicator(this.path);
        Main.panel.addToStatusArea(this.uuid, this._indicator);
    }

    // GNOME caches modules by URL until the session ends, so each local install puts the modules
    // into a new directory; disabling and enabling the extension then loads the new code.
    _modulesUri() {
        try {
            const [, contents] = GLib.file_get_contents(`${this.path}/${CURRENT_BUILD}`);
            return this.dir.get_child(new TextDecoder().decode(contents).trim()).get_uri();
        } catch {
            return this.dir.get_uri();
        }
    }
}
