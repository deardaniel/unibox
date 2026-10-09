# Unibox

One inbox for several Gmail accounts, on Windows. Reading only for now: replies open the
conversation in Gmail.

- Unified, threaded inbox across accounts, with search, attachments and dark mode.
- Gmail's keyboard shortcuts (`j`/`k`, `o`, `u`, `e`, `s`, `x`, `/` … press `?` in the app).
- Archive, read/unread and star are written back to Gmail.
- Mail is cached in SQLite under `%LOCALAPPDATA%\Unibox`; app passwords are kept in the
  Windows Credential Manager.

## How it works

`unibox.exe` is a Rust program that syncs each account over IMAP and serves the UI on
`127.0.0.1:47821`. It shows that UI in a chromeless Edge window with its own profile and
exits a few seconds after the last window is closed. Sync runs every two minutes while it is open.

Gmail accounts are added in the app with an [app password](https://myaccount.google.com/apppasswords)
(needs 2-Step Verification; Workspace admins must allow IMAP).

## Build and install

Everything is built from WSL. Rust (`~/.cargo`) and llvm-mingw (`~/.local/opt/llvm-mingw`)
are installed in the home directory and are not on `PATH`; the scripts add them.

```sh
./install.sh        # build the Windows .exe, install it, add a Start Menu shortcut
./build.sh          # Linux build for development
PATH="$HOME/.cargo/bin:$PATH" cargo test
```

## Developing without real mail

```sh
python3 tests/fake_gmail.py &
UNIBOX_IMAP=plain:127.0.0.1:1143 UNIBOX_UI_DIR=$PWD/ui \
  target/debug/unibox --serve --port 47921 --data /tmp/unibox-dev
```

Open `http://127.0.0.1:47921/` and add any address with any password (`wrong` is rejected).
`UNIBOX_UI_DIR` serves the UI from disk so HTML/CSS/JS edits need no rebuild. Use a port
other than 47821 here: WSL forwards its localhost ports to Windows, where the installed
app wants that one.

## Layout

| Path | What |
| --- | --- |
| `src/imap.rs` | Minimal IMAP client with Gmail's `X-GM-*` extensions |
| `src/sync.rs` | Inbox sync, conversation fill-in from All Mail, write-back of actions |
| `src/mime.rs` | Message parsing, HTML sanitising, attachments |
| `src/db.rs` | SQLite schema and queries |
| `src/main.rs` | HTTP API, security checks, window launch |
| `ui/` | The interface (plain HTML/CSS/JS, embedded in the binary) |
