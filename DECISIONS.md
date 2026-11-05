# Decisions made while you were away (2026-10-30 night)

You asked for a polished minimal GNOME mail app for Tuta: easy login, read and write simple mail,
attachments must work, KISS, fast, Rust. These are the calls I made, in the order they came up.

## Product

- **Name: Tern** (a small, fast seabird), app id `xyz.hollingworth.Tern`, repo `~/Downloads/tern`.
  The name avoids "Tuta" in the product name; the About dialog says it is an unofficial client.
- **Stack: GTK4 + libadwaita via gtk-rs, no WebKit, no relm4.** Mail HTML is parsed with html5ever into
  a small document model and drawn natively (one TextView; pictures and tables embedded). Remote images
  are never fetched. Mails too rich to show get an "Open in Tuta" button to app.tuta.com.
- **Composer writes plain text.** It is turned into a simple HTML body (`tern_compose::text_to_html`).
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

## Search (2026-10-31)

You chose hybrid search: keyword search and search by meaning, fused into one list. Built on branch `search`.

- **Keyword index: SQLite FTS5 inside the encrypted cache.** The bundled SQLCipher (SQLite 3.51.3) has FTS5
  compiled in; every cache test now runs through it. One table, `words`: subject, sender (name and address),
  recipients (To, Cc, Bcc once the mail is opened, else the first recipient) and the body as plain text.
  Tokenizer `unicode61 remove_diacritics 2` ("cafe" finds "café"), with a prefix index for 2 and 3 letters.
  The HTML is reduced to text by a small function in `tern-cache` (`search::plain_text`: tags, styles and
  scripts out, common entities decoded), not by an HTML parser; the index only needs the words.
- **The index is kept in sync in Rust, not by triggers**: `merge_page`, `put_mail`, `remove`, mails gone from
  the server and `set_folders` rewrite or drop a mail's words (and its vector). The folder is not indexed but
  joined at query time, so moving a mail needs no index change. `mails` got an explicit
  `id INTEGER PRIMARY KEY`, the index's row id, because SQLite may renumber implicit row ids on VACUUM.
  `SCHEMA_VERSION` is now 2: **an existing cache is dropped once and fetched again** on the first start.
- **Query syntax**: plain words, "quoted phrases", `from:`, `to:`, `subject:`, `has:attachment`, `in:<folder>`
  (folder name or kind, any case). All terms must match. Every term becomes a quoted FTS5 prefix phrase, so
  nothing typed is FTS5 syntax (`AND`, `NEAR(`, `*`, `^`, stray quotes are words); terms with no letter or
  digit drop out. A test throws 2000 random strings of FTS5 punctuation at it. Unknown `x:` terms are words
  (`12:30` stays a time). Trash and Spam are left out unless `in:` names them.
- **Ranking**: BM25 with the subject weighing 4, the sender 2, recipients and body 1. A query of filters only
  (`from:sam`) lists newest first.
- **Search by meaning: a static embedding model (model2vec), potion-base-8M** (MIT, 256 dimensions, 30 MB,
  English). A static model is a table of one vector per word piece; a text's vector is the mean. Pure Rust, no
  neural network at run time, about 0.2 ms per mail. Transformers through candle or ort were not worth it here:
  hundreds of MB of dependencies or a native ONNX runtime, and milliseconds per mail. potion-retrieval-32M
  (128 MB) ranks a little better and potion-multilingual-128M (about 500 MB) knows German; both were too big
  for an opt-in download. Non-English mail still has keyword search; meaning works best in English.
- **No model2vec or tokenizers crate**: they pull in Hugging Face's tokenizers, hf-hub, ndarray and more.
  Tern reads the two files itself (`tern-cache/src/embed.rs`): the WordPiece vocabulary from `tokenizer.json`
  and the table from `model.safetensors` (an 8-byte length, a JSON header, raw floats). The BERT tokenizer is
  written out (lower case, accents off, punctuation and CJK apart, longest pieces first), about 100 lines.
  Checked against Hugging Face's own tokenizer (0.23.3) on six texts with accents, CJK, punctuation, an
  address, a URL and a soft hyphen: identical ids. That check is an ignored test with the reference ids.
  Accents are stripped by a table for the Latin letters U+00C0–U+024F instead of Unicode decomposition,
  which would need another crate; other letters pass unchanged, and the English vocabulary knows few of them.
- **Opt-in, in Preferences → Search → "Search by Meaning"**, which names the 30 MB download. Off by default:
  keyword search needs nothing. Turning it on downloads the model from huggingface.co at a pinned revision,
  checks each file's SHA-256, and keeps it in `~/.cache/tern/models/potion-base-8M` (public data, not mail).
  The download uses the Tuta SDK's own HTTPS client (hyper and rustls, already in the build) through
  `tern_tuta::download`, which follows redirects only to HTTPS. Turning it off deletes every vector and keeps
  the download, so turning it on again works offline. The setting is one field, `semantic`, in `Settings`.
