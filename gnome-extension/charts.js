import GObject from 'gi://GObject';
import St from 'gi://St';

import {scale} from './format.js';

/** GNOME palette colours (green 3, purple 2, blue 3, orange 3), readable on light and dark menus. */
export const COST_COLOR = '#33d17a';
export const POWER_COLOR = '#c061cb';
export const CPU_COLOR = '#3584e4';
export const GPU_COLOR = '#ff7800';

function rgb(hex) {
    return [1, 3, 5].map(offset => parseInt(hex.slice(offset, offset + 2), 16) / 255);
}

/** Lines that share the time axis, each scaled to `max` (default: its own maximum) within `reach` of the height. */
export const LineChart = GObject.registerClass(
class LineChart extends St.DrawingArea {
    _init() {
        super._init({style: 'width: 320px; height: 96px;', x_expand: true});
        this._lines = [];
        this._sections = 4;
        this.connect('repaint', () => this._repaint());
    }

    /** `lines` is a list of `{values, color, reach, max}`; null values leave gaps. */
    setLines(lines, sections) {
        this._lines = lines;
        this._sections = sections;
        this.queue_repaint();
    }

    _repaint() {
        const cr = this.get_context();
        const [width, height] = this.get_surface_size();
        try {
            const color = this.get_theme_node().get_foreground_color();
            cr.setSourceRGBA(color.red / 255, color.green / 255, color.blue / 255, (color.alpha / 255) * 0.2);
            drawGrid(cr, width, height, this._sections);
            cr.setLineWidth(1.5);
            for (const {values, color, reach, max} of this._lines) {
                cr.setSourceRGB(...rgb(color));
                drawLine(cr, values, width, height, reach, max);
            }
        } finally {
            cr.$dispose();
        }
    }
});

/** A frame with horizontal quarter lines and one vertical line per section. */
function drawGrid(cr, width, height, sections) {
    cr.setLineWidth(1);
    cr.rectangle(0.5, 0.5, width - 1, height - 1);
    for (let quarter = 1; quarter < 4; quarter++) {
        const y = Math.round((height * quarter) / 4) + 0.5;
        cr.moveTo(0, y);
        cr.lineTo(width, y);
    }
    for (let section = 1; section < sections; section++) {
        const x = Math.round((width * section) / sections) + 0.5;
        cr.moveTo(x, 0);
        cr.lineTo(x, height);
    }
    cr.stroke();
}

function drawLine(cr, values, width, height, reach, max) {
    if (values.length < 2)
        return;
    const step = width / (values.length - 1);
    const points = scale(values, (height - 2) * reach, max)
        .map((pointHeight, index) => (pointHeight === null ? null : [index * step, height - 1 - pointHeight]));
    points.forEach((point, index) => {
        if (!point)
            return;
        if (points[index - 1])
            cr.lineTo(...point);
        else
            cr.moveTo(...point);
    });
    cr.stroke();
    // A point without neighbours draws no segment, so it gets a dot.
    points.forEach((point, index) => {
        if (point && !points[index - 1] && !points[index + 1])
            cr.rectangle(point[0] - 1, point[1] - 1, 2, 2);
    });
    cr.fill();
}
