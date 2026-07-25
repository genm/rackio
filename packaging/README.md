# Service packaging

The agent is designed to outlive the tray and user login. The templates in this
directory define that process boundary, but installers are not yet automated.

## Linux system service

The installer must create a non-login `tray-monitor` user and a
`tray-monitor-viewers` group, add authorized desktop users to that group, copy
the binary and unit, then enable the service:

```sh
sudo useradd --system --home-dir /var/lib/tray-monitor --shell /usr/sbin/nologin tray-monitor
sudo groupadd --system tray-monitor-viewers
sudo usermod --append --groups tray-monitor-viewers "$USER"
sudo install -m 0755 target/release/tray-monitor /usr/local/bin/tray-monitor
sudo install -m 0644 packaging/linux/tray-monitor.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now tray-monitor.service
```

Log out and back in after the group change. The systemd runtime directory and
0660 socket gate desktop access; the agent also requires Unix peer credentials
to be available for every accepted local connection.

## macOS LaunchDaemon

The final package installer must create `/var/run/tray-monitor` with an
installer-owned viewer group and mode 2770, create the data/log directories,
install the binary, and load the plist under `/Library/LaunchDaemons`.

Do not manually copy this template into production yet: the signed pkg,
dedicated local group, ownership rollback and uninstall receipts remain release
work.

## Windows

Windows Service registration is intentionally not packaged yet. The Rust agent
can collect and serve P2P traffic on Windows, but named-pipe IPC with an explicit
ACL is still a release blocker. Shipping a service before that boundary exists
would leave the tray disconnected or encourage an insecure fallback.
