import System from 'system';

import {formatKwh, formatMoney, formatWatts, parseHour, parseAmount, readSettingsForm, scale, sectionCount} from '../format.js';

let failures = 0;

/** Object.is per value, element by element for arrays, so NaN never passes for null. */
function same(actual, expected) {
    if (Array.isArray(expected))
        return Array.isArray(actual) && actual.length === expected.length && expected.every((value, i) => same(actual[i], value));
    if (expected !== null && typeof expected === 'object')
        return actual !== null && typeof actual === 'object' && Object.keys(expected).length === Object.keys(actual).length &&
            Object.keys(expected).every(key => same(actual[key], expected[key]));
    return Object.is(actual, expected);
}

function check(name, actual, expected) {
    if (!same(actual, expected)) {
        failures++;
        print(`FAIL ${name}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
}

check('money', formatMoney(41.204, 'EUR'), '41.20 EUR');
check('watts', formatWatts(151.6), '152 W');
check('missing watts', formatWatts(null), '–');
check('kwh', formatKwh(2.2), '2.20 kWh');
check('scale', scale([1, 2, null, 4], 80), [20, 40, null, 80]);
check('scale zeros', scale([0, null, 0], 80), [0, null, 0]);
check('scale empty', scale([], 80), []);
check('scale to a shared maximum', scale([10, null, 20], 80, 40), [20, null, 40]);
check('hour', parseHour(' 7 '), 7);
check('hour midnight', parseHour('0'), 0);
for (const bad of ['24', '-1', '7.5', '7:00', '', 'x'])
    check(`bad hour ${bad}`, parseHour(bad), null);
check('price with comma', parseAmount('0,31'), 0.31);
check('price with dot', parseAmount(' 0.12 '), 0.12);
for (const bad of ['', '-1', 'abc', '1,2,3'])
    check(`bad price ${bad}`, parseAmount(bad), null);
const typed = {currency: ' EUR ', day: {start: '7', end: '23', price: '0,31'}, night: {start: '23', end: '7', price: '0.12'}, base_watts: '40', monitor_watts: ' 32,5 '};
check('settings form', readSettingsForm(typed), {
    currency: 'EUR',
    day: {start_hour: 7, end_hour: 23, price: 0.31},
    night: {start_hour: 23, end_hour: 7, price: 0.12},
    base_watts: 40,
    monitor_watts: 32.5,
});
function formError(texts) {
    try {
        readSettingsForm(texts);
        return null;
    } catch (error) {
        return error.message;
    }
}
check('missing currency', formError({...typed, currency: ' '}), 'Enter a currency.');
check('bad hour', formError({...typed, night: {...typed.night, start: '24'}}), 'Night: hours must be whole numbers from 0 to 23.');
check('bad price', formError({...typed, day: {...typed.day, price: 'x'}}), 'Day: enter the price per kWh as a number.');
check('bad power', formError({...typed, monitor_watts: '-5'}), 'Monitor: enter the power in watts as a number.');
check('sections of a day', sectionCount('day', 288), 4);
check('sections of a week', sectionCount('week', 168), 7);
check('sections of a month', sectionCount('month', 9 * 24), 9);
check('sections of an empty month', sectionCount('month', 0), 1);

print(failures === 0 ? 'ok' : `${failures} failed`);
if (failures > 0)
    System.exit(1);
