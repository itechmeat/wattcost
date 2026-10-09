//! Installs the GNOME Shell extension that is built into the binary, and reloads it in the
//! running session when it can.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{fs, io, thread};

use thiserror::Error;

pub const UUID: &str = "wattcost@itechmeat.github.io";

const EXTENSION_JS: &str = include_str!("../gnome-extension/extension.js");

/// Files the shell loads by a fixed URL: changing `extension.js` needs a new login session.
const LOADER: [(&str, &str); 3] = [
    (
        "metadata.json",
        include_str!("../gnome-extension/metadata.json"),
    ),
    ("extension.js", EXTENSION_JS),
    (
        "icons/wattcost-bolt-symbolic.svg",
        include_str!("../gnome-extension/icons/wattcost-bolt-symbolic.svg"),
    ),
];

/// Modules imported from a per-install directory, so a new install is loaded fresh.
const MODULES: [(&str, &str); 5] = [
    ("charts.js", include_str!("../gnome-extension/charts.js")),
    ("data.js", include_str!("../gnome-extension/data.js")),
    ("format.js", include_str!("../gnome-extension/format.js")),
    (
        "indicator.js",
        include_str!("../gnome-extension/indicator.js"),
    ),
    (
        "settings.js",
        include_str!("../gnome-extension/settings.js"),
    ),
];

/// Read by `extension.js`: the directory with the current modules.
const CURRENT_BUILD: &str = "current-build";
/// Files of the layout before modules moved into build directories.
const LEGACY_FILES: [&str; 6] = [
    "charts.js",
    "data.js",
    "format.js",
    "indicator.js",
    "settings.js",
    "stylesheet.css",
];
const STATE_POLLS: u32 = 25;
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("cannot write the extension to {path}: {error}")]
    Write { path: PathBuf, error: io::Error },
    #[error("cannot run {command}: {error}")]
    Command { command: String, error: io::Error },
    #[error("the reload did not finish; check `gnome-extensions info {UUID}`")]
    Reload,
}

/// What happened, phrased for the user by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// GNOME Shell is not installed.
    NoGnome,
    /// A first install: the shell loads it after the next login.
    NeedsLogin,
    /// `extension.js` changed: the shell keeps the old one until the next login.
    LoaderChanged,
    Reloaded,
    /// Installed, but left alone because it is not active (state as reported by the shell).
    NotActive(String),
}

pub fn install_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("gnome-shell/extensions")
        .join(UUID)
}

/// Writes the files and reloads the extension in the running session where that is possible.
pub fn install() -> Result<Outcome, ExtensionError> {
    if !command_exists("gnome-extensions") {
        return Ok(Outcome::NoGnome);
    }
    let loader_changed = write_files(&install_dir())?;
    let Some(state) = shell_state()? else {
        enable_at_next_login()?;
        return Ok(Outcome::NeedsLogin);
    };
    if loader_changed {
        return Ok(Outcome::LoaderChanged);
    }
    if state != "ACTIVE" {
        return Ok(Outcome::NotActive(state));
    }
    // The shell applies a disable asynchronously; enabling before it has done so is a no-op.
    run("gnome-extensions", &["disable", UUID])?;
    wait_for_state("INACTIVE")?;
    run("gnome-extensions", &["enable", UUID])?;
    if wait_for_state("ACTIVE")? {
        Ok(Outcome::Reloaded)
    } else {
        Err(ExtensionError::Reload)
    }
}

/// Lays the files out in `dest`; returns whether `extension.js` differs from the installed one.
pub(crate) fn write_files(dest: &Path) -> Result<bool, ExtensionError> {
    let write_error = |path: &Path| {
        let path = path.to_path_buf();
        move |error| ExtensionError::Write { path, error }
    };
    fs::create_dir_all(dest).map_err(write_error(dest))?;
    let loader_path = dest.join("extension.js");
    let loader_changed =
        fs::read_to_string(&loader_path).is_ok_and(|installed| installed != EXTENSION_JS);

    let build = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(dest)
        .map_err(write_error(dest))?
        .keep();
    fs::set_permissions(&build, fs::Permissions::from_mode(0o755)).map_err(write_error(&build))?;
    for (name, contents) in MODULES {
        fs::write(build.join(name), contents).map_err(write_error(&build.join(name)))?;
    }
    let _ = fs::remove_dir_all(dest.join("icons"));
    for (name, contents) in LOADER {
        let path = dest.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(write_error(parent))?;
        }
        fs::write(&path, contents).map_err(write_error(&path))?;
    }
    let build_name = build
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    fs::write(dest.join(CURRENT_BUILD), format!("{build_name}\n")).map_err(write_error(dest))?;

    for entry in fs::read_dir(dest).map_err(write_error(dest))?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("build-") && name != build_name {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    for legacy in LEGACY_FILES {
        let _ = fs::remove_file(dest.join(legacy));
    }
    Ok(loader_changed)
}

