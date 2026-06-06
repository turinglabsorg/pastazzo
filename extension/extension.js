import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const CLIPBOARD_TYPE = St.ClipboardType.CLIPBOARD;
const STORE = `${GLib.get_home_dir()}/.local/bin/pastazzo`;
const RESULT_LIMIT = 40;
const BAR_HEIGHT = 252;
const BAR_PADDING = 14;
const CARD_GAP = 8;
const CARD_SIZE = 160;
const TASKBAR_WIDTH = 50;
const DOUBLE_CLICK_DELAY_MS = 220;
const SCROLL_ANIMATION_MS = 160;
const IMAGE_MIMES = ['image/png', 'image/jpeg', 'image/jpg', 'image/webp', 'image/gif', 'image/bmp', 'image/tiff'];
const FEEDBACK_SOUND = '/usr/share/sounds/Yaru/stereo/message.oga';

const PastazzoPanel = GObject.registerClass(
class PastazzoPanel extends St.Widget {
    _init() {
        super._init({
            style_class: 'pastebar-overlay',
            reactive: true,
            visible: false,
            x_expand: true,
            y_expand: true,
        });

        this._selected = 0;
        this._items = [];
        this._grab = null;
        this._clickTimeoutId = 0;
        this._clickIndex = -1;
        this._scrollTarget = null;
        this._searchTimeoutId = 0;

        this._panel = new St.BoxLayout({
            vertical: true,
            style_class: 'pastebar-panel',
            reactive: true,
        });
        this.add_child(this._panel);

        this._content = new St.BoxLayout({
            vertical: false,
            style_class: 'pastebar-content',
            x_expand: true,
            y_expand: true,
        });
        this._panel.add_child(this._content);

        this._mainColumn = new St.BoxLayout({
            vertical: true,
            style_class: 'pastebar-main',
            x_expand: true,
            y_expand: true,
        });
        this._content.add_child(this._mainColumn);

        this._taskbar = new St.BoxLayout({
            vertical: true,
            style_class: 'pastebar-taskbar',
            x_align: Clutter.ActorAlign.CENTER,
            y_expand: true,
        });

        this._entry = new St.Entry({
            style_class: 'pastebar-search',
            can_focus: true,
            hint_text: 'Search clipboard',
            track_hover: true,
        });
        this._mainColumn.add_child(this._entry);

        this._scrollView = new St.ScrollView({
            style_class: 'pastebar-scroll',
            x_expand: true,
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.NEVER,
            overlay_scrollbars: true,
            enable_mouse_scrolling: true,
        });
        this._mainColumn.add_child(this._scrollView);

        this._list = new St.BoxLayout({
            vertical: false,
            style_class: 'pastebar-list',
            x_expand: true,
        });
        this._scrollView.set_child(this._list);
        this._scrollView.connect('scroll-event', (_actor, event) => this._smoothScrollShelf(event));

        const clearButtonContent = new St.BoxLayout({
            vertical: true,
            style_class: 'pastebar-tool-content',
        });
        clearButtonContent.add_child(new St.Icon({
            icon_name: 'user-trash-symbolic',
            style_class: 'pastebar-tool-icon',
            icon_size: 20,
        }));

        this._clearButton = new St.Button({
            style_class: 'pastebar-tool-button pastebar-clear-button',
            can_focus: true,
            reactive: true,
            track_hover: true,
            x_align: Clutter.ActorAlign.CENTER,
            y_align: Clutter.ActorAlign.START,
        });
        this._clearButton.set_child(clearButtonContent);
        this._clearButton.connect('clicked', () => this._clearHistory());
        this._taskbar.add_child(this._clearButton);

        this._content.add_child(this._taskbar);

        this._entry.clutter_text.connect('text-changed', () => {
            this._selected = 0;
            this._queueRefresh();
        });

        this._entry.clutter_text.connect('key-press-event', (_actor, event) => {
            const key = event.get_key_symbol();

            if (key === Clutter.KEY_Escape) {
                this.hidePanel();
                return Clutter.EVENT_STOP;
            }

            if (key === Clutter.KEY_Down) {
                this._moveSelection(1);
                return Clutter.EVENT_STOP;
            }

            if (key === Clutter.KEY_Up) {
                this._moveSelection(-1);
                return Clutter.EVENT_STOP;
            }

            if (key === Clutter.KEY_Return || key === Clutter.KEY_KP_Enter) {
                this._activateSelected();
                return Clutter.EVENT_STOP;
            }

            return Clutter.EVENT_PROPAGATE;
        });

        this.connect('button-press-event', (_actor, event) => {
            if (event.get_source() === this)
                this.hidePanel();

            return Clutter.EVENT_PROPAGATE;
        });
    }

    showPanel() {
        if (this.visible) {
            this._entry.grab_key_focus();
            return;
        }

        this.show();
        this._entry.clutter_text.set_text('');
        this._selected = 0;
        this._refresh();
        this._relayout();

        this._grab = Main.pushModal(this, {
            actionMode: Shell.ActionMode.POPUP,
        });
        this._entry.grab_key_focus();
    }

    hidePanel() {
        if (!this.visible)
            return;

        if (this._searchTimeoutId) {
            GLib.Source.remove(this._searchTimeoutId);
            this._searchTimeoutId = 0;
        }

        this._clearClickPending();

        if (this._grab) {
            Main.popModal(this._grab);
            this._grab = null;
        }

        this.hide();
    }

    toggle() {
        if (this.visible)
            this.hidePanel();
        else
            this.showPanel();
    }

    _queueRefresh() {
        if (this._searchTimeoutId)
            GLib.Source.remove(this._searchTimeoutId);

        this._searchTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 50, () => {
            this._searchTimeoutId = 0;
            this._refresh();
            return GLib.SOURCE_REMOVE;
        });
    }

    _refresh() {
        const query = this._entry.clutter_text.get_text();
        runStore(['search', query], null, output => {
            try {
                const items = JSON.parse(output || '[]');
                this._items = items.slice(0, RESULT_LIMIT);
            } catch (error) {
                logError(error, 'Pastazzo failed to parse search results');
                this._items = [];
            }

            if (this._selected >= this._items.length)
                this._selected = Math.max(0, this._items.length - 1);

            this._render();
        });
    }

    _render() {
        this._list.destroy_all_children();

        if (!this._items.length) {
            this._list.add_child(new St.Label({
                style_class: 'pastebar-empty',
                text: 'No clipboard items',
            }));
            this._relayout();
            return;
        }

        for (let index = 0; index < this._items.length; index++) {
            const item = this._items[index];
            const row = new St.Button({
                style_class: index === this._selected
                    ? 'pastebar-row pastebar-row-selected'
                    : 'pastebar-row',
                can_focus: true,
                reactive: true,
                track_hover: true,
            });
            row.set_size(CARD_SIZE, CARD_SIZE);

            const content = new St.BoxLayout({
                vertical: true,
                style_class: 'pastebar-card-content',
                x_expand: true,
                y_expand: true,
            });

            const title = new St.Label({
                style_class: 'pastebar-card-title',
                text: item.kind === 'image' ? 'Image' : 'Text',
            });

            let body;
            if (item.kind === 'image' && item.path) {
                body = createImagePreview(item.path);
            } else {
                body = new St.Label({
                    style_class: 'pastebar-row-label',
                    text: item.preview || item.text || '',
                    x_expand: true,
                    y_expand: true,
                });
                body.clutter_text.ellipsize = Pango.EllipsizeMode.END;
                body.clutter_text.line_wrap = true;
                body.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
            }

            const meta = new St.Label({
                style_class: 'pastebar-card-meta',
                text: item.kind === 'image'
                    ? (item.mime || 'image')
                    : `${(item.text || '').length} characters`,
            });

            content.add_child(title);
            content.add_child(body);
            content.add_child(meta);
            row.set_child(content);
            row.connect('clicked', () => {
                this._queueActivate(index);
            });
            this._list.add_child(row);
        }

        this._relayout();
    }

    _moveSelection(delta) {
        if (!this._items.length)
            return;

        this._selected += delta;
        if (this._selected < 0)
            this._selected = this._items.length - 1;
        else if (this._selected >= this._items.length)
            this._selected = 0;

        this._render();
    }

    _activateSelected() {
        this._activate(this._selected, false);
    }

    _queueActivate(index) {
        if (this._clickTimeoutId && this._clickIndex === index) {
            GLib.Source.remove(this._clickTimeoutId);
            this._clickTimeoutId = 0;
            this._clickIndex = -1;
            this._selected = index;
            this._activate(index, true);
            return;
        }

        this._clearClickPending();
        this._clickIndex = index;
        this._selected = index;
        this._clickTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DOUBLE_CLICK_DELAY_MS, () => {
            const pendingIndex = this._clickIndex;
            this._clickTimeoutId = 0;
            this._clickIndex = -1;
            this._activate(pendingIndex, false);
            return GLib.SOURCE_REMOVE;
        });
    }

    _clearClickPending() {
        if (!this._clickTimeoutId)
            return;

        GLib.Source.remove(this._clickTimeoutId);
        this._clickTimeoutId = 0;
        this._clickIndex = -1;
    }

    _activate(index, paste) {
        const item = this._items[index];
        if (!item)
            return;

        this._copyItem(item, () => {
            playFeedback();
            this.hidePanel();

            if (paste) {
                GLib.timeout_add(GLib.PRIORITY_DEFAULT, 80, () => {
                    pasteToFocusedApp();
                    return GLib.SOURCE_REMOVE;
                });
            }
        });
    }

    _copyItem(item, callback) {
        const clipboard = St.Clipboard.get_default();

        if (item.kind === 'image' && item.path && item.mime) {
            try {
                const file = Gio.File.new_for_path(item.path);
                file.load_bytes_async(null, (_file, result) => {
                    try {
                        const [bytes] = file.load_bytes_finish(result);
                        clipboard.set_content(CLIPBOARD_TYPE, item.mime, bytes);
                        runStore(['touch', item.id], null, () => callback());
                    } catch (error) {
                        logError(error, 'Pastazzo failed to copy image');
                        callback();
                    }
                });
            } catch (error) {
                logError(error, 'Pastazzo failed to load image');
                callback();
            }
            return;
        }

        clipboard.set_text(CLIPBOARD_TYPE, item.text || '');
        runStore(['touch', item.id], null, () => callback());
    }

    _clearHistory() {
        this._clearClickPending();
        runStore(['clear'], null, () => {
            this._items = [];
            this._selected = 0;
            this._entry.clutter_text.set_text('');
            playFeedback();
            this._render();
        });
    }

    _smoothScrollShelf(event) {
        if (event.is_pointer_emulated?.())
            return Clutter.EVENT_STOP;

        const adjustment = this._scrollView.hadjustment ??
            this._scrollView.get_hadjustment?.() ??
            this._scrollView.get_hscroll_bar().get_adjustment();
        if (!adjustment)
            return Clutter.EVENT_PROPAGATE;

        let delta = 0;
        switch (event.get_scroll_direction()) {
        case Clutter.ScrollDirection.UP:
        case Clutter.ScrollDirection.LEFT:
            delta = -1;
            break;
        case Clutter.ScrollDirection.DOWN:
        case Clutter.ScrollDirection.RIGHT:
            delta = 1;
            break;
        case Clutter.ScrollDirection.SMOOTH: {
            const [dx, dy] = event.get_scroll_delta();
            delta = Math.abs(dx) > Math.abs(dy) ? dx : dy;
            break;
        }
        default:
            return Clutter.EVENT_PROPAGATE;
        }

        if (!delta)
            return Clutter.EVENT_STOP;

        const [value, lower, upper, stepIncrement, _pageIncrement, pageSize] = adjustment.get_values();
        const max = Math.max(lower, upper - pageSize);
        const increment = Math.max(stepIncrement || 0, CARD_SIZE * 0.75);
        const base = this._scrollTarget ?? value;
        const target = Math.max(lower, Math.min(max, base + delta * increment));

        this._scrollTarget = target;
        adjustment.ease(target, {
            progress_mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            duration: SCROLL_ANIMATION_MS,
            onComplete: () => {
                if (this._scrollTarget === target)
                    this._scrollTarget = null;
            },
        });

        return Clutter.EVENT_STOP;
    }

    _relayout() {
        const monitor = Main.layoutManager.primaryMonitor;
        if (!monitor)
            return;

        this.set_position(monitor.x, monitor.y);
        this.set_size(monitor.width, monitor.height);

        const width = monitor.width;
        const height = BAR_HEIGHT;
        this._panel.set_size(width, height);
        this._panel.set_position(
            monitor.x,
            Math.floor(monitor.y + monitor.height - height)
        );

        const contentWidth = width - BAR_PADDING * 2;
        const contentHeight = height - BAR_PADDING * 2;
        const mainWidth = contentWidth - TASKBAR_WIDTH - CARD_GAP;

        this._content.set_size(contentWidth, contentHeight);
        this._mainColumn.set_size(mainWidth, contentHeight);
        this._taskbar.set_size(TASKBAR_WIDTH, contentHeight);
        this._clearButton.set_size(38, 38);
        this._entry.set_width(mainWidth);
        this._scrollView.set_size(mainWidth, CARD_SIZE + 22);
        const listWidth = this._items.length
            ? this._items.length * CARD_SIZE + Math.max(0, this._items.length - 1) * CARD_GAP
            : CARD_SIZE;
        this._list.set_size(listWidth, CARD_SIZE);
    }

    _cardWidth() {
        return CARD_SIZE;
    }
});

