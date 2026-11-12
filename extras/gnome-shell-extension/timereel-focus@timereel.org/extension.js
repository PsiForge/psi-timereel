// TimeReel Focus: exports org.timereel.Focus1.GetFocus() on the session bus.
//
// GetFocus() -> (ok, app_name, wm_class, title)
//   ok = false: the extension could not tell (the caller treats the app as
//                unknown);
//   ok = true and all three strings empty: no window has the focus;
//   ok = true otherwise: the focused window. app_name comes from
//                Shell.WindowTracker, wm_class from Mutter.
//
// Privacy: this file never logs. Window titles and app names must not reach
// the journal, so there is no log call anywhere in it.

import Gio from 'gi://Gio';
import Shell from 'gi://Shell';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.timereel.Focus1';
const OBJECT_PATH = '/org/timereel/Focus1';

const INTERFACE_XML = `
<node>
  <interface name="org.timereel.Focus1">
    <method name="GetFocus">
      <arg type="b" direction="out" name="ok"/>
      <arg type="s" direction="out" name="app_name"/>
      <arg type="s" direction="out" name="wm_class"/>
      <arg type="s" direction="out" name="title"/>
    </method>
  </interface>
</node>`;

export default class TimeReelFocusExtension extends Extension {
    enable() {
        this._exported = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
        this._ownerId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            connection => this._exported.export(connection, OBJECT_PATH),
            null,
            null
        );
    }

    disable() {
        if (this._ownerId) {
            Gio.bus_unown_name(this._ownerId);
            this._ownerId = 0;
        }
        if (this._exported) {
            this._exported.unexport();
            this._exported = null;
        }
    }

    GetFocus() {
        try {
            const win = global.display.focus_window;
            if (!win)
                return [true, '', '', ''];
            const app = Shell.WindowTracker.get_default().get_window_app(win);
            const appName = app ? app.get_name() ?? '' : '';
            const wmClass = win.get_wm_class() ?? '';
            if (appName === '' && wmClass === '')
                return [false, '', '', ''];
            return [true, appName, wmClass, win.get_title() ?? ''];
        } catch (_e) {
            // Deliberately not logged: the error text could hold a title.
            return [false, '', '', ''];
        }
    }
}
