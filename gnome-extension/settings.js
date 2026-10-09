import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Pango from 'gi://Pango';
import St from 'gi://St';

import {runWattcost} from './data.js';
import {readTariffForm} from './format.js';

export const SETTINGS_ICON = 'emblem-system-symbolic';

/** The same timing as GNOME's quick settings menus: the height grows, then the content fades in. */
const ANIMATION_MS = 125;
const PERIODS = [['day', 'Day'], ['night', 'Night']];

/**
 * Themes paint entries and buttons in nearly the colour of the quick settings card; a slight shade
 * keeps them visible on light and dark themes alike.
 */
const CONTROL_SHADE = 'background-color: rgba(0, 0, 0, 0.18);';

/** A compact entry; aligned to the start so a wider caption above does not stretch it. */
function entry(widthEm) {
    return new St.Entry({
        can_focus: true,
        x_align: Clutter.ActorAlign.START,
        style: `width: ${widthEm}em; ${CONTROL_SHADE}`,
    });
}

function label(text, styleClass = null) {
    const widget = new St.Label({text, style_class: styleClass, y_align: Clutter.ActorAlign.CENTER});
    widget.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
    return widget;
}

/** An hour entry followed by ":00". */
function hourCell(field) {
    const cell = new St.BoxLayout({style: 'spacing: 2px;'});
    cell.add_child(field);
    cell.add_child(label(':00'));
    return cell;
}

