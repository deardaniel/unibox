# Unibox

One inbox for several Gmail accounts, as a native Windows app. It is for reading and
triage: replying opens the conversation in Gmail.

## What it does

**The inbox**

- One threaded list across every account, or one account at a time, with search.
- The whole list is there at once (no "load more"), with a count of conversations and
  unread beneath it.
- Filters for unread only (`f`) and for mail a person wrote rather than a machine (`F`).
- Archive, read/unread and star are written back to Gmail. `z` undoes.
- Gmail's keyboard shortcuts: `j`/`k`, `o`, `u`, `e`, `s`, `x`, `/` and the rest. Press `?`
  in the app for the list.
- The unread count shows as a badge on the taskbar button and in the window title.

**Reading**

- A conversation opens at its latest message with the earlier ones collapsed; `;` and `:`
  expand and collapse them all, `n`/`p` step through.
- A long message reopens where you had scrolled to, also after a restart.
- Attachments preview in place. A conversation can be opened in a window of its own.
- Dark mode follows Windows.

**What it tells you about a message**

- Whether the sender is who they claim to be, from Gmail's own SPF/DKIM/DMARC verdict:
  verified, unverified or failed, with the reason in plain words.
- Whether a person or a machine wrote it, and why it thinks so.
- The real sender of mail that arrived through a Google Group.
- Calendar invitations, flights, bookings and orders as a summary card above the message.
- An Unsubscribe button where the sender supports one-click unsubscribe (RFC 8058).
- "Archive everything from this sender" in a message's menu.

**Privacy**

- Message HTML is sanitised and shown in a sandboxed frame; scripts never run.
- Remote pictures (and so tracking pixels) are shown for verified senders and blocked for
  unverified or failed ones until you allow them.
- Mail is cached in SQLite under `%LOCALAPPDATA%\Unibox`. App passwords and the API key are
  kept in the Windows Credential Manager, not in files.
- Nothing leaves the PC except IMAP to Gmail, an unsubscribe request you click, pictures a
  message loads, and what the assistant sends (below).

## The assistant

A chat panel (Ctrl+K) backed by Claude. It persists between sessions, is told what the window
is showing with each message, and works through five tools defined in `src/agent.rs`:
read-only SQL over the mail cache, show a set of conversations in the list, read a
conversation, archive / mark / star, and save a reply as a Gmail draft. It cannot send or
delete. Errors are kept in the conversation so they can be looked at later.

It is optional and needs an Anthropic API key (panel → Settings). The endpoint can be
changed there too. Mail it reads to answer a question is sent to that endpoint.

## Requirements

- 64-bit Windows 10 or 11 with the WebView2 runtime (part of Windows 11; a free download
  from Microsoft for Windows 10).
- For each Gmail account, an [app password](https://myaccount.google.com/apppasswords).
  That needs 2-Step Verification, and on Workspace the admin must allow IMAP.

The app is `unibox.exe` with `WebView2Loader.dll` beside it; it will not start without the
DLL. It is not code-signed, so Windows asks for confirmation the first time it runs.

## How it works

`unibox.exe` is a Rust program. It syncs each account over IMAP, keeps an IDLE connection
open per account so new mail shows up straight away, and falls back to a full sync every
two minutes (every fifteen while every account is being watched). It serves the interface on
`127.0.0.1:47821` and shows it in its own window (tao + wry hosting WebView2). Closing the
main window exits the app.

What a message says about itself (sender checks, person or machine, unsubscribe, cards) is
worked out once when the message is stored. Bumping `SUMMARY_VERSION` in `src/mime.rs`
recomputes it for the whole cache on the next start; `RENDER_VERSION` in `src/main.rs` does
the same for cached message pages.

## Build and install

Everything is built from WSL and cross-compiled for Windows. It expects, in the home
directory and not necessarily on `PATH` (the scripts add them):

- Rust in `~/.cargo`, with the `x86_64-pc-windows-gnullvm` target added;
- [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) in `~/.local/opt/llvm-mingw`.

```sh
./install.sh        # build the Windows .exe, install it for the current Windows user,
                    # add a Start Menu shortcut (closes a running copy first)
./build.sh windows  # just the .exe: target/x86_64-pc-windows-gnullvm/release/unibox.exe
./build.sh          # Linux build for development
PATH="$HOME/.cargo/bin:$PATH" cargo test
```

`install.sh` puts the app in `%LOCALAPPDATA%\Programs\Unibox` and leaves mail and settings
alone. The shortcut's icon is drawn with Pillow if Python has it, and skipped otherwise.

To hand the app to someone else, give them `unibox.exe` and `WebView2Loader.dll` from that
folder together. Nothing of yours is in them.

## Developing without real mail

`tests/fake_gmail.py` is a pretend Gmail IMAP server with a small inbox that exercises each
feature (a newsletter, a Google Group message, an invitation, a booking, a long read and so
on).

```sh
python3 tests/fake_gmail.py &
UNIBOX_IMAP=plain:127.0.0.1:1143 UNIBOX_UI_DIR=$PWD/ui \
  target/debug/unibox --serve --port 47921 --data /tmp/unibox-dev
```

Open `http://127.0.0.1:47921/` and add any address with any password (`wrong` is rejected).

- `UNIBOX_UI_DIR` serves the interface from disk, so HTML/CSS/JS edits need no rebuild.
- Use a port other than 47821: WSL forwards its localhost ports to Windows, where the
  installed app wants that one.
- `FAKE_ROUTINE=3000` in the fake server's environment makes a long inbox.
- `tests/fake_claude.py` is a scripted stand-in for the Claude API, for trying the assistant
  without a key: run it and set the assistant's endpoint to `http://127.0.0.1:1144`.

## Layout

| Path | What |
| --- | --- |
| `src/main.rs` | HTTP API, security checks, the window and taskbar badge |
| `src/imap.rs` | Minimal IMAP client with Gmail's `X-GM-*` extensions and IDLE |
| `src/sync.rs` | Inbox sync, conversation fill-in from All Mail, write-back of actions |
| `src/mime.rs` | Message parsing, HTML sanitising, attachments |
| `src/trust.rs` | Sender verification from Gmail's authentication results |
| `src/facts.rs` | Person or machine, unsubscribe, group senders, events and booking cards |
| `src/db.rs` | SQLite schema and queries |
| `src/secrets.rs` | Passwords and keys in the Windows Credential Manager |
| `src/agent.rs` | The assistant: model loop, tools, draft building |
| `ui/` | The interface (plain HTML/CSS/JS, embedded in the binary) |
| `tests/` | The pretend Gmail and Claude servers |
