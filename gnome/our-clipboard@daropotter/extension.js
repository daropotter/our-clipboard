import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const INTERFACE = `<node><interface name="org.gnome.Shell.Extensions.OurClipboard1">
  <method name="Socket"><arg type="s" direction="out"/></method>
  <method name="Version"><arg type="u" direction="out"/></method>
  <method name="GetState"><arg type="t" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
  <method name="GetText"><arg type="s" direction="out"/><arg type="t" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
  <method name="SetText"><arg type="s" direction="in"/></method>
  <method name="ApplyText"><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
</interface></node>`;

export default class OurClipboardAdapter extends Extension {
    enable() {
        this._clipboard = St.Clipboard.get_default();
        this._selection = global.display.get_selection();
        this._revision = 0;
        this._generation = GLib.uuid_string_random();
        this._marker = '';
        this._ownSource = null;
        this._enabled = true;
        this._changed = this._selection.connect('owner-changed', (_selection, kind, owner) => {
            if (kind !== Meta.SelectionType.SELECTION_CLIPBOARD)
                return;
            this._revision++;
            if (this._applying)
                this._appliedSource = owner;
            if (owner !== this._ownSource)
                this._marker = '';
        });
        this._export = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this);
        this._export.export(Gio.DBus.session, '/org/gnome/Shell/Extensions/OurClipboard');
    }

    Version() { return 1; }
    Socket() {
        const display = GLib.getenv('WAYLAND_DISPLAY');
        if (!display)
            throw new Error('GNOME Wayland socket is unavailable.');
        return GLib.path_is_absolute(display) ? display
            : GLib.build_filenamev([GLib.getenv('XDG_RUNTIME_DIR'), display]);
    }
    GetState() { return [this._revision, this._marker, this._generation]; }

    GetTextAsync(_parameters, invocation) {
        const revision = this._revision;
        const generation = this._generation;
        this._clipboard.get_text(St.ClipboardType.CLIPBOARD, (_clipboard, text) => {
            if (!this._enabled || revision !== this._revision || generation !== this._generation) {
                invocation.return_dbus_error('org.gnome.Shell.Extensions.OurClipboard1.Changed',
                    'Clipboard changed during read.');
                return;
            }
            const value = text ?? '';
            if (new TextEncoder().encode(value).length > 10_000_000) {
                invocation.return_dbus_error('org.gnome.Shell.Extensions.OurClipboard1.TooLarge',
                    'Clipboard text exceeds the size limit.');
                return;
            }
            invocation.return_value(new GLib.Variant('(stss)', [value, revision, this._marker, generation]));
        });
    }

    SetText(text) {
        if (!text || text.includes('\0') || new TextEncoder().encode(text).length > 10_000_000)
            throw new Error('Invalid clipboard text (contents omitted).');
        this._clipboard.set_text(St.ClipboardType.CLIPBOARD, text);
        this._marker = '';
        this._ownSource = null;
    }

    ApplyText(text, marker) {
        if (!text || text.includes('\0') || new TextEncoder().encode(text).length > 10_000_000)
            throw new Error('Invalid clipboard text (contents omitted).');
        if (!/^[0-9a-f]{64}$/.test(marker))
            throw new Error('Invalid clipboard origin marker.');
        this._appliedSource = null;
        this._applying = true;
        try {
            this._clipboard.set_text(St.ClipboardType.CLIPBOARD, text);
        } finally {
            this._applying = false;
        }
        if (!this._appliedSource)
            throw new Error('Clipboard write did not confirm a new owner.');
        this._ownSource = this._appliedSource;
        this._marker = marker;
    }

    disable() {
        this._enabled = false;
        this._export?.unexport();
        if (this._changed)
            this._selection.disconnect(this._changed);
        this._export = null;
        this._changed = 0;
        this._marker = '';
        this._ownSource = null;
        this._selection = null;
        this._clipboard = null;
    }
}