export default class PastazzoExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._clipboard = St.Clipboard.get_default();
        this._lastText = null;
        this._lastImageKey = null;
        this._lastMimeKey = null;
        this._panel = new PastazzoPanel();
        Main.uiGroup.add_child(this._panel);

        this._monitorsChangedId = Main.layoutManager.connect('monitors-changed', () => {
            this._panel?._relayout();
        });

        Main.wm.addKeybinding(
            'toggle-pastazzo',
            this._settings,
            Meta.KeyBindingFlags.NONE,
            Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP,
            () => this._panel.toggle()
        );

        this._pollId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 1, () => {
            this._pollClipboard();
            return GLib.SOURCE_CONTINUE;
        });
        this._pollClipboard();
    }

    disable() {
        Main.wm.removeKeybinding('toggle-pastazzo');

        if (this._pollId) {
            GLib.Source.remove(this._pollId);
            this._pollId = 0;
        }

        if (this._monitorsChangedId) {
            Main.layoutManager.disconnect(this._monitorsChangedId);
            this._monitorsChangedId = 0;
        }

        this._panel?.hidePanel();
        this._panel?.destroy();
        this._panel = null;
        this._settings = null;
    }

    _pollClipboard() {
        if (this._pollImageClipboard())
            return;

        this._clipboard.get_text(CLIPBOARD_TYPE, (_clipboard, text) => {
            if (typeof text !== 'string')
                text = '';

            if (text.trim() && text !== this._lastText) {
                this._lastText = text;
                runStore(['add'], text, () => {});
                return;
            }

            this._pollFileClipboard();
        });
    }

    _pollImageClipboard() {
        let mimetypes = [];
        try {
            mimetypes = this._clipboard.get_mimetypes(CLIPBOARD_TYPE) || [];
        } catch (error) {
            return false;
        }

        const mimeKey = mimetypes.join(',');
        if (mimeKey && mimeKey !== this._lastMimeKey) {
            this._lastMimeKey = mimeKey;
            log(`Pastazzo clipboard MIME types: ${mimeKey}`);
        }

        const mime = IMAGE_MIMES.find(type => mimetypes.includes(type));
        if (!mime)
            return false;

        this._clipboard.get_content(CLIPBOARD_TYPE, mime, (_clipboard, bytes) => {
            if (!bytes)
                return;

            const size = bytes.get_size?.() ?? 0;
            const key = `${mime}:${size}`;
            if (!size || key === this._lastImageKey)
                return;

            this._lastImageKey = key;
            runStoreBytes(['add-image', mime], bytes, () => {
                log(`Pastazzo saved image clipboard item: ${mime}, ${size} bytes`);
            });
        });
        return true;
    }

    _pollFileClipboard() {
        let mimetypes = [];
        try {
            mimetypes = this._clipboard.get_mimetypes(CLIPBOARD_TYPE) || [];
        } catch (error) {
            return;
        }

        const mime = mimetypes.includes('text/uri-list')
            ? 'text/uri-list'
            : mimetypes.includes('x-special/gnome-copied-files')
                ? 'x-special/gnome-copied-files'
                : null;
        if (!mime)
            return;

        this._clipboard.get_content(CLIPBOARD_TYPE, mime, (_clipboard, bytes) => {
            if (!bytes)
                return;

            try {
                const decoder = new TextDecoder();
                const text = decoder.decode(bytes.get_data());
                const uri = text
                    .split(/\r?\n/)
                    .map(line => line.trim())
                    .find(line => line.startsWith('file://'));
                if (!uri)
                    return;

                const file = Gio.File.new_for_uri(uri);
                const path = file.get_path();
                if (!path)
                    return;

                const guessed = Gio.content_type_guess(path, null);
                const contentType = guessed?.[0] ?? '';
                const imageMime = contentType.startsWith('image/')
                    ? contentType
                    : imageMimeFromPath(path);
                if (!imageMime)
                    return;

                file.load_bytes_async(null, (_file, result) => {
                    try {
                        const [imageBytes] = file.load_bytes_finish(result);
                        const size = imageBytes.get_size?.() ?? 0;
                        const key = `${imageMime}:${path}:${size}`;
                        if (!size || key === this._lastImageKey)
                            return;

                        this._lastImageKey = key;
                        runStoreBytes(['add-image', imageMime], imageBytes, () => {
                            log(`Pastazzo saved image file clipboard item: ${imageMime}, ${size} bytes, ${path}`);
                        });
                    } catch (error) {
                        logError(error, 'Pastazzo failed to read image file clipboard item');
                    }
                });
            } catch (error) {
                logError(error, 'Pastazzo failed to parse file clipboard item');
            }
        });
    }
}

