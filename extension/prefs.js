import Adw from 'gi://Adw';
import Gdk from 'gi://Gdk';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

const KEY_TOGGLE = 'toggle-pastazzo';
const DEFAULT_SHORTCUT = '<Shift><Alt>v';

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
    }
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
