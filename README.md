# dziber

Dziber provides XMPP, OMEMO, JMAP, IMAP/SMTP, and ManageSieve clients, with an optional Iced desktop application.

## Features

- `desktop` (default): the complete native application.
- `mail`: reusable mail models and JMAP, IMAP/SMTP, and ManageSieve clients without desktop dependencies.
- `calendar`: CalDAV/JMAP calendars, events, tasks, calendar management, and iCalendar import/export.
- `omemo`: the OMEMO module, account and session storage traits, and legacy Signal-compatible encryption.
- `web`: the Rust/WASM browser XMPP client with OMEMO and WebRTC audio calls.

OMEMO is part of this crate as `dziber::omemo`, not a separate package. The browser client runs entirely in the browser; no backend XMPP worker is required.

Freenit uses the sibling checkout until Dziber is published:

```toml
dziber = { path = "../dziber", default-features = false }
```

Enable `dziber/mail` for server-side mail pages or `dziber/web` when building the browser client. The Freenit `scripts/build-chat.sh` script builds the WASM bundle and its small JavaScript bootstrap.