/// The `org.gnome.shell enabled-extensions` value with `uuid` added, or `None` if it is listed.
pub(crate) fn with_enabled(list: &str, uuid: &str) -> Option<String> {
    let list = list.trim().trim_start_matches("@as").trim();
    if list.contains(&format!("'{uuid}'")) {
        return None;
    }
    let inner = list.trim_start_matches('[').trim_end_matches(']').trim();
    Some(if inner.is_empty() {
        format!("['{uuid}']")
    } else {
        format!("[{inner}, '{uuid}']")
    })
}

/// The `State:` line of `gnome-extensions info`.
pub(crate) fn parse_state(info: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.trim().strip_prefix("State:"))
        .map(|state| state.trim().to_owned())
}

fn enable_at_next_login() -> Result<(), ExtensionError> {
    let current = output(
        "gsettings",
        &["get", "org.gnome.shell", "enabled-extensions"],
    )?;
    if let Some(list) = with_enabled(&current, UUID) {
        run(
            "gsettings",
            &["set", "org.gnome.shell", "enabled-extensions", &list],
        )?;
    }
    Ok(())
}

fn shell_state() -> Result<Option<String>, ExtensionError> {
    Ok(parse_state(&output("gnome-extensions", &["info", UUID])?))
}

/// Polls the shell for up to five seconds; returns whether `state` was reached.
fn wait_for_state(state: &str) -> Result<bool, ExtensionError> {
    for _ in 0..STATE_POLLS {
        if shell_state()?.as_deref() == Some(state) {
            return Ok(true);
        }
        thread::sleep(STATE_POLL_INTERVAL);
    }
    Ok(false)
}

fn command_exists(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

fn output(program: &str, args: &[&str]) -> Result<String, ExtensionError> {
    let output =
        Command::new(program)
            .args(args)
            .output()
            .map_err(|error| ExtensionError::Command {
                command: program.to_owned(),
                error,
            })?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run(program: &str, args: &[&str]) -> Result<(), ExtensionError> {
    output(program, args).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_build(dest: &Path) -> PathBuf {
        dest.join(fs::read_to_string(dest.join(CURRENT_BUILD)).unwrap().trim())
    }

    #[test]
    fn lays_out_the_loader_and_a_build_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(UUID);
        assert!(!write_files(&dest).unwrap());
        for (name, _) in LOADER {
            assert!(dest.join(name).is_file(), "{name}");
        }
        let build = current_build(&dest);
        for (name, _) in MODULES {
            assert!(build.join(name).is_file(), "{name}");
        }
        assert_eq!(
            fs::metadata(&build).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn a_new_install_replaces_the_previous_build_and_legacy_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(UUID);
        write_files(&dest).unwrap();
        let first = current_build(&dest);
        fs::write(dest.join("indicator.js"), "old").unwrap();
        fs::write(dest.join("icons/old-wallet-symbolic.svg"), "old").unwrap();
        write_files(&dest).unwrap();
        assert!(!first.exists());
        assert!(current_build(&dest).is_dir());
        assert!(!dest.join("indicator.js").exists());
        assert!(!dest.join("icons/old-wallet-symbolic.svg").exists());
    }

    #[test]
    fn reports_a_changed_loader() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(UUID);
        write_files(&dest).unwrap();
        assert!(!write_files(&dest).unwrap());
        fs::write(dest.join("extension.js"), "an older loader").unwrap();
        assert!(write_files(&dest).unwrap());
    }

    #[test]
    fn adds_the_uuid_to_the_enabled_list_once() {
        assert_eq!(with_enabled("@as []", "x@y").as_deref(), Some("['x@y']"));
        assert_eq!(
            with_enabled("['a@b', 'c@d']\n", "x@y").as_deref(),
            Some("['a@b', 'c@d', 'x@y']")
        );
        assert_eq!(with_enabled("['a@b', 'x@y']", "x@y"), None);
    }

    #[test]
    fn reads_the_state_line() {
        let info =
            "wattcost@itechmeat.github.io\n  Name: wattcost\n  Enabled: Yes\n  State: ACTIVE\n";
        assert_eq!(parse_state(info).as_deref(), Some("ACTIVE"));
        assert_eq!(parse_state(""), None);
    }
}
