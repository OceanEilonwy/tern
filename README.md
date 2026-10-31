# Tern

A small, fast GNOME mail client for [Tuta](https://tuta.com) mail, in Rust with GTK4 and libadwaita.
Unofficial; not affiliated with Tuta GmbH.

Read and write simple mail, with attachments, as conversations. No WebKit: mail is drawn natively, remote
images are never loaded, and anything too rich has an "Open in Tuta" button.

- **Conversations** group mail by subject across folders; each mail is a card with Reply, Reply All and
  Forward under it.
- **Offline cache**: recent mail is kept on this computer in an encrypted SQLite database (SQLCipher, key in
  the GNOME keyring), so folders open instantly; a background sync checks every five minutes. How much is
  kept is set in Preferences (Low, Medium, High). Signing out deletes it.
- **Drafts**: closing a message keeps it in Drafts; it reopens from its conversation.
- **Suggestions** in To, Cc and Bcc from your contacts and the people you write to.
- **Search** (Ctrl+F) through the mail on this computer, across folders: words as you type them (prefixes
  match), `from:`, `to:`, `subject:`, `in:folder` and `has:attachment`. Turn on "Search by Meaning" in
  Preferences to also find mail about what you type; it downloads a 30 MB language model once, and your mail
  never leaves the computer.
- **Spam and rules**: Move to Spam (Ctrl+J) lists the sender's other mail, or mail from their domain or
  with the subject, to move along with it, and can keep that as a rule. Rules in Preferences sort new
  Inbox mail as it arrives (to Spam, a folder, or a Tuta label, which a rule can make), written like
  `(from:@example.com or subject:"offer") and not to:me@tuta.com`; they live in
  `~/.config/tern/rules.toml`.
- **Conversation menu**: right-click a conversation (or press Shift+F10) to pin it to the top of the
  list, mark it read or unread, or move it to Spam or Trash. Pins stay on this computer.
- **Keyboard**: Alt+Up/Down between conversations, Enter to read, Escape back, Delete, Ctrl+1…9 for
  folders, Ctrl+F to search, F9 to fold the folders to icons; Ctrl+? lists them all.
- **Resizable panes**: drag the edge of the folders or the list; drag the folders narrow (or double-click
  their edge) for a rail of icons. Widths are kept.

![Reading a mail](docs/screenshots/reading.png)

## Run

```sh
make run          # sign in with your Tuta address and password
make demo         # in-memory demo account, no network
make install      # binary, desktop file and icon into ~/.local
```

Type `demo` as the address on the sign-in screen to try it without an account. Your session (not your
password) is kept in the GNOME keyring; sign out from the main menu.

Not supported yet: two-factor sign-in, password-protected mail to people outside Tuta, labels.

## Layout

- `crates/core`: the `Backend` trait the app talks to, shared types, the demo account.
- `crates/tuta`: `Backend` for a Tuta account.
- `crates/cache`: the encrypted mail cache, conversations, address suggestions and the sync.
- `crates/app`: the GTK app.
- `vendor/tuta-sdk`: Tuta's own Rust SDK from a release tag, with Tern's additions; see `PATCHES.md`.

## Test

```sh
make test         # unit tests, plus the window checks on a headless display (kwin_wayland or broadway)
cargo test -p tuta-sdk --features net --lib
TERN_TEST_EMAIL=you@tuta.com TERN_TEST_PASSWORD=… cargo test -p tern-tuta -- --ignored   # live round trip
cargo test -p tern semantic -- --ignored    # downloads the search model and finds mail by meaning
```

The live tests send you a mail with a PDF attached, find it in Sent, and compare the attachment bytes;
open mail in a batch and one by one and compare; and move your newest Inbox mail to Archive and back
(and add your first label to it, if you have labels).

GPL-3.0, like the Tuta SDK it builds on.