- **Vectors live only in the encrypted database** (table `vectors`), since they reveal what mail is about.
  Stored as one byte per number with a scale (260 bytes instead of 1 KB; cosines off by about 0.005), and
  compared by brute force in Rust. Measured on 20,000 mails in an encrypted file, release build: keyword search
  12–25 ms, keyword and meaning 35–70 ms (140 ms with 32-bit floats). No sqlite-vec. Searches run off the
  UI thread; a late result for older text is dropped.
- **Indexing by meaning runs in the background sync, after mails are opened ahead**, a hundred at a time on a
  blocking thread: 20,000 mails take about 10 s on disk. A mail's vector is worked out from its subject and the
  first 3000 characters of its body (the model reads 512 word pieces). Mails not opened yet get one from the
  subject; when the body arrives the vector is dropped and worked out again. A vector worked out from text that
  changed meanwhile is not stored. Until the first background round has loaded the model after start, search
  is by keyword only.
- **A mail counts as close in meaning at a cosine of 0.25 or more** (`MEANING_MIN`), else every mail would be
  found. Measured with the real model on mails like the demo's: related pairs scored 0.28–0.46 ("pictures" and
  "Photos from Saturday" 0.46, "security" and a password reset 0.28), unrelated ones at most 0.24.
- **Fusion: reciprocal rank fusion** with k = 60: each mail scores 1/(60 + rank) in each list it is in. A
  conversation takes its best mail's score (the maximum, so a long thread of weak matches does not beat one
  strong match), and the newer conversation wins a tie. A conversation found opens in the folder of its best
  match, so a match in Trash (with `in:trash`) shows the trashed mails.
- **UI**: a search button and Ctrl+F in the list's header open a search bar under it. Results replace the list
  (title "Search") and update as the cache changes; Enter opens the best match; Escape, the button or picking a
  folder go back to the folder. Clearing the text goes back to the folder too but keeps the bar open. Found
  while testing: GTK announces an empty search at once, also between select-all and the next letter, and the
  first version closed the bar there; the `search` window check types over old text and failed on it. Also
  found: typing on with a result open made the list select that result's neighbour, which opened it and
  marked it read. New words now leave nothing open; the check types on after opening one and failed before.
  Nothing found says "No matches in mail on this computer": search sees only what the offline level keeps,
  and body words only of mails opened or opened ahead. Search shows the best 100 conversations, no paging.
- **Tests**: query parsing, FTS5 quoting and random input, plain text, fusion (both lists, grouping, ties), the
  tokenizer against a tiny vocabulary, the stored-vector accuracy, the index following moves, removals, new
  bodies and folder changes, meaning with a two-dimensional test model and filters, vectors redone after a
  change, an older layout rebuilt with its index, and the no-plain-text check now also looks for an indexed
  body word and a vector in the database file and its log. The `search` window check (demo account) searches
  by filter, body words and a prefix, opens a result, shows "No Matches", goes back with Escape, and opens a
  conversation spanning Inbox and Sent. Ignored, needing the model or the network: the reference token ids
  (`TERN_MODEL_DIR=… cargo test -p tern-cache -- --ignored`) and the real download
  (`cargo test -p tern semantic -- --ignored`); both passed on 2026-10-31.
- **New dependencies: none.** `tern-tuta` now also depends on the workspace's `crypto-primitives` (vendored
  Tuta SDK, already built) for SHA-256.

## Fetching (2026-10-31)

Opening a mail used to cost 3 to 5 requests in a row: the Mail, a Group for its key pair (every mail
with a bucket key), a BlobAccessTokenService POST, the MailDetailsBlob, and the files when it had any.
Mail is now fetched in batches, and the keys and tokens a batch needs are loaded once.

