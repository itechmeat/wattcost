import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {COST_COLOR, CPU_COLOR, GPU_COLOR, LineChart, POWER_COLOR} from './charts.js';
import {findBinary, loadSeries} from './data.js';
import {SETTINGS_ICON, SettingsCard} from './settings.js';
import {formatKwh, formatMoney, formatWatts, maxValue, sectionCount} from './format.js';

const REFRESH_SECONDS = 60;
const SPANS = [['day', 'Today'], ['week', 'Week'], ['month', 'Month']];

export const WattcostIndicator = GObject.registerClass(
class WattcostIndicator extends PanelMenu.Button {
    _init(extensionPath) {
        super._init(0.5, 'wattcost');
        this._span = 'day';
        this._cancellable = null;

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        box.add_child(new St.Icon({
            gicon: Gio.icon_new_for_string(`${extensionPath}/icons/wattcost-bolt-symbolic.svg`),
            style_class: 'system-status-icon',
        }));
        this._label = new St.Label({text: '…', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(this._label);
        this.add_child(box);

        this._buildMenu();
        this.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this.refresh();
            else
                this._settings.close();
        });
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_SECONDS, () => {
            this.refresh();
            return GLib.SOURCE_CONTINUE;
        });
        this.refresh();
    }

    _buildMenu() {
        // Added to the menu box directly: the buttons carry the menu-item padding themselves.
        const spans = new St.BoxLayout({style: 'spacing: 8px;'});
        this.menu.box.add_child(spans);
        this._spanIcons = new Map(SPANS.map(([span, title]) => {
            const icon = new St.Icon({style_class: 'popup-menu-ornament'});
            const content = new St.BoxLayout({style: 'spacing: 6px;'});
            content.add_child(icon);
            content.add_child(new St.Label({text: title, y_align: Clutter.ActorAlign.CENTER}));
            const button = new St.Button({child: content, style_class: 'popup-menu-item', can_focus: true});
            button.connect('clicked', () => this._selectSpan(span));
            spans.add_child(button);
            return [span, icon];
        }));
        spans.add_child(new St.Widget({x_expand: true}));
        const settingsButton = new St.Button({
            style_class: 'icon-button',
            child: new St.Icon({icon_name: SETTINGS_ICON}),
            can_focus: true,
            y_align: Clutter.ActorAlign.CENTER,
        });
        settingsButton.connect('clicked', () => this._settings.toggle());
        spans.add_child(settingsButton);

        this._settings = new SettingsCard(() => (this._binary ??= findBinary()));
        this._settings.connect('open-state-changed', (_card, open) => this._dimChart(open));
        this._settings.connect('saved', () => this.refresh());
        this.menu.box.add_child(this._settings);

        this._chartItems = [];
        const separator = new PopupMenu.PopupSeparatorMenuItem();
        this.menu.addMenuItem(separator);
        this._chartItems.push(separator);

        this._chart = this._addItem(new LineChart());
        const axis = this._addItem(new St.BoxLayout({x_expand: true}));
        this._startLabel = new St.Label({x_expand: true});
        this._endLabel = new St.Label();
        axis.add_child(this._startLabel);
        axis.add_child(this._endLabel);
        [this._costTitle] = this._addLegend(COST_COLOR);
        [this._powerTitle] = this._addLegend(POWER_COLOR);
        [this._cpuTitle, this._gpuTitle] = this._addLegend(CPU_COLOR, GPU_COLOR);
        this._updateSpanItems();
    }

    /** One legend row: a bullet in each line's colour before its text; returns the text labels. */
    _addLegend(...colors) {
        const row = this._addItem(new St.BoxLayout({x_expand: true, style: 'spacing: 12px;'}));
        return colors.map(color => {
            const entry = new St.BoxLayout();
            entry.add_child(new St.Label({text: '● ', style: `color: ${color};`}));
            const label = new St.Label();
            entry.add_child(label);
            row.add_child(entry);
            return label;
        });
    }

    /** Fades the chart while the settings card is open, as quick settings fade their grid. */
    _dimChart(dim) {
        for (const item of this._chartItems)
            item.ease({opacity: dim ? 96 : 255, duration: 125});
    }

    _addItem(child) {
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        item.add_child(child);
        this.menu.addMenuItem(item);
        this._chartItems?.push(item);
        return child;
    }

    _selectSpan(span) {
        this._span = span;
        this._updateSpanItems();
        this.refresh();
    }

    /** The radio dots of system menu items, in one row. */
    _updateSpanItems() {
        for (const [span, icon] of this._spanIcons)
            icon.icon_name = span === this._span ? 'ornament-dot-checked-symbolic' : 'ornament-dot-unchecked-symbolic';
    }

    async refresh() {
        this._cancellable?.cancel();
        const cancellable = new Gio.Cancellable();
        this._cancellable = cancellable;
        this._binary ??= findBinary();
        if (!this._binary) {
            this._showError('wattcost is not installed');
            return;
        }
        try {
            const series = await loadSeries(this._binary, this._span, cancellable);
            if (!cancellable.is_cancelled())
                this._show(series);
        } catch (error) {
            if (!cancellable.is_cancelled())
                this._showError(error.message);
        }
    }

    _show(series) {
        this._label.text = formatMoney(series.today_cost, series.currency);
        this._costTitle.text = `Spent: ${formatMoney(series.total_cost, series.currency)} · ${formatKwh(series.total_kwh)}`;
        this._powerTitle.text = `Power: average ${formatWatts(series.average_w)} · peak ${formatWatts(series.peak_w)}`;
        this._cpuTitle.text = `CPU ${formatWatts(series.cpu_average_w)}`;
        this._gpuTitle.text = `GPU ${formatWatts(series.gpu_average_w)}`;
        const values = key => series.points.map(point => point[key] ?? null);
        // Total, CPU and GPU share one watt scale, so the parts read against the whole.
        const wattsMax = maxValue([...values('average_w'), ...values('cpu_w'), ...values('gpu_w')]);
        this._chart.setLines([
            // With a flat tariff spending and total power have the same shape; a lower reach keeps both visible.
            {values: values('cost_per_hour'), color: COST_COLOR, reach: 0.6},
            {values: values('cpu_w'), color: CPU_COLOR, reach: 0.9, max: wattsMax},
            {values: values('gpu_w'), color: GPU_COLOR, reach: 0.9, max: wattsMax},
            {values: values('average_w'), color: POWER_COLOR, reach: 0.9, max: wattsMax},
        ], sectionCount(series.span, series.points.length));
        this._startLabel.text = series.start_label;
        this._endLabel.text = series.end_label;
    }

    _showError(message) {
        this._label.text = '–';
        this._costTitle.text = message;
        this._powerTitle.text = '';
        this._cpuTitle.text = '';
        this._gpuTitle.text = '';
        this._chart.setLines([], sectionCount(this._span, 0));
        this._startLabel.text = '';
        this._endLabel.text = '';
    }

    // PanelMenu.Button already connects `destroy` to this method; chaining up keeps its cleanup.
    _onDestroy() {
        this._cancellable?.cancel();
        GLib.Source.remove(this._timer);
        super._onDestroy();
    }
});

