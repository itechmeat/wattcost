// Pure helpers shared by the indicator and its tests; no GNOME imports here.

export function formatMoney(value, currency) {
    return `${value.toFixed(2)} ${currency}`;
}

export function formatWatts(value) {
    return value === null ? '–' : `${Math.round(value)} W`;
}

export function formatKwh(value) {
    return `${value.toFixed(2)} kWh`;
}

/** Scales values to `[0, height]` against `max` (default: the largest value); missing values stay null. */
export function scale(values, height, max = maxValue(values)) {
    return values.map(value => {
        if (value === null)
            return null;
        return max === 0 ? 0 : (value / max) * height;
    });
}

/** The largest non-null value, or 0. */
export function maxValue(values) {
    return Math.max(0, ...values.filter(value => value !== null));
}

/** Vertical grid sections: quarters of a day, the days of a week or month (hourly points). */
export function sectionCount(span, pointCount) {
    if (span === 'day')
        return 4;
    return Math.max(1, Math.round(pointCount / 24));
}

/** A whole hour 0–23 typed by the user, or null. */
export function parseHour(text) {
    const trimmed = text.trim();
    if (!/^\d{1,2}$/.test(trimmed))
        return null;
    const hour = Number(trimmed);
    return hour <= 23 ? hour : null;
}

/** A non-negative number typed with a dot or a comma, or null. */
export function parseAmount(text) {
    const trimmed = text.trim().replace(',', '.');
    if (!/^\d+(\.\d+)?$/.test(trimmed))
        return null;
    return Number(trimmed);
}

const PERIOD_TITLES = {day: 'Day', night: 'Night'};
const POWER_TITLES = {base_watts: 'System', monitor_watts: 'Monitor'};

/**
 * The settings card's texts as `wattcost config set` expects them. Throws an Error whose message
 * is shown to the user; overlaps and gaps between the periods are checked by wattcost itself.
 */
export function readSettingsForm(texts) {
    const currency = texts.currency.trim();
    if (!currency)
        throw new Error('Enter a currency.');
    const form = {currency};
    for (const [key, title] of Object.entries(PERIOD_TITLES)) {
        const period = texts[key];
        const [start, end] = [parseHour(period.start), parseHour(period.end)];
        const price = parseAmount(period.price);
        if (start === null || end === null)
            throw new Error(`${title}: hours must be whole numbers from 0 to 23.`);
        if (price === null)
            throw new Error(`${title}: enter the price per kWh as a number.`);
        form[key] = {start_hour: start, end_hour: end, price};
    }
    for (const [key, title] of Object.entries(POWER_TITLES)) {
        const watts = parseAmount(texts[key]);
        if (watts === null)
            throw new Error(`${title}: enter the power in watts as a number.`);
        form[key] = watts;
    }
    return form;
}
