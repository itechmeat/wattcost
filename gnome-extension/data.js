import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

Gio._promisify(Gio.Subprocess.prototype, 'communicate_utf8_async');

/** The shell's PATH often lacks per-user install directories, so they are checked too. */
export function findBinary() {
    const home = GLib.get_home_dir();
    const candidates = [
        GLib.find_program_in_path('wattcost'),
        `${home}/.cargo/bin/wattcost`,
        `${home}/.local/bin/wattcost`,
    ];
    return candidates.find(path => path && GLib.file_test(path, GLib.FileTest.IS_EXECUTABLE)) ?? null;
}

/** Runs wattcost with `args`, optionally writing `input` to its stdin, and returns its stdout. */
export async function runWattcost(binary, args, cancellable, input = null) {
    const flags = Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE |
        (input === null ? Gio.SubprocessFlags.NONE : Gio.SubprocessFlags.STDIN_PIPE);
    const process = Gio.Subprocess.new([binary, ...args], flags);
    const cancelId = cancellable.connect(() => process.force_exit());
    try {
        const [stdout, stderr] = await process.communicate_utf8_async(input, cancellable);
        if (!process.get_successful())
            throw new Error(stderr.trim().replace(/^error: /, '') || `wattcost exited with status ${process.get_exit_status()}`);
        return stdout;
    } finally {
        cancellable.disconnect(cancelId);
    }
}

/** Runs `wattcost series` for a span and returns the parsed JSON. */
export async function loadSeries(binary, span, cancellable) {
    return JSON.parse(await runWattcost(binary, ['series', '--span', span], cancellable));
}