function runStore(args, input, callback) {
    try {
        const launcher = new Gio.SubprocessLauncher({
            flags: Gio.SubprocessFlags.STDIN_PIPE |
                Gio.SubprocessFlags.STDOUT_PIPE |
                Gio.SubprocessFlags.STDERR_PIPE,
        });
        const proc = launcher.spawnv([STORE, ...args]);

        proc.communicate_utf8_async(input, null, (_proc, result) => {
            try {
                const [, stdout, stderr] = proc.communicate_utf8_finish(result);
                if (stderr)
                    log(`Pastazzo store stderr: ${stderr.trim()}`);
                callback(stdout || '');
            } catch (error) {
                logError(error, 'Pastazzo store failed');
                callback('');
            }
        });
    } catch (error) {
        logError(error, 'Pastazzo failed to start store');
        callback('');
    }
}

function createImagePreview(path) {
    const frame = new St.Bin({
        style_class: 'pastebar-image-frame',
        x_expand: true,
        y_expand: true,
    });

    try {
        const file = Gio.File.new_for_path(path);
        const texture = St.TextureCache.get_default().load_file_async(
            file,
            CARD_SIZE - 22,
            CARD_SIZE - 56,
            1,
            St.ThemeContext.get_for_stage(global.stage).scaleFactor
        );
        texture.set_size(CARD_SIZE - 22, CARD_SIZE - 56);
        frame.set_child(texture);
    } catch (error) {
        logError(error, 'Pastazzo failed to render image preview');
        frame.set_child(new St.Label({
            style_class: 'pastebar-row-label',
            text: 'Image',
        }));
    }

    return frame;
}

