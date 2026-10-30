# Decisions made while you were away (2026-10-30 night)

You asked for a polished minimal GNOME mail app for Tuta: easy login, read and write simple mail,
attachments must work, KISS, fast, Rust. These are the calls I made, in the order they came up.

## Product

- **Name: Tern** (a small, fast seabird), app id `xyz.hollingworth.Tern`, repo `~/Downloads/tern`.
  The name avoids "Tuta" in the product name; the About dialog says it is an unofficial client.
- **Stack: GTK4 + libadwaita via gtk-rs, no WebKit, no relm4.** Mail HTML is parsed with html5ever into
  a small document model and drawn natively (one TextView; pictures and tables embedded). Remote images
  are never fetched. Mails too rich to show get an "Open in Tuta" button to app.tuta.com.
- **Composer writes plain text.** It is turned into a simple HTML body (`tern_core::text_to_html`).
  No rich-text toolbar: KISS, and it reads fine in every client.
- **Folders only, no labels or "All mail".** Unread counts in the sidebar are 0 for Tuta for now
  (the counter service is not ported); the mail list still shows unread mails in bold.
- **Login is remembered** in the GNOME keyring (Secret Service via `oo7`). The keyring holds the Tuta
  session (access token + encrypted passphrase key), never the password. Sign out deletes the session
  on the server and the keyring item.
- **`demo` login** opens an in-memory account, so the app can be tried without a Tuta account and the
  UI is tested without the network.

## Tuta protocol

- **Tuta's own Rust SDK is vendored**, not depended on by git: Tern adds code inside it (`tern.rs`)
  that needs the SDK's crate-private facades. Every change is listed in `vendor/tuta-sdk/PATCHES.md`.
- **Vendored from the `tutanota-desktop-release-362.261023.2` tag, not master.** First attempt used
  master; app.tuta.com answered its sys model 157 with `400 Bad Request` (probed: 156 and below get 200).
  Update only to newer release tags.
- **Sending is end-to-end encrypted when every recipient is on Tuta** (TutaCrypt or RSA, whichever key
  the recipient has); **if any recipient is outside Tuta the mail goes out as ordinary mail**, which is
  what the official client does when "confidential" is off. Password-protected mail to outsiders is not
  supported. A recipient at a Tuta domain with no key is reported as not found, as the official client does.
- **Recipient key verification (Tuta's key transparency) is skipped**, like the SDK's own send path.
  Keys are taken from the server as served.
- **Two-factor accounts are refused with a clear message** rather than half-supported; the pending
  session is deleted at once. Accounts still on the old bcrypt password format get a message to sign in
  once at app.tuta.com (which upgrades them); the SDK would otherwise panic.
- **Mail from password-protected external mailboxes** (symmetric bucket keys) shows an error instead of
  opening; the SDK has a `todo!()` there and Tern routes around it.
- Notification-mail language is "en"; the account's "send plain text only" setting is ignored (Tern sends HTML).
- **Received mail is not migrated to owner keys** (the official client's UpdateSessionKeysService call is not
  ported). Every open repeats the public-key decryption of the mail's bucket key. Correct, just slower on
  first pages of new mail; port it if paging feels slow.
- **Mail Tern cannot decrypt still gets a row** ("Open this message in Tuta") instead of disappearing.
- **SDK panics cannot take the window down**: every Tuta call runs in its own task, and a panic becomes an
  error toast.
- Sent and Drafts rows show "To: <first recipient>" instead of yourself.

## What is verified, and what is not

- Verified against the real server: sign-in up to the password check (salt, Argon2, session request).
- Not verified live: reading, downloading, sending. There was no Tuta account to test with, and creating
  one needs a captcha and is not my call. The port follows the TypeScript client at the same tag line by
  line, an independent review compared the two, and unit tests cover the encodings
  (draft encryption, blob response parsing, id encoding).
- To prove it on your account: `TERN_TEST_EMAIL=you@tuta.com TERN_TEST_PASSWORD=… cargo test -p tern-tuta -- --ignored`
  sends yourself a mail with a PDF attached, reads it back from Sent and compares the attachment bytes.
