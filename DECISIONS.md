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
  line, and an independent review compared the two. It found one real bug, now fixed: the read-token,
  blob and send requests had aggregates without ids (the SDK fills those in only for encrypted requests),
  so they would not have serialized. Unit tests now push every request Tern builds (draft, send in both
  modes, read tokens, blob fetch) through the SDK's own wire serializer.
- To prove it on your account: `TERN_TEST_EMAIL=you@tuta.com TERN_TEST_PASSWORD=… cargo test -p tern-tuta -- --ignored`
  sends yourself a mail with a PDF attached, reads it back from Sent and compares the attachment bytes.

## Morning fixes (2026-10-31)

- **Sign-in with a real account stayed on the sign-in page.** `main` dropped the window's state right after
  building it, and the sign-in callback held only a weak reference to it, so a successful sign-in went
  nowhere (the demo and screenshot paths held their own reference, which hid it). The GTK window now owns
  that state until it is destroyed. `make test` signs in as "demo" through the real button on a headless
  broadway display (`test-ui`) and fails if the mailbox does not open.
- **Every Tuta request has a two-minute limit**, and `TERN_LOG=1` prints each request (method, path,
  status, time; never query strings, headers or bodies) and each sign-in step on stderr.
- **Audit for the same kind of silent failure** (state dropped or never updated, so the window waits forever):
  - Folders that fail to load now show "Could Not Load Folders" with the reason and "Press F5 to try again",
    instead of a spinner behind a toast that disappears. An account with no folders says "No Folders".
  - A refresh (F5, or sending while Sent is open) during a folder's first page left the list thinking a
    page was still loading, so scrolling never loaded older mail. Loads now carry the list generation.
  - Folders arriving after sign-out were put back in the list; signing into the same account then opened
    no folder and spun. Results for a backend that is no longer signed in are dropped.
  - Each has a scene in `make test-ui` that fails with the old code (checked by putting each bug back).
  - Checked and fine: the composer keeps itself alive until its window is destroyed; Mailbox and Reader are
    owned by the window; mail, page and reader loads already ignore stale results.
- **Mail to recipients outside Tuta failed with 500.** Found by comparing requests with app.tuta.com: the
  vendored SDK sent encrypted fields holding a default value (`bodyText` "", `method` 0, `confidential`
  false) as empty strings, where the web client encrypts them. The server decrypts drafts itself only for
  outside recipients, so only those sends broke. The SDK now always encrypts (see PATCHES.md). Sends also
  allow undo now, as the web app does by default: the server sends a few seconds later.

## Second round (2026-10-31)

- **The cache is encrypted** (SQLCipher via rusqlite 0.40.2, linked to the system libcrypto). It holds
  decrypted mail, so a plain file would undo Tuta's end-to-end encryption at rest. The key is random,
  kept in the GNOME keyring per account; without a keyring nothing is written to disk. Signing out
  deletes the database and its key. A database that does not open (other key, older layout) is rebuilt.
- **Offline levels**: Low keeps 100 mails per folder and opens the newest 25 ahead; Medium 500 and 200
  (the default); High 2000 and 1000. Sync runs at sign-in, every five minutes, on F5 and when the level
  changes. Without Tuta's event bus, a folder is paged from the newest until a page brings nothing new.
  The cache is not trimmed: scrolling past the level keeps what it fetched.
- **Conversations are by subject** ("Re:", "Fwd:", "AW:" and the like removed, case and spaces folded),
  across folders. Trash and Spam join a conversation only when listing them. Delete trashes the
  conversation's mails in the folder on show.
- **Closing a composer saves a draft** without asking (an untouched reply is not saved). The trash button
  in the composer discards, after asking, and deletes its draft. Drafts open from their conversation
  ("Continue Editing") or with Enter in Drafts.
- **Drafts on Tuta** (done by a helper agent, reviewed): updates mirror the TypeScript `updateDraft`.
  Attachments already on the draft are not sent again (the TS client does not either); removed ones go in
  `removedAttachments`. A draft is saved confidential only if every recipient is on Tuta.
- **Icons**: Adwaita's reply icons read as back/forward arrows, so Tern draws its own curved ones.
  Actions under each mail are labelled pill buttons; "New" in the list header is labelled and blue.
- **Keyboard**: Alt+Up/Down move between conversations from anywhere; arrow keys move in the list
  without leaving it on a narrow window; Enter reads (or edits a draft); Escape goes back; Ctrl+1…9 pick
  folders; Ctrl+U marks unread and leaves the conversation so it stays unread.
- **Window checks run on headless kwin** (`kwin_wayland --virtual`) when it is installed: broadway draws
  no frames without a browser watching, so text views never settled and pictures lied.
- Found while checking: a text view outside a scrolled window keeps the height of its first layout,
  made before it knows its width, until asked again; the reader asks once laid out. Inline pictures are
  fitted to the card's inner width; fitting them to the column made the card grow every frame.

## Third round (2026-10-31)

- **Arrow keys jumped to the top on unread mail**: opening an unread mail marks it read, which reloaded
  the list by replacing every row, and the list's keyboard focus went with the replaced row. The list
  now changes rows in place and only inserts or removes the conversations that came or went, so focus
  and scroll stay. The `arrows` window check moves the focus onto an unread conversation and fails on
  the old code (focus back on row 0).
- **Resizable panes**: libadwaita's split views cannot be dragged, so a thin handle on each pane's edge
  sets the split view's width bounds; a pane takes at most a share of the window (folders 30%, list
  60%) so a narrower window still narrows it. Widths go to `settings.toml` when a drag ends.
- **Folder rail**: dragging the folders under 140 px, a double click on their edge, or F9 folds them to
  icons with the unread count under each (names in tooltips). F9 on a narrow window shows or hides the
  folders instead. The rail keeps the last width for when it opens again. A double click on the list's
  edge puts it back to its default width.
- **One word for what Tern is doing**, beside a spinner in the list header and in each composer's
  header: Fetching…, Opening…, Downloading…, Deleting…, Attaching…, Drafting…, Sending…. Each piece of
  work holds the word while it runs; when several run, the one the person is waiting on wins
  (Sending over Drafting over Attaching over Deleting over Downloading over Opening over Fetching).
  Opening counts only after 150 ms, so mail from the cache does not flicker it. The background sync
  says Fetching for its whole round, opening mail ahead included.
- **Settings are TOML** (`~/.config/tern/settings.toml`), written by hand as four plain lines and read
  with `toml` 1.1.8, which the build already used. Each value falls back to its default on its own. An
  old `settings.json` is moved over once and removed.
- **Sent toast** names the mail: `Sent “Lunch on Friday?”`, cut at 40 characters.
- Window checks now run with their own `XDG_CONFIG_HOME`, so they never read or write your settings.
