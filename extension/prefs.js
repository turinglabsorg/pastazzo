import Adw from 'gi://Adw';
import Gdk from 'gi://Gdk';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const KEY_TOGGLE = 'toggle-pastazzo';
const DEFAULT_SHORTCUT = '<Shift><Alt>v';
const SYNC = `${GLib.get_home_dir()}/.local/bin/pastazzo-sync`;

export default class PastazzoPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();

        const page = new Adw.PreferencesPage({
            title: 'Settings',
            icon_name: 'preferences-system-symbolic',
        });
        const group = new Adw.PreferencesGroup({
            title: 'Keyboard',
        });

        const row = new Adw.ActionRow({
            title: 'Open Pastazzo',
            subtitle: 'Click the shortcut and press a new key combination.',
        });

        const shortcutButton = new Gtk.Button({
            valign: Gtk.Align.CENTER,
        });
        const resetButton = new Gtk.Button({
            icon_name: 'edit-undo-symbolic',
            tooltip_text: 'Reset shortcut',
            valign: Gtk.Align.CENTER,
        });

        const refreshLabel = () => {
            shortcutButton.set_label(shortcutLabel(settings.get_strv(KEY_TOGGLE)));
        };

        shortcutButton.connect('clicked', () => {
            captureShortcut(window, shortcutButton, settings, refreshLabel);
        });
        resetButton.connect('clicked', () => {
            settings.set_strv(KEY_TOGGLE, [DEFAULT_SHORTCUT]);
            refreshLabel();
        });

        refreshLabel();
        row.add_suffix(shortcutButton);
        row.add_suffix(resetButton);
        row.activatable_widget = shortcutButton;

        group.add(row);
        page.add(group);
        window.add(page);
        window.add(new SyncPage(window));
    }
}