function runStoreBytes(args, bytes, callback) {
    try {
        const launcher = new Gio.SubprocessLauncher({
            flags: Gio.SubprocessFlags.STDIN_PIPE |
                Gio.SubprocessFlags.STDOUT_PIPE |
                Gio.SubprocessFlags.STDERR_PIPE,
        });
        const proc = launcher.spawnv([STORE, ...args]);

        proc.communicate_async(bytes, null, (_proc, result) => {
            try {
                const [, _stdout, stderr] = proc.communicate_finish(result);
                if (stderr && stderr.get_size() > 0) {
                    const decoder = new TextDecoder();
                    log(`Pastazzo store stderr: ${decoder.decode(stderr.get_data()).trim()}`);
                }
                callback('');
            } catch (error) {
                logError(error, 'Pastazzo binary store failed');
                callback('');
            }
        });
    } catch (error) {
        logError(error, 'Pastazzo failed to start binary store');
        callback('');
    }
}

function pasteToFocusedApp() {
    try {
        const backend = Clutter.get_default_backend();
        const seat = backend.get_default_seat();
        const keyboard = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
        const time = global.get_current_time?.() ?? Clutter.CURRENT_TIME;

        keyboard.notify_keyval(time, Clutter.KEY_Control_L, Clutter.KeyState.PRESSED);
        keyboard.notify_keyval(time, Clutter.KEY_v, Clutter.KeyState.PRESSED);
        keyboard.notify_keyval(time, Clutter.KEY_v, Clutter.KeyState.RELEASED);
        keyboard.notify_keyval(time, Clutter.KEY_Control_L, Clutter.KeyState.RELEASED);
    } catch (error) {
        logError(error, 'Pastazzo failed to paste with virtual keyboard');
    }
}

