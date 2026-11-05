<p align="center">
  <img src="data/icons/xyz.hollingworth.Tern.svg" width="128" height="128" alt="Tern">
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/readme/reading-dark.png">
    <img src="docs/readme/reading.png" width="720" alt="Reading a mail in Tern">
  </picture>
</p>

<h1 align="center">Tern</h1>

<p align="center">
  <strong>A small, fast mail client for Tuta, on the Linux desktop and Android.</strong><br>
  One Rust core, drawn natively: GTK 4 and libadwaita on the desktop, Material 3 on Android.<br>
  Unofficial; not affiliated with Tuta GmbH.
</p>

<p align="center">
  <a href="https://github.com/OceanEilonwy/tern/actions/workflows/ci.yml"><img src="https://github.com/OceanEilonwy/tern/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/OceanEilonwy/tern/actions/workflows/release.yml"><img src="https://github.com/OceanEilonwy/tern/actions/workflows/release.yml/badge.svg?branch=main" alt="Release"></a>
  <a href="https://github.com/OceanEilonwy/tern/releases/tag/latest-main"><img src="https://img.shields.io/badge/download-latest--main-3584e4" alt="Download latest-main"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-informational" alt="GPL-3.0"></a>
</p>

<p align="center">
  <a href="#linux-desktop"><b>Linux desktop</b></a> ·
  <a href="#android"><b>Android</b></a> ·
  <a href="https://github.com/OceanEilonwy/tern/releases/tag/latest-main"><b>Downloads</b></a> ·
  <a href="#build-from-source"><b>Build from source</b></a>
</p>

## Why Tern

- **Fast.** Recent mail is kept on your device in an encrypted database, so folders open at once and work offline.
- **Private.** No web view: mail is drawn natively, remote images never load, and the cache key lives in your keyring or the Android Keystore.
- **Conversations.** Mail is grouped by subject across folders, with pins, labels, read and unread, and moving to spam.
- **Search and rules.** Search everything on your device, optionally by meaning. Rules sort new mail into folders, labels or Spam.
- **Several addresses.** Send from any alias; a reply starts from the address the mail came to.
- **Keyboard first on the desktop.** Every action has a shortcut; press Ctrl+? to see them.

<table>
  <tr>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/readme/reply-dark.png"><img src="docs/readme/reply.png" alt="A reply quoting only the picked line"></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/readme/rule-dark.png"><img src="docs/readme/rule.png" alt="Writing a rule, with the mail it already catches"></picture></td>
    <td width="33%"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/readme/cleanup-dark.png"><img src="docs/readme/cleanup.png" alt="Clean Up, freeing room on disk"></picture></td>
  </tr>
  <tr>
    <td align="center">Replies quote only what you pick</td>
    <td align="center">Rules that show what they catch</td>
    <td align="center">Clean Up when the cache grows</td>
  </tr>
</table>

## Install

### Linux desktop

Download from [the latest build](https://github.com/OceanEilonwy/tern/releases/tag/latest-main):

| Your system | Download | Then |
|---|---|---|
| Any distribution with Flatpak: Linux Mint, Ubuntu, Fedora, Void, CachyOS… | `tern-…-x86_64.flatpak` | `flatpak install --user tern-*.flatpak` |
| CachyOS, Arch, Void and other up-to-date distributions | `tern-…-x86_64-linux.tar.gz` | unpack, then `./install.sh` |

Both add Tern to your applications menu with its icon. The tarball needs GTK 4.22 and libadwaita 1.9 or
newer; `./install.sh --uninstall` takes it out again. Flatpak fetches the GNOME runtime from Flathub the
first time.

To try it without an account, type `demo` as the address on the sign-in screen.

To have email links open in Tern, press **Use Tern** in Preferences → Email Links. With the Flatpak, choose
Tern as the mail app in your system's Default Applications settings instead.

### Android

Tern runs on Android 17 and newer, with the same Rust core drawn with Material 3 on true black. It has no
store listing yet. [The latest build](https://github.com/OceanEilonwy/tern/releases/tag/latest-main) has an
APK for each kind of phone (`arm64-v8a` for almost all of them), but these are unsigned, and Android will
not install them until they are signed. For now, [build it from source](android/README.md) or sign the APK
with your own key.

## Build from source

```bash
git clone https://github.com/OceanEilonwy/tern.git
cd tern
./install.sh --deps     # shows the command that installs what the build needs on your distribution
./install.sh            # builds, then installs Tern with its menu entry and icon into ~/.local
```

`./install.sh` checks for Rust, GTK 4.22 and libadwaita 1.9 first and says what is missing. Run
`sudo PREFIX=/usr/local ./install.sh` to install for everyone, and `./install.sh --uninstall` to take it
out again. To update, `git pull` and run `./install.sh` again.

For working on Tern:

```sh
make run          # sign in with your Tuta address and password
make demo         # in-memory demo account, no network
make test         # unit tests, plus the window checks on a headless display (more in docs/TESTING.md)
```

Not supported yet: two-factor sign-in, and password-protected mail to people outside Tuta.

`crates/desktop` is the GTK app, built with Relm4, `android/` the Compose app, and `crates/app` the core they share.
Beneath it, `crates/sync` keeps `crates/cache`, the encrypted cache, up to date from `crates/tuta`, the
Tuta account. `crates/mail`, `rules`, `html`, `compose` and `demo` are the smaller pieces, and
`vendor/tuta-sdk` is Tuta's own Rust SDK with Tern's additions (see `PATCHES.md`).

GPL-3.0, like the Tuta SDK it builds on.