// Runs pastazzo-sync and hands its output, or its error, to the callback.
function runSync(args, callback) {
    try {
        const process = Gio.Subprocess.new([SYNC, ...args],
            Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
        process.communicate_utf8_async(null, null, (_process, result) => {
            try {
                const [, stdout, stderr] = process.communicate_utf8_finish(result);
                callback(process.get_successful() ? null : (stderr || 'failed').trim(), stdout);
            } catch (error) {
                callback(error.message, '');
            }
        });
    } catch (error) {
        callback(`${SYNC} is missing: install pastazzo-sync to sync this device`, '');
    }
}

function escape(text) {
    return GLib.markup_escape_text(text || '', -1);
}

// Sync: the account, its devices, the key fingerprints and the history.
const SyncPage = GObject.registerClass(
class SyncPage extends Adw.PreferencesPage {
    _init(window) {
        super._init({title: 'Sync', icon_name: 'emblem-synchronizing-symbolic'});
        this._window = window;
        this._groups = [];
        this._refresh();
    }

    _setGroups(groups) {
        this._groups.forEach(group => this.remove(group));
        this._groups = groups;
        groups.forEach(group => this.add(group));
    }

    _refresh() {
        const loading = new Adw.PreferencesGroup({title: 'Sync', description: 'Loading…'});
        this._setGroups([loading]);
        runSync(['status', '--json'], (error, output) => {
            let status = null;
            try {
                status = JSON.parse(output);
            } catch (_error) {
                status = {logged_in: false, error: error || 'unexpected answer from pastazzo-sync'};
            }
            this._render(status);
        });
    }

    _render(status) {
        if (!status.logged_in) {
            this._setGroups([new Adw.PreferencesGroup({
                title: 'Sync',
                description: 'This device isn\'t syncing. Set it up with `pastazzo-sync join` (new account) ' +
                    'or `pastazzo-sync login` (another device of an account).',
            })]);
            return;
        }

        const account = new Adw.PreferencesGroup({title: 'Account'});
        account.add(infoRow('Username', status.username));
        account.add(infoRow('Server', status.server_url));
        account.add(infoRow('This device', status.this_device.name));

        const devices = new Adw.PreferencesGroup({
            title: 'Devices',
            description: 'Devices syncing with this account. Each one shows its key fingerprint: ' +
                'it must match what that device shows for itself.',
        });
        if (status.devices_error)
            devices.add(infoRow('Couldn\'t reach the server', status.devices_error));
        for (const device of status.devices || []) {
            const row = new Adw.ActionRow({
                title: escape(device.this ? `${device.name} (this device)` : device.name),
                subtitle: `<tt>${escape(device.fingerprint)}</tt>`,
            });
            const remove = new Gtk.Button({
                label: device.this ? 'Log Out' : 'Remove',
                valign: Gtk.Align.CENTER,
                css_classes: ['destructive-action'],
            });
            remove.connect('clicked', () => this._removeDevice(device));
            row.add_suffix(remove);
            devices.add(row);
        }

        const keys = new Adw.PreferencesGroup({
            title: 'End-to-End Encryption',
            description: 'Your clipboard is encrypted on your devices with the account key, which never ' +
                'leaves them. Its fingerprint must be the same on every device: the server only ever ' +
                'stores ciphertext it can\'t read.',
        });
        keys.add(fingerprintRow('Account key', status.account_key_fingerprint));
        keys.add(fingerprintRow('Server', status.server_fingerprint));
        keys.add(fingerprintRow('This device', status.this_device.fingerprint));

        const history = new Adw.PreferencesGroup({title: 'History'});
        history.add(this._actionRow('Clear on this device', 'Empties the clipboard history here.',
            'Clear', () => this._clear(false)));
        history.add(this._actionRow('Clear on all devices',
            'Empties the history here, on the server and on every other device as it syncs.',
            'Clear Everywhere', () => this._clear(true)));

        this._setGroups([account, devices, keys, history]);
    }

    _actionRow(title, subtitle, label, onClicked) {
        const row = new Adw.ActionRow({title, subtitle});
        const button = new Gtk.Button({label, valign: Gtk.Align.CENTER, css_classes: ['destructive-action']});
        button.connect('clicked', onClicked);
        row.add_suffix(button);
        return row;
    }

    _confirm(heading, body, action, onConfirmed) {
        const dialog = new Adw.AlertDialog({heading, body});
        dialog.add_response('cancel', 'Cancel');
        dialog.add_response('confirm', action);
        dialog.set_response_appearance('confirm', Adw.ResponseAppearance.DESTRUCTIVE);
        dialog.connect('response', (_dialog, response) => {
            if (response === 'confirm')
                onConfirmed();
        });
        dialog.present(this._window);
    }

    _report(error, success) {
        this._window.add_toast(new Adw.Toast({title: error ? `Failed: ${error}` : success}));
        this._refresh();
    }

    _removeDevice(device) {
        const body = device.this
            ? 'This device stops syncing and forgets the account. You can log in again later.'
            : `${device.name} stops syncing with this account: it can't send or receive anything any more.`;
        this._confirm(device.this ? 'Log out of sync?' : `Remove ${device.name}?`, body,
            device.this ? 'Log Out' : 'Remove',
            () => runSync(['revoke', device.id], error =>
                this._report(error, device.this ? 'Logged out' : `${device.name} removed`)));
    }

    _clear(everywhere) {
        const run = () => runSync(everywhere ? ['clear', '--everywhere'] : ['clear'], error =>
            this._report(error, everywhere ? 'History cleared on all devices' : 'History cleared'));
        if (!everywhere) {
            run();
            return;
        }
        this._confirm('Clear the history on all devices?',
            'Every device empties its clipboard history as it syncs, and the server deletes what it stores.',
            'Clear Everywhere', run);
    }
});

function infoRow(title, value) {
    const row = new Adw.ActionRow({title, subtitle: escape(value)});
    row.add_css_class('property');
    return row;
}

function fingerprintRow(title, fingerprint) {
    const row = new Adw.ActionRow({title, subtitle: `<tt>${escape(fingerprint)}</tt>`, subtitle_selectable: true});
    row.add_css_class('property');
    return row;
}

function captureShortcut(window, button, settings, onDone) {
    button.set_label('Press keys...');
    button.set_sensitive(false);

    const controller = new Gtk.EventControllerKey();
    controller.set_propagation_phase(Gtk.PropagationPhase.CAPTURE);
    controller.connect('key-pressed', (_controller, keyval, keycode, state) => {
        const mask = normalizedMask(state);

        if (keyval === Gdk.KEY_Escape) {
            stopCapture(window, controller, button, onDone);
            return Gdk.EVENT_STOP;
        }

        if (!mask && (keyval === Gdk.KEY_BackSpace || keyval === Gdk.KEY_Delete)) {
            settings.set_strv(KEY_TOGGLE, []);
            stopCapture(window, controller, button, onDone);
            return Gdk.EVENT_STOP;
        }

        if (!isBindingValid(keyval, keycode, mask) || !Gtk.accelerator_valid(keyval, mask)) {
            button.set_label('Invalid shortcut');
            return Gdk.EVENT_STOP;
        }

        settings.set_strv(KEY_TOGGLE, [Gtk.accelerator_name(keyval, mask)]);
        stopCapture(window, controller, button, onDone);
        return Gdk.EVENT_STOP;
    });

    window.add_controller(controller);
}

function stopCapture(window, controller, button, onDone) {
    window.remove_controller(controller);
    button.set_sensitive(true);
    onDone();
}

function shortcutLabel(shortcuts) {
    if (!shortcuts.length)
        return 'Disabled';

    return shortcuts
        .map(shortcut => {
            const [, keyval, mask] = Gtk.accelerator_parse(shortcut);
            return Gtk.accelerator_get_label(keyval, mask);
        })
        .filter(label => label)
        .join(' / ') || 'Disabled';
}

function normalizedMask(state) {
    let mask = state & Gtk.accelerator_get_default_mod_mask();
    mask &= ~Gdk.ModifierType.LOCK_MASK;
    return mask;
}

function isBindingValid(keyval, keycode, mask) {
    if ((mask === 0 || mask === Gdk.ModifierType.SHIFT_MASK) && keycode !== 0) {
        if (
            (keyval >= Gdk.KEY_a && keyval <= Gdk.KEY_z) ||
            (keyval >= Gdk.KEY_A && keyval <= Gdk.KEY_Z) ||
            (keyval >= Gdk.KEY_0 && keyval <= Gdk.KEY_9) ||
            keyval === Gdk.KEY_space ||
            isKeyvalForbidden(keyval)
        )
            return false;
    }

    return true;
}

function isKeyvalForbidden(keyval) {
    return [
        Gdk.KEY_Home,
        Gdk.KEY_Left,
        Gdk.KEY_Up,
        Gdk.KEY_Right,
        Gdk.KEY_Down,
        Gdk.KEY_Page_Up,
        Gdk.KEY_Page_Down,
        Gdk.KEY_End,
        Gdk.KEY_Tab,
        Gdk.KEY_KP_Enter,
        Gdk.KEY_Return,
        Gdk.KEY_Mode_switch,
    ].includes(keyval);
}
