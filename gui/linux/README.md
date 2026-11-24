# Phone Link for Linux

The Linux app of this repository: the same engine as the macOS app (`engine/`) with a Qt 6 / QML window.

Tested on KDE Plasma with Ubuntu 26.10 (Kubuntu). Other desktops and distributions should work but are
untested; report problems in the issue tracker.

## What it does

The same as the macOS app, except:

| Not on Linux | Instead |
|---|---|
| Sharing a photo with another app | Linux desktops have no share menu: copy, save or drag the photo instead. |
| Opening files with Phone Link, or dropping them on its icon | Drop them on the phone in the sidebar, or use Send Files to Phone… in the tray menu. |
| The computer's desktop picture on the phone, on some desktops | It works on GNOME, Budgie, Cinnamon, MATE, KDE Plasma and Xfce, with a picture set; GNOME's slideshows and timed pictures send the picture they show (Plasma's slideshow and picture of the day send none). |
| Updating from within the app | The package manager updates it. |

## Requirements

- Qt 6 with its QML modules and QtWebEngine (tested with Qt 6.10 and 6.11)
- FUSE 3, for the phone's files in `~/PhoneLink`
- A Secret Service provider (GNOME Keyring, KWallet or KeePassXC), running and unlocked, to keep the sign-in
- PipeWire or PulseAudio, for the audio of mirrored apps and of calls
- BlueZ, for calls and the instant hotspot
- NetworkManager (1.16 or later), for joining the instant hotspot and for the network type and Wi-Fi
  signal the phone shows for this computer
- power-profiles-daemon or tuned-ppd (optional), to tell the phone when the computer saves power; the
  battery comes from `/sys/class/power_supply`
- v4l2loopback, to use the phone as a camera. Phone Link uses the loopback device labeled `Phone Link`:

```bash
sudo modprobe v4l2loopback card_label="Phone Link" exclusive_caps=1
```

With other loopback devices next to it (such as OBS's virtual camera), give each its label, for example
`devices=2 card_label="Phone Link","OBS Virtual Camera" exclusive_caps=1,1`. With a single unlabeled
loopback device, Phone Link uses that one.

Calls need the Bluetooth hands-free profile, which PipeWire's WirePlumber takes by default. To give it to Phone Link, turn off WirePlumber's hands-free backend:

```bash
mkdir -p ~/.config/wireplumber/wireplumber.conf.d
printf 'monitor.bluez.properties = {\n  bluez5.hfphsp-backend = "none"\n}\n' > ~/.config/wireplumber/wireplumber.conf.d/51-phonelink-no-hfp.conf
systemctl --user restart wireplumber pipewire pipewire-pulse
```

Bluetooth headsets then play media but no longer work as a microphone for other apps.

## Installing

Debian and Ubuntu (`amd64` or `arm64`):

```bash
sudo apt install ./phonelink_<version>_<arch>.deb
```

Fedora (the package names Fedora's Qt packages as its dependencies):

```bash
sudo dnf install ./phonelink-<version>.<arch>.rpm
```

Arch: build the package with the PKGBUILD in `packaging/arch`:

```bash
makepkg -si
```

The packages open Phone Link in the system tray at login; turn that off in Settings › General › Open
Phone Link at login. Closing the window keeps Phone Link running: quit it from the tray menu or with
Ctrl+Q, and open the window again from the tray or from your apps (also with the tray icon turned off in
Settings › General). Without a tray (GNOME without the AppIndicator extension), closing the window quits.

## Using it

- **Pairing a phone**: with no phone on your account yet, Phone Link shows a QR code to scan with Link to
  Windows on the phone. Pair another phone from Settings › Account › Pair a New Phone…
- **Calls**: an incoming call opens a small window with Answer and Decline, and this computer rings while
  it shows. A call you place or answer here uses this computer's microphone and speakers, and opens a
  call window to mute it, put it on hold, use the keypad, transfer it to your phone and back, or end it;
  closing that window asks whether to end the call. A call you place or answer on your phone stays
  there: a notification offers to transfer it (turn it off in Settings › Calls). Without a microphone
  or a sound output on this computer, calls you answer here are heard on your phone. While the desktop's Do
  Not Disturb is on, an incoming call shows nothing here and rings on your phone only; this works where
  the notification server says so (KDE Plasma does). A `tel:` link, such as a phone number on a web
  page, opens the Calls keypad with the number once Phone Link is the handler for phone links.
- **Sending files**: Send Files to Phone… in the tray menu, or Send Files in the sidebar, opens a window
  to choose files; files dropped on the phone in the sidebar are sent from that window too. Settings ›
  Files sets the folder for files the phone sends.
- **Media keys**: with Settings › General › "Use media keys and the desktop's media controls" turned on,
  what plays on the phone shows in the desktop's media controls (MPRIS) and the media keys control it.
- **Phone files**: the phone's storage is mounted at `~/PhoneLink` while the phone is connected.
  - Files already on the phone are read-only, because the phone can't replace a file's content. To
    change one, copy it to your computer and copy the edited file back under a new name (or delete the
    original first).
  - Copying a new file onto the phone finishes when the phone has it; if the upload fails, the program
    that copied it reports the error. New files wait in `~/.cache/phonelink/uploads` meanwhile, which
    needs room for them.
  - Phone Link doesn't send names starting with a dot, or temporary and system files such as `.tmp`,
    `.temp`, `.lnk` and `.ini` files; copying those fails with "Operation not supported".
  - There is no Trash on the phone: file managers offer to delete permanently, and the phone keeps the
    file in its own recycle bin where it has one.
  - Opening a file downloads all of it, so turn off previews for `~/PhoneLink` in your file manager if a
    folder of photos opens slowly.
- **Pairing over Bluetooth** (for calls and the instant hotspot): Phone Link shows the code your phone
  shows, to compare before you pair.
- **Instant hotspot**: joining adds a temporary NetworkManager connection for your user, which goes away
  when you disconnect; the computer then returns to its usual Wi-Fi.
- **Screen lock**: Phone Link stops what needs an unlocked computer while the screen is locked. It reads
  the lock state from logind on KDE and GNOME and from the screen saver on Xfce, MATE and Cinnamon. On
  desktops with neither (such as sway or Hyprland), it can't see the lock and treats the screen as
  unlocked.

## Where it keeps data

| Path | What |
|---|---|
| Secret Service item "Phone Link engine store" | The sign-in and the computer's registration |
| `~/.local/share/phonelink` | Messages, contacts, photos list and the other data kept from the phone |
| `~/.cache/phonelink` | Pictures shown in the window, files dragged out of mirrored apps, files being copied to the phone |
| `~/PhoneLink` | The phone's files, while the phone is connected |
| `~/.local/state/phonelink` | The log |
| `~/.config/phonelink/phonelink.conf` | The window's own settings, such as the tray icon, the sizes of mirrored apps' windows, the apps the notification settings list and media keys |
| `~/.config/autostart/phonelink-linux.desktop` | Written when you change "Open Phone Link at login" |

Signing out deletes the data kept from the phone and the cached pictures.

## Logs

Phone Link writes its log to `~/.local/state/phonelink` (`$XDG_STATE_HOME/phonelink`), with account and
device IDs, names, numbers and other personal details replaced by REDACTED tags. Settings › Advanced › Log
saves a copy for a bug report, or clears it. For more detail, start Phone Link from a terminal with
`RUST_LOG`:

```bash
RUST_LOG=debug phonelink-linux
```

When it can't open the log file, Phone Link writes its log to standard error instead.

## Uninstalling

Sign out in Settings first, then remove the package (`sudo apt remove phonelink`, `sudo dnf remove phonelink` or `sudo pacman -R phonelink-git`) and, if you like, the folders above.

## Building from source

Build from a clone of the whole repository: the app uses the engine in `engine/` and the macOS app's translations in `gui/mac/`.

You need Rust 1.89 or later, a C++ compiler, CMake, and the development files of Qt 6 (`qmake6`, Qt Quick, Qt Widgets and QtWebEngine), of PulseAudio and of the SBC codec library (`libsbc`). On Ubuntu:

```bash
sudo apt install build-essential cmake qt6-base-dev qt6-declarative-dev qt6-webengine-dev libpulse-dev libsbc-dev
```

From the repository root:

```bash
just linux-run      # build and run
just linux-check    # lints, formatting and tests
just linux-install  # install for this user in ~/.local
just linux-deb      # build the .deb (needs cargo-deb)
just linux-rpm      # build the .rpm (needs cargo-generate-rpm)
```

`just linux-install` also needs `desktop-file-install` (from `desktop-file-utils`).

A package depends on the exact Qt version it was built with, so build it on the release it is for.

## Note for Tailscale users

When Tailscale accepts a subnet route that covers your local network, traffic to the phone goes through the Tailscale tunnel. Its smaller MTU drops the large video packets, so screen mirroring and the camera show nothing.

Keep local traffic out of the tunnel with a routing rule (replace `192.168.45.0/24` with your network):

```bash
sudo ip rule add to 192.168.45.0/24 lookup main suppress_prefixlength 0 priority 5200
```

The rule is lost on reboot. Alternatively, stop accepting subnet routes with `sudo tailscale set --accept-routes=false`.

## Current limitations

- Calls on this computer have no echo cancellation of their own; with speakers, the other party may hear
  an echo. A headset avoids it.
- A mirrored app's window doesn't keep the app's shape while you resize it: the picture is letterboxed.
  On Wayland, mirrored apps' windows and the call windows open where the desktop places them.
- Unfolding a foldable phone stops screen mirroring.
- Disconnecting from the instant hotspot leaves the hotspot on the phone.
- The camera starts streaming when another app starts reading the `Phone Link` device. v4l2loopback
  versions that report this tell Phone Link directly; with older ones, Phone Link looks at the processes
  using the device, which some sandboxed apps hide: if a video call shows nothing, turn the camera on
  with the Camera tile in the sidebar.
- On GNOME, the tray icon needs the AppIndicator extension.
