# phonelink-linux
phonelink-linux is an app implementation for Linux-based operating system.

Currently tested: KDE Plasma with Ubuntu 26.10 (Kubuntu)

Support for other distros are provided as-is - report any bugs you encounter to the issue tracker.

## Requirements

- Qt 6 with its QML modules and QtWebEngine (tested with Qt 6.10)
- FUSE 3, for the phone's files in `~/PhoneLink`
- A Secret Service provider (GNOME Keyring, KWallet or KeePassXC), to keep the sign-in
- PipeWire or PulseAudio, for the audio of mirrored apps
- BlueZ, for call control and the instant hotspot
- NetworkManager, for joining the instant hotspot
- v4l2loopback, to use the phone as a camera:

```bash
sudo modprobe v4l2loopback card_label="Phone Link" exclusive_caps=1
```

Call control needs the Bluetooth hands-free profile, which PipeWire's WirePlumber takes by default. To give it to Phone Link, turn off WirePlumber's hands-free backend:

```bash
mkdir -p ~/.config/wireplumber/wireplumber.conf.d
printf 'monitor.bluez.properties = {\n  bluez5.hfphsp-backend = "none"\n}\n' > ~/.config/wireplumber/wireplumber.conf.d/51-phonelink-no-hfp.conf
systemctl --user restart wireplumber pipewire pipewire-pulse
```

Bluetooth headsets then play media but no longer work as a microphone for other apps.

## Installing

Debian and Ubuntu:

```bash
sudo apt install ./phonelink_<version>_amd64.deb
```

Fedora and openSUSE:

```bash
sudo dnf install ./phonelink-<version>.x86_64.rpm
```

Arch: build the package with the PKGBUILD in `packaging/arch`:

```bash
makepkg -si
```

The app starts in the system tray at login. Closing its window keeps it running; quit it from the tray menu.

## Building from source

Build from a clone of the whole repository: the app uses the engine in `engine/` and the translations in `gui/`.

You need Rust 1.89 or later, a C++ compiler, CMake and the Qt 6 development files (`qmake6` and the Qt Quick and QtWebEngine headers). From the repository root:

```bash
just linux-run      # build and run
just linux-install  # install for this user in ~/.local
just linux-deb      # build the .deb (needs cargo-deb)
just linux-rpm      # build the .rpm (needs cargo-generate-rpm)
```

A package depends on the exact Qt version it was built with, so build it on the release it is for.

## Note for Tailscale users

When Tailscale accepts a subnet route that covers your local network, traffic to the phone goes through the Tailscale tunnel. Its smaller MTU drops the large video packets, so screen mirroring and the camera show nothing.

Keep local traffic out of the tunnel with a routing rule (replace `192.168.45.0/24` with your network):

```bash
sudo ip rule add to 192.168.45.0/24 lookup main suppress_prefixlength 0 priority 5200
```

The rule is lost on reboot. Alternatively, stop accepting subnet routes with `sudo tailscale set --accept-routes=false`.

## Current limitations

- Call audio stays on the phone; the app places, answers and ends calls only.
- Unfolding a foldable phone stops screen mirroring.
- Disconnecting from the instant hotspot leaves the hotspot on the phone.
- On GNOME, the tray icon needs the AppIndicator extension. Without it, launch the app again to show its window.