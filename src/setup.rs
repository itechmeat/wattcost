//! The systemd user unit and the udev rule that makes RAPL readable.

use std::path::{Path, PathBuf};

pub const UNIT_NAME: &str = "wattcost.service";

pub fn unit_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("systemd/user")
        .join(UNIT_NAME)
}

/// The unit text, or the path that cannot be written into an `ExecStart` line safely.
pub fn unit_file(exe: &Path, config: &Path) -> Result<String, String> {
    Ok(format!(
        "[Unit]\n\
         Description=wattcost power and cost logger\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\" --config \"{}\" run\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        systemd_quoted(exe)?,
        systemd_quoted(config)?
    ))
}

/// systemd expands `%` specifiers, `$` variables and backslash escapes inside `ExecStart`, even
/// within quotes.
fn systemd_quoted(path: &Path) -> Result<String, String> {
    let text = path.display().to_string();
    if text.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("{text:?} contains a quote or a control character"));
    }
    Ok(text
        .replace('\\', "\\\\")
        .replace('%', "%%")
        .replace('$', "$$"))
}

pub const LINGER_HINT: &str = "User services start at login. To collect from boot and after logout as well, run: loginctl enable-linger";

pub const UDEV_INSTRUCTIONS: &str = r#"CPU energy (RAPL) is readable by root only. To let the members of a `wattcost` group read it:

  sudo groupadd --system wattcost
  sudo usermod -aG wattcost "$USER"
  sudo tee /etc/udev/rules.d/70-wattcost-rapl.rules >/dev/null <<'EOF'
SUBSYSTEM=="powercap", KERNEL=="intel-rapl:[0-9]|intel-rapl:[0-9][0-9]", RUN+="/usr/bin/chgrp wattcost /sys%p/energy_uj", RUN+="/usr/bin/chmod g+r /sys%p/energy_uj"
EOF
  sudo udevadm control --reload
  sudo udevadm trigger --subsystem-match=powercap --action=add

Then log out and back in so the group applies, and run `systemctl --user restart wattcost`.
Trade-off: group members can read the fine-grained energy counter used by the PLATYPUS
side-channel attack (CVE-2020-8694). Without the rule wattcost uses an AMD integrated GPU's
package power when there is one, and otherwise estimates CPU power."#;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn unit_runs_the_binary_with_the_config() {
        let unit = unit_file(
            Path::new("/opt/bin/wattcost"),
            Path::new("/home/u/.config/wattcost/config.toml"),
        )
        .unwrap();
        assert!(unit.contains(
            "ExecStart=\"/opt/bin/wattcost\" --config \"/home/u/.config/wattcost/config.toml\" run"
        ));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn percent_signs_are_escaped_for_systemd() {
        let unit = unit_file(Path::new("/opt/100%/wattcost"), Path::new("/home/u/c.toml")).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/100%%/wattcost\""), "{unit}");
    }

    #[test]
    fn dollar_signs_and_backslashes_are_escaped_for_systemd() {
        let unit = unit_file(Path::new("/opt/$HOME/a\\b/wattcost"), Path::new("/c.toml")).unwrap();
        assert!(
            unit.contains("ExecStart=\"/opt/$$HOME/a\\\\b/wattcost\""),
            "{unit}"
        );
    }

    #[test]
    fn quotes_and_newlines_cannot_reach_the_unit() {
        for bad in ["/opt/a\"b", "/opt/a\nExecStartPre=/bin/x"] {
            assert!(
                unit_file(Path::new(bad), Path::new("/c.toml")).is_err(),
                "{bad:?}"
            );
        }
    }
}
