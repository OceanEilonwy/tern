# Testing Tern

```sh
make test         # unit tests, plus the window checks on a headless display (kwin_wayland or broadway)
cargo test -p tuta-sdk --features net --lib
TERN_TEST_EMAIL=you@tuta.com TERN_TEST_PASSWORD=… cargo test -p tern-tuta -- --ignored   # live round trip
cargo test -p tern semantic -- --ignored    # downloads the search model and finds mail by meaning
```

The live tests send you a mail with a PDF attached, find it in Sent, and compare the attachment bytes;
open mail in a batch and one by one and compare; and move your newest Inbox mail to Archive and back
(and add your first label to it, if you have labels).