function playFeedback() {
    try {
        const launcher = new Gio.SubprocessLauncher({
            flags: Gio.SubprocessFlags.NONE,
        });
        launcher.spawnv(['/usr/bin/canberra-gtk-play', '-f', FEEDBACK_SOUND, '-d', 'Pastazzo']);
    } catch (error) {
        try {
            const player = new Meta.SoundPlayer();
            player.play_from_theme('message', 'Pastazzo copied item', null);
        } catch (_fallbackError) {
            logError(error, 'Pastazzo failed to play feedback sound');
        }
    }
}

function cssEscape(value) {
    return `${value}`.replaceAll('\\', '\\\\').replaceAll('"', '\\"');
}

function imageMimeFromPath(path) {
    const lowered = path.toLowerCase();
    if (lowered.endsWith('.png'))
        return 'image/png';
    if (lowered.endsWith('.jpg') || lowered.endsWith('.jpeg'))
        return 'image/jpeg';
    if (lowered.endsWith('.webp'))
        return 'image/webp';
    if (lowered.endsWith('.gif'))
        return 'image/gif';
    if (lowered.endsWith('.bmp'))
        return 'image/bmp';
    if (lowered.endsWith('.tif') || lowered.endsWith('.tiff'))
        return 'image/tiff';
    return null;
}