- **Batched open** (`TernMail::open_many`, `Backend::open_many`): one GET per mail list for the mails,
  one per archive for the bodies, one per list for draft bodies and for attachment files, each with up
  to 100 ids comma-joined (Tuta's limit). Bodies follow the TypeScript client's `loadMultipleBlobElements`
  (EntityRestClient at the 362.261023.2 tag): one read token per archive, the same query parameters as
  the single load that already worked, `ids=a,b,c`; a 403 evicts the token and tries once more, as
  `doBlobRequestWithRetry` does. A failed request fails the batch; a mail missing or not decrypting
  fails alone.
- **Keys and tokens once**: key pairs are kept per group and version for the session (the SDK loaded
  the Group on every bucket key; this also speeds up listing, which decrypts 50 mails a page). Archive
  read tokens are kept until they expire, as the TypeScript client's BlobAccessTokenCache does.
- **Requests, counted from the code** (bucket-key mail, one mail list and archive, some attachments):
  a 10-mail conversation went from 30–50 requests (4–5 per mail) to about 7 (the focused mail: mail,
  token, body, files, Group; then the other nine: mail, body, files). Opening 200 mails ahead went from
  600–1000 requests one after another to about 17: five batches (10, 25, 50, 100, 15) of three each,
  plus one token per archive and one Group. Mail spread over several mail lists or archives adds one
  request per extra list per batch.
- **Policy** (`tern_cache::policy`, pure): Reading fetches the focused mail alone first, then the rest
  of the conversation newest first in one batch. Viewing fetches the newest mail of the first 25
  conversations of the list on show (a tall window's worth; tracking which rows are actually on screen
  needed layout timing for little gain, and the background covers the rest). Background opens ahead in
  batches of 10, 25, 50, then 100: the first is small so the first bodies land soon and anything that
  ends up waiting on it waits little; the later ones are large because each batch costs about three
  requests whatever its size. Mails the cache has are left out.
- **Fetcher** (`crates/sync/src/fetcher.rs`, on the Tokio runtime): the one place bodies are fetched.
  Three requests at most at once: the background may use one, the list two, the reader all three, so a
  conversation being read always has a lane free and the list one besides the background. Most urgent first
  (Reading, Viewing, Background), oldest first within; a mail already queued less urgently moves to the
  more urgent batch; nothing queued or in flight is asked for again. A newly opened conversation goes
  ahead of the one opened before (that one drops to Viewing). A card waiting on a mail that a list or
  background batch is fetching fetches it alone at once on the free lane rather than waiting for that
  whole batch; that costs one duplicate fetch, rarely.
- **Fallbacks**: a batch that fails for any reason but offline, signed out or rate-limited is opened one
  mail at a time the old way (logged once with `TERN_LOG`), so a server that refuses the batched shape
  still works, slower. A mail that fails inside a good batch is tried once alone before it is marked
  unopenable. A 429, or a 503 with `Retry-After`/`Suspension-Time`, is now `Error::RateLimited` instead
  of a failure: the batch waits as told (5 s if not told, 60 s at most) and goes again, and no mail is
  marked unopenable for it; the SDK's suspendable client also holds every other request until then.
  Offline or signed out, the queue stops and keeps its work; cards waiting show the error as before, and
  the next sync round or a conversation opened starts the queue again.
- **Activity word**: the reader still says Opening… after 150 ms while a card waits; the background
  round says Fetching… while the fetcher opens ahead.
- `sync::bodies` is gone (the fetcher's `ahead` replaces it). `Backend::open` stays: the fallback and
  the alone-retry use it.
- Window check `focus-first`: batches take 5 s, single mails none; the list's batch holds the mail
  chosen, which must show within 2 s while that batch is still on its way. It fails if the card waits
  behind the batch.
- **Not verified against Tuta**: the batched requests (several ids for Mail, MailDetailsBlob,
  MailDetailsDraft and File) are built like the TypeScript client's and like the single loads that work,
  but no account was at hand. `cargo test -p tern-tuta -- --ignored live_batch_open` (with
  `TERN_TEST_EMAIL` and `TERN_TEST_PASSWORD`, `TERN_LOG=1` to see each request) opens the newest 20
  mails of Inbox, Sent and Drafts in one batch and compares each with a single open. Should the server
  refuse the shape, the fallback keeps Tern working one mail at a time. The rate-limit path is tested
  only against scripted answers.
- Found while checking: the `draft` window check fails about one run in ten on main too (the saved
  draft does not reach the Drafts list within its five seconds); not changed here.

## Typos in search (2026-10-31)

- **A misspelt word finds what was meant**: "touch rutby" finds "touch rugby". A plain word of four
  letters or more that is in fewer than three mails also matches the one indexed word nearest to it:
  one typo for up to seven letters, two from eight (a letter wrong, missing, extra, or two swapped).
  The nearest is the one with the fewest typos, then in the most mails, and it must be in at least
  twice as many mails as the typed word, so a rare word spelled right is left alone.
- **One fix per word, not several**: BM25 ranks rarer words higher, so offering several near words
  let an unlikely one ("ruby") outrank the meant one ("rugby"). The typed word still matches too and
  ranks first when present.
- **No new storage**: the words come from FTS5's own `fts5vocab` view of the index, created per
  connection in the temp schema; no cache rebuild. Only words starting with the first or second
  letter typed are compared, which keeps a typo search at about 95 ms against 1.4 million distinct
  words (far more than a real mailbox holds). A dropped first letter ("ugby") is not found.
- Filters (`from:`, `subject:`…) and quoted phrases are matched as typed.

## Fourth round (2026-10-31)

- **A tighter rail**: on the rail each folder is its icon alone, with the unread count as a small
  badge on its corner instead of a line under it, and the main menu moves to the list's header. The
  rail is 52 pixels, down from 92 (more with KDE's window icon on the folders' header, which the
  panes scene now sets so the width is checked as the user sees it). Counts over 99 show as "99+";
  the tooltip has the exact number.
- **Sent icon**: Tern draws its own outlined paper plane, as it does for Inbox and Archive; the
  theme's `mail-send` was cut off in Breeze.
- **One language for rules and Move to Spam**: a rule's line is the same thing the spam dialog's
  search box holds, so what the dialog lists is exactly what a kept rule will catch. `from:`
  (`sender:`), `to:`, `subject:` (`title:`), bare words (subject or sender), `and`, `or`, `not`/`-`,
  brackets; `@domain` matches the domain and its subdomains. It lives in `tern-rules` and is
  separate from the search syntax: search is ranked full-text over bodies too, rules are a yes/no
  test that must run on what the sync has before bodies are opened.
- **Rules see what the list sees**: sender, the first recipient and the subject. Tuta's list entries
  carry only the first recipient; all recipients and the body need the mail opened, which rules must
  not wait for. `to:` is documented as the first recipient.
- **Rules sort new mail only**, after each sync and before mail is opened ahead, oldest first. A
  mark in the cache (`marks` table, added without a schema bump so no rebuild) holds the newest
  Inbox mail looked at; the first run only sets it, so turning rules on never sweeps old mail
  unasked. Old mail is what the Move to Spam dialog is for; the rule page says how many mails here
  already match. Mail this account sent is never moved. Of the rules that catch a mail, the first
  that moves it wins; every label applies. On an error nothing advances, so the next sync tries
  again.
- **Rules name folders and labels as shown**, not by id, so `rules.toml` reads and edits by hand. A
  rule naming one that is gone does nothing. Tern does not create folders or labels (that needs
  Tuta's key handling for new mail sets); labels are made in Tuta and picked in Tern. Labels are not
  shown in Tern's sidebar yet: a mail can carry many, and the cache keeps one place per mail.
- **A broken rule is kept**, flagged in Preferences, and catches nothing, rather than being dropped
  from the file.
- **Moving uses Tuta's MoveMailService and ApplyLabelService** (added to the SDK beside its simple
  move, in batches of 50). The demo account gained three offers from one sender for the spam scene;
  tests that counted the demo Inbox were updated.
- **Ctrl+J** moves to Spam (Thunderbird's J; a bare letter would fire while typing).


## Fifth round (2026-10-31)

- **Rules can make a label**: the rule page's Label list ends with "New Label…", which asks for a
  name; saving makes the label in Tuta (ManageLabelService, its name encrypted with a new session key
  as the official client does) before the rule is kept. New labels get Tuta's blue; Tern shows no
  label colours. Picking an existing label is unchanged. Labels still are not in Tern's sidebar.
- **Conversations read newest first**: the newest mail is the top card and is the one open, older
  mails below. The cache still hands them over oldest first; only the reader turns them round.
- **Pins are Tern's own**: Tuta has no pinning, so a pin lives in the encrypted cache on this
  computer (a `pins` table by conversation, like `marks` added without a schema bump, and spared
  when a new schema rebuilds the cache, since the server cannot give pins back). Pinned
  conversations come first in every folder they are in, newest first among themselves, with a pin
  beside the sender; search results keep their ranking and show the pin. A pin is by conversation
  subject, so a new conversation with the same subject comes in pinned.
- **The conversation menu** (right click, long press, the Menu key or Shift+F10): Pin/Unpin, Mark
  as Read/Unread, Move to Spam… (the same dialog as Ctrl+J), Move to Trash. It acts on the
  conversation clicked, not the one open. Read marks every unread mail of it in that folder; Unread
  marks only its newest there, as Mark as Unread does for the open mail, and closes it if open so
  showing it does not mark it read again. Spam is left out in Spam, Trash in Trash. Mail cards in
  the reader have no menu: their buttons do the same.
- **The rail's icon is centred**: KDE puts the window's icon at the start of the folders' header,
  3 px left of the 52 px rail's middle. On the rail, when the start of the window's button layout
  holds only the icon, Tern hides it and shows the app's icon in the header's middle instead; any
  other layout (close buttons on the left) is left alone. The panes scene checks the middle.
- **A pin icon of Tern's own**: Breeze has no `view-pin-symbolic`.
- **The rule page's look is not changed this round**: mockups of a nicer one are for the user to
  pick from first.

## Sending from an alias (2026-10-31)

- **Which addresses**: the mailbox's main address and its aliases that are turned on, read from its
  GroupInfo at sign-in (a new alias shows after signing in again). Disabled aliases are left out:
  Tuta refuses mail from them.
- **The From list shows only with more than one address**, above To. A new mail starts from the main
  address; a reply or forward from the first of the account's addresses in the mail's To or Cc (so
  mail to an alias is answered from it), else the main one; a reopened draft from the address it was
  saved with. Reply All leaves every one of the account's addresses out, not only the main one, and
  mail from any of them counts as sent by the account.
- **The display name stays the account's**: Tuta keeps per-alias sender names in mailbox properties,
  which Tern does not read yet.
- The demo account has a second address, `news@you.example`, which its newsletter came to.

## Rule page and quoting, as designed (2026-10-31)

Built from the approved mockups (rule editor A; the quoting design with its four scenarios).

- **The rule's line** is a one-line text view coloured as it reads (`rules::parts`, which never
  fails, so half-typed lines colour too): fields blue, purple and orange, operators and brackets
  grey. Enter saves; pasted line breaks become spaces. Buttons under it put in `from:`, `to:`,
  `subject:`, `and`, `or`, `not` and brackets at the cursor, spaced from their neighbours; brackets
  wrap a selection. The help moved behind "How to Write a Rule".
- **The preview never grows past four mails**: a message (no match, how many, or why the line does
  not read), the newest four, then "and N more, older" with Show All in Search. That closes
  Preferences and lists, in the main window's search, the conversations the rule catches outside
  Trash and Spam (`Cache::rule_hits`); typing over the line in the bar goes back to an ordinary
  search, as the search syntax differs from the rules'.
- **What happens** is three linked buttons; Delete Rule is a red card of its own.
- **A saved rule's line showed squeezed** when its page opened as Preferences opened (24 px for a
  42 px line): the text view's height was worked out before it showed. It is worked out again once
  shown; the `rule-edit` scene (now a check) fails without that.
- **Replies quote nothing unless asked.** Lines selected in the mail before Reply are quoted above
  the answer, with an × to leave them out. The whole mail waits in a dashed pill under the text:
  `•••` shows it, "+ Quote …" adds it and turns the pill blue with an ×. A forward starts with the
  pill blue (the mail goes along) and its × leaves the mail's text out; forwarded attachments stay
  in the attachment list either way.
- **Quotes are sent as HTML blockquotes** (picked lines with "Sam wrote:", the whole mail with
  "On …, Sam wrote:"), which other mail apps indent and fold; a forward's text follows its header
  unquoted, as before. They are kept out of the text being written, so a saved draft carries them
  in its body and reopens with them as text.
- The pill sits at the bottom of the writing area, not straight under the last line: the text area
  fills the window so a click anywhere in it types.

## Reopened drafts keep their quotes apart (2026-10-31)

- Tuta stores a draft's body only, with the quotes merged in, and reopening turned it all into
  text. Now both ways back, the first that applies:
  - **Kept here**: saving a draft keeps what it quotes (picked lines, the whole mail, whether that
    was added) in the encrypted cache, by draft, and it goes when the draft is replaced, sent or
    deleted. Reopening on this computer shows the box and the pill exactly as left, a whole mail
    left out included.
  - **Read back**: Tern's own quotes are written in a fixed shape (a `<div>` line, then a
    blockquote with Tern's style, or a forward's header), so a draft saved by Tern elsewhere comes
    apart into text, picked lines and the whole mail. What was left out is not in the mail, so it
    cannot come back this way, and the pill gets a general label ("Earlier Message", "Forwarded
    Mail") as the short one is not in the mail.
  - **Anything else** (a draft written in Tuta's own app, or edited there) opens as text, as
    before; quotes kept here are not shown over it, so nothing shows twice.

## Aliases and sign-out (2026-10-31)

- **Aliases come from the user's GroupInfo** for a personal mailbox, as the SDK's own
  `get_group_id_for_mail_address` reads them. The mail group's GroupInfo carries no aliases, so the
  From row never showed. A shared mailbox (a group with no user) keeps them on its own GroupInfo.
  `TERN_LOG=1` prints how many other addresses the account has.
- **Signing out keeps the mail on this computer** by default, so signing in again opens with it
  already there. Preferences → Offline Mail → "Forget Mail on Sign Out" brings back the old
  behaviour: the database and its keyring key are deleted. The cache stays encrypted either way; the
  saved session is always deleted.

## Cache cleanup (2026-10-31)

Built to the "Cache cleanup" boards on the design canvas.

- **Mail past Keep Ready is trimmed after each folder's sync**, oldest first, with its body,
  search words and meaning. Drafts and pinned conversations stay. The folder on screen is skipped,
  since older mail you scrolled to should not vanish while you look at it. Before this, the cache
  only ever grew.
- **Trimming cuts at a page's edge.** Each page's cursor is kept (`cursors` table) on its last mail.
  A trim keeps up to a page more than Keep Ready, so it can cut where a cursor is and "load more"
  continues right after. When no cursor is near (mail cached before this change), it cuts at
  exactly Keep Ready and "load more" pages from the newest, passing pages already stored, up to
  100 pages.
- **The broom** shows at the foot of the folders once Tern takes more than "Offer Cleanup Past"
  (500 MB, 1 GB by default, 2, 5 or 10 GB, or Never). It also shows when the disk has less than
  5% free, whatever the setting, but only once Tern takes at least 100 MB, as below that cleaning
  would not help. Sizes are decimal (1 GB = 10^9 bytes), as GNOME shows them. The check runs after
  each sync round and after a cleanup.
- **Clean Up's choices:**
  - Older Mail (ticked to start): trims every folder, the open one too, and drops the text of all
    but the newest Keep Ready "opened ahead" mails. Their search words stay, so search still finds
    them; they open from the server.
  - Search by Meaning Data: only listed when there is some.
  - Language Model: only listed while search by meaning is off. When it is on, the model would
    just be downloaded again.
  - All Mail: drops every mail and fetches again. Folders, pins, draft quotes, addresses and
    rule marks stay.
- After cleaning, the database is VACUUMed and its log truncated, so the room goes back to the
  disk. The toast gives the real size difference, not the estimate. Estimates in the dialog are the
  stored text's length plus free pages, so they are approximate.
- The dialog is an AdwAlertDialog. It is narrower than the mockup, as AlertDialog sets its own
  width.

## Downloads for Linux (2026-10-31)

- **Two downloads per build, x86_64:** a Flatpak bundle for any distribution, and a tarball with
  `install.sh` for up-to-date ones. Both put Tern in the applications menu with its icon.
- **Linux Mint gets the Flatpak.** Mint 22 is Ubuntu 24.04 underneath, with GTK 4.14 and libadwaita
  1.5; Tern needs GTK 4.22 and libadwaita 1.9, so a plain binary cannot run there. The GNOME 50
  runtime brings both.
- **The tarball is built on Void.** Void has GTK 4.22 and libadwaita 1.9 but glibc 2.41, older than
  Arch's, so a binary built there runs on Void and on CachyOS or Arch; one built on Arch would not run
  on Void. Checked: all its libraries resolve on Void, and installed with `install.sh` it runs the
  window scenes on CachyOS.
- `install.sh` installs into `~/.local` (or `PREFIX`) and writes the menu entry with the program's full
  path, since `~/.local/bin` is not on every desktop's path. `--uninstall` removes it and leaves mail
  and settings.
- **The Flatpak sandbox** has network, Wayland (X11 as fallback) and the GPU only. Files are opened,
  saved and attached through portals; the keyring comes through the Secret portal (oo7 does this by
  itself), and without one Tern keeps no cache on disk, as before. Cargo fetches crates during the
  Flatpak build, which is fine for a bundle; Flathub would want them listed as sources, a later step.
- **No AppImage, .deb or .rpm**: GTK 4 and libadwaita AppImages theme badly, and Debian and Ubuntu do
  not ship the GTK Tern needs. Not built yet: aarch64, Void's musl variant, an AUR package, a Void
  template, a Flathub listing.
- **Releases**: every push to main replaces the `latest-main` pre-release; a `v*` tag makes a release.
- **CI runs on Arch** in a container, with GNOME's mutter, headless, and GTK's cairo renderer for the window
  scenes. Without a GPU, kwin and GTK's GL renderer draw nothing, so nothing is laid out; weston's
  headless backend crashes on the composer's popups; under Broadway the panes size differently. Locally `make test-ui` still prefers kwin. Clippy on stable 1.99 calls
  async_trait's generated `#[must_use]` doubled, so that lint is allowed in each Tern crate; clippy is
  off altogether for the vendored Tuta SDK.

## The default mail app (2026-11-05)

- **The menu entry says Tern takes `mailto:` links** (`MimeType=x-scheme-handler/mailto;`, `Exec=tern %u`),
  which lists it among the desktop's mail apps. GApplication's `HANDLES_OPEN` passes a link from a second
  `tern` to the one already open, so a link opens a composer in the running app. Every other argument is
  still dropped, as Tern takes none of its own.
- **Preferences → Email Links has a Use Tern button**, which sets the default through GIO
  (`mimeapps.list`, as the desktop's own settings do) and checks it took: a desktop-specific list such as
  `kde-mimeapps.list` wins over the one GIO writes. It never asks at start-up, as nothing here should nag.
- **The Flatpak only says where to choose it.** No portal sets a default, and GIO in the sandbox would write
  a `mimeapps.list` the desktop never reads. Reaching the real one would need write access to the host's
  config, more than this is worth given the sandbox has network, display and GPU only. The exported menu
  entry does list Tern in the system's settings. Run without being installed, as from `cargo run`, there is
  no menu entry to make default, and the row says so.
- **A link only fills in text.** It can come from any web page, so To, Cc, Bcc, Subject and Body are read
  and everything else ignored, `attach=` above all, which would let a page pick up a file from this
  computer. Nothing is sent until you press Send. Control characters go too; a NUL would make GTK's text
  calls panic.
- **A link arriving before sign-in waits** in the window and opens once the account is ready, so a link can
  start Tern cold.
- A link in a mail still goes through the desktop's default, as other links do: it reaches Tern's composer
  when Tern is the default, and whichever app is otherwise.

## Tern for Android (2026-11-05)

- **All behaviour is Rust.** `crates/app` owns the state, the navigation stack and what every action
  does, and publishes view records; Compose draws them and sends actions back. Wording, sorting, what Back
  does, and which screen is on top are all decided there, so the whole app is tested on the host
  against the demo account. Kotlin holds only what typing needs locally and the platform's own work.
- **One task owns the state.** Actions and the results of background work come through one channel; work
  runs in its own task and comes back as a closure over the state, dropped if the account has gone
  meanwhile. The UI thread never waits on Rust, and a view is sent only if it changed.
- **UniFFI**, the version the vendored SDK already pins, with bindings written from the compiled library
  at build time. `--crate tern_app` keeps the SDK's own UniFFI scaffolding out of them.
- **The desktop's dates still come from GLib**, and the core's from chrono, so those exist twice for now.
  See "One core" below for what else the desktop has not moved over yet.
- **Opens before it connects.** A saved account shows its encrypted cache at once with a stand-in backend
  that answers Offline; the session is resumed behind it and the real backend swapped in. So a cold start
  does not wait on the network, and Tern opens with no network at all.
- **Connection status lives in the header's second line**, mapped from the real errors (`Offline`,
  `RateLimited`, `SessionExpired`) and the desktop's activity words. Connected shows only the unread
  count. Signed out keeps saved mail readable until you sign in again, which differs from the desktop.
- **Pull to refresh shows a disc only while dragging**; on release the list springs back and the header
  says Fetching. Offered as a menu item too, for those who cannot do the gesture.
- **Labels are real now.** A mail lists its mail sets (`MailSummary.sets`), `Backend::remove_label` is new
  (ApplyLabelService with `removedLabels`), and rows show label names without colours, as Tern has
  always ignored label colours. Old cached rows show labels once a sync fetches them again.
- **Quoting by long press.** Compose does not expose a text selection, so a long press picks a whole block
  of the mail for the reply to quote, rather than lines as on the desktop. The cost is that mail text
  cannot be selected for copying.
- **There is no default-mail-app role to ask for.** `android.app.role.EMAIL` is not available and Default
  apps has no email entry; the default is whatever "Always" chose in the picker, per action and per
  set of candidates. Settings shows who answers `SENDTO mailto:`, which is how apps send mail, and offers
  the picker. `VIEW mailto:someone`, as browsers usually open, also has Contacts as a candidate, so it stays
  a separate choice made in that picker.
- **Certificates** come from the platform's directories by way of `SSL_CERT_DIR`, as `rustls-native-certs`
  does not know Android's. It takes one directory, so the engine picks the first that exists. User-added
  CAs are not trusted.
- **SQLCipher** links a vendored OpenSSL on Android, which has no system libcrypto to link.
- **Not there yet**: notifications and background sync (sync runs only while the process is alive), a
  light theme, and Move to Spam's and Clean Up's dialogs were built without a mockup.
- **Android 17 only** (minSdk 37), as asked. The `mobile` Cargo profile keeps symbols for the bindings;
  AGP strips them when packaging.

## Background checks and notifications (2026-11-05)

- **Off until asked for, in one place.** Settings has a pace (Off, 15, 30 or 60 minutes) and a Notifications
  switch. Notifications need the checks, so turning them on turns on the checks and turning off the checks
  turns them off; both rules are the engine's. Tern is a privacy-minded app and this spends battery, so
  neither starts on its own, and Android asks for the permission only when the switch is turned on.
- **WorkManager**, as the system's own scheduler: runs only with a network, survives restarts, and costs
  nothing between runs. It cannot be closer than 15 minutes, and Doze can delay it, so news can be late. A
  foreground service would be prompter and would show a permanent notification. Tuta's own push has not
  been looked into for Tern, and would be the way to instant news.
- **The check is the same round as the foreground sync**, shared as `Round`, with a lock so two never run at
  once, and rules run first so mail a rule moves is never announced. A process started only for a check
  resumes the saved session first, and retries on its next turn if it cannot.
- **What is new** is mail in the Inbox received after a mark in the cache, unread and not this account's. The
  first look only sets the mark, so turning this on does not announce the whole Inbox. While Tern is on
  screen nothing is announced; leaving the screen moves the mark past what was there.
- **One notification per conversation**, with an id from its key so news replaces what was shown, grouped with
  a count past one. Opening Tern clears them. On the lock screen they say only "New mail".
- **The demo account has mail arrive on each check**, as it has no server, so all this can be tried without
  an account.

## Publishing Android (2026-11-05)

- **As in Fob:** `-PternSplits` gives APKs per ABI and a universal one sharing a versionCode, signed only when
  CI's secrets exist; a tag must match the version and has the secrets or fails. The `profile` build type is
  for trying on a phone. They go in the one Release job with the Linux downloads, as both come from one tag
  here, and `latest-main` gets them too, unsigned until the secrets exist.
- **32-bit libraries are excluded** from every APK, or a 32-bit phone would install one and crash.

## One core (2026-11-05)

- **`tern-app` is the one core.** It holds the state, navigation and what each action does, and publishes
  views. Platforms draw them. Android does this entirely. The desktop runs the engine in its own process,
  and every screen goes through it. All the desktop keeps is drawing and what only it can do: windows, keyboard
  handling, pane widths, file dialogs and the keyring. The Rust-only `Host` trait remains to say when an account
  opens and closes, and so that the desktop's scenes can look behind the views; no mail goes through it.
- **Several composers.** The core holds any number and says which one actions are for with `ComposeFocus`. A
  phone only ever has one, and goes on hearing about just that one in `Update::Composer`; the desktop draws a
  window for each in `Update::Composers`.
- **Attachments stop at 25 MB.** The desktop used to warn past that and try anyway, which Tuta may refuse. It now
  says so and does not add the file, as on Android.
- **Beside the list, the next conversation follows.** `follow_neighbour` has the core open a conversation's
  neighbour when it leaves the list, where the two are on screen together; on a phone it goes back to the list.
  A lone draft opens as a conversation on the desktop too, and in a composer on a phone.
- **The desktop takes on the core's start-up.** A saved account opens at once from its cache, even offline. It
  used to show the sign-in page unless the server answered. An expired session still goes to the sign-in page.
- **"Core" names nothing else.** `tern-mail` is only mail's types and the `Backend` trait. `tern-html`,
  `tern-compose` and `tern-rules` are pure and do no I/O. `tern-demo` is a backend beside `tern-tuta`.
  `tern-cache` only stores; `tern-sync` decides what to fetch and when in a round.
- **One round, with events.** `tern_sync::round` does the steps and reports what it found as `Event`s, so
  it knows nothing of what is on screen or which thread that runs on. When rounds run stays with the app.
  A rate limit stops a round as an offline error does; failing rules are logged and tried again; a failure
  opening mail ahead reaches the connection status.
- **A connection status on the desktop**, because of that last point: Offline, Server busy or Signed out,
  where the activity word goes, and only with nothing under way.
- **Paths are handed down**, not read from the environment wherever needed. Android used to rewrite its own
  `HOME` and `XDG_*` to steer the crates beneath it. Only `SSL_CERT_DIR` remains, set once before anything
  reads it, as the TLS library has no other way in. The desktop's `paths()` and `meaning()` are process-wide
  because its environment never changes.
- **One log**, through the `log` crate. Tern's warnings only by default, so that the SDK cannot put anything
  about mail in the system's log on Android; `TERN_LOG` asks for more. The SDK used to install its own Android
  logger and panic if one was there, so it is patched not to. Only a real sign-in on a device goes that way:
  `android/tools/drive.py fake_login` checks it. Panics go to the log too, or Android would lose them.
- **The bridge is a feature, not a dependency.** `tern-ffi` is three lines that turn on `uniffi` in `tern-app`
  and re-export its symbols, so there is one namespace and one Kotlin package, `com.tern.engine`. Mirroring
  the views in `tern-ffi` would be about 600 lines of records and conversions to keep in step. `tern-tuta` still
  brings UniFFI in through the SDK, so CI can only check that the core does not ask for it itself.
- **A secret store's failure is an error, never "none".** `get` answers `Result<Option<_>>`, because taking a
  failed decrypt for "no key yet" would replace the key the cache needs. Secrets are also read off the core's
  task, as a keyring may wait on an unlock prompt. The desktop's store keeps its oo7 attributes, so existing sign-ins survive.