/** A card in the quick settings style that slides open inside the menu and edits the tariff. */
export const SettingsCard = GObject.registerClass({
    Signals: {
        'saved': {},
        'open-state-changed': {param_types: [GObject.TYPE_BOOLEAN]},
    },
}, class SettingsCard extends St.Widget {
    _init(findBinary) {
        super._init({
            layout_manager: new Clutter.BinLayout(),
            x_expand: true,
            height: 0,
            visible: false,
            clip_to_allocation: true,
        });
        this._findBinary = findBinary;
        this._cancellable = null;
        this.isOpen = false;
        this._saving = false;
        this._box = new St.BoxLayout({
            style_class: 'quick-toggle-menu',
            orientation: Clutter.Orientation.VERTICAL,
            x_expand: true,
        });
        this.add_child(this._box);
        this._addHeader();
        this._addForm();
        this._addButtons();
        this.connect('destroy', () => this._cancellable?.cancel());
    }

    _addHeader() {
        const layout = new Clutter.GridLayout();
        const header = new St.Widget({style_class: 'header', layout_manager: layout});
        layout.hookup_style(header);
        const icon = new St.Icon({style_class: 'icon active', icon_name: SETTINGS_ICON});
        layout.attach(icon, 0, 0, 1, 1);
        layout.attach_next_to(label('Settings', 'title'), icon, Clutter.GridPosition.RIGHT, 1, 1);
        this._box.add_child(header);
    }

    _addForm() {
        const layout = new Clutter.GridLayout({column_spacing: 6, row_spacing: 8});
        const grid = new St.Widget({layout_manager: layout, x_expand: true});
        this._currency = entry(3);
        layout.attach(label('Currency'), 0, 0, 1, 1);
        layout.attach(this._currency, 1, 0, 3, 1);
        layout.attach(label('Hours', 'subtitle'), 1, 1, 3, 1);
        // The price column takes the spare width and keeps its content on the card's right edge,
        // in line with the Save button.
        const priceCaption = label('Per kWh', 'subtitle');
        Object.assign(priceCaption, {x_expand: true, x_align: Clutter.ActorAlign.END});
        layout.attach(priceCaption, 4, 1, 1, 1);
        this._periods = new Map(PERIODS.map(([key, title], index) => {
            const fields = {start: entry(1.6), end: entry(1.6), price: entry(3.4)};
            const row = index + 2;
            // "7:00 – 23:00" reads as a time range without extra captions.
            [label(title), hourCell(fields.start), label('–'), hourCell(fields.end)]
                .forEach((child, column) => layout.attach(child, column, row, 1, 1));
            Object.assign(fields.price, {x_expand: true, x_align: Clutter.ActorAlign.END});
            layout.attach(fields.price, 4, row, 1, 1);
            fields.price.clutter_text.connect('activate', () => this._save());
            return [key, fields];
        }));
        this._box.add_child(grid);

        // A fixed width makes long messages wrap instead of widening the whole menu.
        this._error = new St.Label({style_class: 'subtitle', style: 'width: 280px; padding-top: 8px;'});
        this._error.clutter_text.line_wrap = true;
        this._error.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
        this._error.visible = false;
        this._box.add_child(this._error);
    }

    _addButtons() {
        const buttons = new St.BoxLayout({x_align: Clutter.ActorAlign.END, style: 'spacing: 8px; padding-top: 12px;'});
        for (const [title, onClick] of [['Cancel', () => this.close()], ['Save', () => this._save()]]) {
            const button = new St.Button({label: title, style_class: 'button', style: CONTROL_SHADE, can_focus: true});
            button.connect('clicked', onClick);
            buttons.add_child(button);
        }
        this._box.add_child(buttons);
    }

    // The card takes the width the chart gives the menu instead of asking for its own, so opening
    // it never changes the menu's width.
    vfunc_get_preferred_width(_forHeight) {
        return [0, 0];
    }

    toggle() {
        if (this.isOpen)
            this.close();
        else
            this.open();
    }

    async open() {
        if (this.isOpen)
            return;
        this._setOpen(true);
        this._showError('');
        this._animateOpen();
        try {
            this._fill(JSON.parse(await this._run(['config', 'show'])));
            this._currency.grab_key_focus();
        } catch (error) {
            this._reportError(error);
        }
    }

    close() {
        if (!this.isOpen)
            return;
        this._setOpen(false);
        // A running save finishes, so a written config is never left without a refresh.
        if (!this._saving)
            this._cancellable?.cancel();
        this._stopAnimations();
        this._box.ease({
            opacity: 0,
            duration: ANIMATION_MS,
            onComplete: () => this.ease({
                height: 0,
                duration: ANIMATION_MS,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                onComplete: () => this.hide(),
            }),
        });
    }

    _setOpen(open) {
        this.isOpen = open;
        this.emit('open-state-changed', open);
    }

    // Setting a property does not stop its running transition, whose completion would still run.
    _stopAnimations() {
        this.remove_all_transitions();
        this._box.remove_all_transitions();
    }

    _animateOpen() {
        this._stopAnimations();
        this.show();
        this.height = -1;
        const [, naturalHeight] = this.get_preferred_height(-1);
        this.height = 0;
        this._box.opacity = 0;
        this.ease({
            height: naturalHeight,
            duration: ANIMATION_MS,
            mode: Clutter.AnimationMode.EASE_OUT_QUAD,
            onComplete: () => {
                this.height = -1;
                this._box.ease({opacity: 255, duration: ANIMATION_MS});
            },
        });
    }

    _fill(form) {
        this._currency.text = form.currency;
        for (const [key] of PERIODS) {
            const fields = this._periods.get(key);
            fields.start.text = String(form[key].start_hour);
            fields.end.text = String(form[key].end_hour);
            // toPrecision drops float noise from computed prices without rounding real digits.
            fields.price.text = String(Number(form[key].price.toPrecision(12)));
        }
    }

    _readForm() {
        const texts = {currency: this._currency.text};
        for (const [key] of PERIODS) {
            const fields = this._periods.get(key);
            texts[key] = {start: fields.start.text, end: fields.end.text, price: fields.price.text};
        }
        return readTariffForm(texts);
    }

    async _save() {
        if (this._saving)
            return;
        this._saving = true;
        try {
            await this._run(['config', 'set'], JSON.stringify(this._readForm()));
            this.emit('saved');
            this.close();
        } catch (error) {
            this._reportError(error);
        } finally {
            this._saving = false;
        }
    }

    _reportError(error) {
        const cancelled = error instanceof GLib.Error && error.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED);
        if (this.isOpen && !cancelled)
            this._showError(error.message);
    }

    _showError(message) {
        this._error.text = message.charAt(0).toUpperCase() + message.slice(1);
        this._error.visible = message !== '';
    }

    async _run(args, input = null) {
        const binary = this._findBinary();
        if (!binary)
            throw new Error('wattcost is not installed');
        this._cancellable?.cancel();
        this._cancellable = new Gio.Cancellable();
        return runWattcost(binary, args, this._cancellable, input);
    }
});
