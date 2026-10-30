# Tern

A small, fast GNOME mail client for [Tuta](https://tuta.com) mail, in Rust with GTK4 and libadwaita.
Unofficial; not affiliated with Tuta GmbH.

Read and write simple mail, with attachments. No WebKit: mail is drawn natively, remote images are never
loaded, and anything too rich has an "Open in Tuta" button.

![Reading a mail](docs/screenshots/reading.png)

## Run

```sh
make run          # sign in with your Tuta address and password
make demo         # in-memory demo account, no network
make install      # binary, desktop file and icon into ~/.local
```

Type `demo` as the address on the sign-in screen to try it without an account. Your session (not your
password) is kept in the GNOME keyring; sign out from the main menu.

Not supported yet: two-factor sign-in, password-protected mail to people outside Tuta, search, labels,
saving drafts.

## Layout

- `crates/core`: the `Backend` trait the app talks to, shared types, the demo account.
- `crates/tuta`: `Backend` for a Tuta account.
- `crates/app`: the GTK app.
- `vendor/tuta-sdk`: Tuta's own Rust SDK from a release tag, with Tern's additions; see `PATCHES.md`.

## Test

```sh
cargo test -p tern -p tern-core -p tern-tuta
cargo test -p tuta-sdk --features net --lib
TERN_TEST_EMAIL=you@tuta.com TERN_TEST_PASSWORD=… cargo test -p tern-tuta -- --ignored   # live round trip
```

The live test sends you a mail with a PDF attached, finds it in Sent, and compares the attachment bytes.

GPL-3.0, like the Tuta SDK it builds on.
