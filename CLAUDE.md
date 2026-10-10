# Working on Unibox

Build, test and install with `./build.sh`, `PATH="$HOME/.cargo/bin:$PATH" cargo test` and
`./install.sh` (see README). Commit each logical change separately and push before installing.

## Keep everything in step

A feature is not finished until every place that describes or exposes it agrees. When adding,
changing or removing one, go through this list:

- **Translations** (`ui/i18n.js`): every new interface string goes through `tx()` / `txn()`,
  written in American English, with a `ja-JP` entry (and an `en-GB` one only where the
  spelling differs). Text the server sends to the page needs an entry or a pattern in
  `txServer()`. Remove entries for strings that are gone.
- **Keyboard shortcuts**: the `?` dialog in `ui/index.html`, the tooltip of the control, and the
  key shown on the control itself (`data-key`; keep it in step when the label changes).
- **The assistant and MCP tools** (`src/agent.rs`, `src/mcp.rs`): the `mail` view's columns
  in `Db::agent_query`, the tool descriptions and action list, and the instructions. If the
  list can filter or act on something, the tools should be able to as well. What the page
  tells the assistant about the screen (`list_is_showing` in `ui/app.js`) too.
- **The connection doctor** (`src/doctor.rs`): anything new that talks to the outside world
  records what it did with `doctor::note` (commands, sizes, timings; never mail content or
  secrets), and gets a row in the window if it is a connection that can be up or down.
- **README.md**: the feature list, how it works, and the source layout table.
- **The fake servers** (`tests/fake_gmail.py`, `tests/fake_claude.py`): mail or behaviour that
  exercises the feature, so it can be tried without real mail.
- **Tests**: `cargo test` covers the Rust side; add a test for new parsing, queries or protocol.
- **Version constants** when stored or cached data changes shape: `SUMMARY_VERSION`
  (`src/mime.rs`), `RENDER_VERSION` (`src/main.rs`), `ALL_MAIL_LISTING` and `SEARCH_INDEX`
  (`src/db.rs`).

## Things that bite

- The page's translation function is `tx`, not `t`: `t` is a conversation or a trust record
  all over `app.js`.
- The real mailbox is about 140,000 messages. Try list and query changes at that size
  (`FAKE_ARCHIVE=60000` for the fake server).
- Other assistants start `unibox-mcp.exe`, a copy the installer keeps beside `unibox.exe`.
  A client only learns of changed tools when it reconnects (restart Claude Desktop).
