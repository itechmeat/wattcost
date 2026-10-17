# Security

## Reporting a vulnerability

Report vulnerabilities privately through [GitHub security advisories](https://github.com/itechmeat/wattcost/security/advisories/new), not in public issues.

## What wattcost touches

1. It reads sensors from `/sys`, `/proc`, the NVIDIA driver library and the GNOME session bus.
2. It writes only to the user's own config (`~/.config/wattcost`), data (`~/.local/share/wattcost`) and systemd user unit directories.
3. The GNOME extension runs `wattcost` as the logged-in user and never needs root.
4. The optional udev rule printed by `wattcost setup` lets members of a `wattcost` group read the RAPL energy counter. That counter was used by the PLATYPUS side-channel attack (CVE-2020-8694), which is why the kernel restricts it to root by default; apply the rule only if you accept that trade-off.
5. The optional Prometheus endpoint (`[metrics] listen`, off by default) serves power, energy, cost and the current price over plain HTTP without authentication. It reveals when the computer is in use; bind it to `127.0.0.1` or a private network and limit the port with a firewall.
