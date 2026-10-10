# Unibox design system

Unibox is a tool for power users on Windows. It should feel clean, modern and frictionless,
at home on Windows 11, and above all fast. This file is the reference for every interface
change: the tokens live in `ui/style.css` (`:root`, one set per theme); the rules below say how
to use them.

## Principles

1. **Instant.** Nothing waits on an animation. The outline of what is coming is drawn before
   the data, and every action shows its result in the same frame.
2. **Keyboard first, mouse welcome.** Every control shows its key on its face (`data-key`).
   Anything the mouse can do, a key can; anything typed can be clicked.
3. **One colour, for state.** The picked colour means *on*, *focused*, *here* or *a link*. It
   is never decoration, so its presence always says something.
4. **Lines, not boxes.** Hairlines and alignment separate things. Shadows are kept for what
   floats over the window: menus, suggestions, toasts, dialogs.
5. **At home on Windows.** Segoe UI Variable, Windows-sized controls and radii, one focus ring,
   follows the system theme.
6. **Quiet when healthy, exact when not.** Normal states say little. A problem is a status dot
   and the full, unedited detail in place; it never takes over the screen.
7. **No selection means no filter.** Filters are independent toggles, off by default. Where you
   are looking (Inbox / All mail) is a scope, not a filter.

## Tokens

Use a token for every colour, font size, radius, shadow and duration, in `ui/style.css` and in
`ui/app.js` alike. A raw colour belongs only in `:root`; account colours, which come from the
database (`--c`), are the one exception.

`ui/style.css` is the source of truth for values; this document names the tokens and their
roles. Where it lists a family without values (avatars, swatches), the values live only in the
style sheet.

A custom property that only one component uses may stay local to that component's rule (the
rail's `--lead` and `--open` on `#side`); it becomes a design token once a second component
needs it.

Spacing on the 4 px grid uses `--s1`–`--s6`. Component dimensions (column widths, icon sizes,
a deliberate off-grid padding) may stay literal: snapping them would move things for no gain.

```css
:root {
  /* neutrals */
  --bg: #f4f4f3;        /* the window */
  --panel: #fff;        /* surfaces: fields, messages, dialogs, unread rows */
  --raised: #fafaf9;    /* filled fields inside a panel */
  --hover: #ececea;
  --line: #e1e0dc;      /* hairlines */
  --ink: #1b1b1b;
  --muted: #616161;     /* secondary text; 4.5:1 or better on every ground */
  --unread-bg: var(--panel);
  --read-bg: #f8f8f7;

  /* the picked colour: the only input; everything else is derived */
  --pick: #5b2ee8;
  --accent: oklch(from var(--pick) clamp(0.38, l, 0.58) c h);
  --tint: color-mix(in srgb, var(--accent) 11%, var(--panel));         /* behind what is on */
  --accent-line: color-mix(in srgb, var(--accent) 40%, var(--panel));  /* borders, keys on an on-control */
  --on-accent: #fff;
  --cursor: var(--accent);

  /* status: dots and text only, never fills behind content */
  --ok: #2e7d4f; --warn: #a86400; --danger: #c0392b;

  /* type */
  --font: "Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI", "Yu Gothic UI", "Meiryo", system-ui, sans-serif;
  --mono: "Cascadia Mono", Consolas, ui-monospace, monospace;
  --fs-key: 11px; --fs-caption: 12px; --fs-small: 13px; --fs-body: 14px; --fs-subtitle: 16px; --fs-title: 20px; --fs-mono: 12.5px;

  /* space (4 px grid), shape, size */
  --s1: 4px; --s2: 8px; --s3: 12px; --s4: 16px; --s5: 24px; --s6: 32px;
  --r-sm: 4px;   /* tokens, kbd, small chips */
  --r: 6px;      /* buttons, fields, rows, toggles */
  --r-lg: 8px;   /* menus, suggestions, messages, panels */
  --r-xl: 10px;  /* dialogs */
  --control-h: 32px;      /* buttons, toggles, fields, menu items, row actions */
  --control-h-sm: 24px;   /* controls inside a field or a line of text: scope, tokens, inline buttons */
  --rail: 50px;

  /* fixed grounds: the same in both themes */
  --paper: #fff;          /* the ground an email is drawn on */
  --on-strong: #fff;      /* text on a strong colour: account pills, avatars, the danger link warning */
  --star: #e2a400;        /* stars only */
  --scrim: rgba(0,0,0,.45);                       /* behind a dialog */
  --stage: rgba(0,0,0,.93); --stage-muted: #bbb;  /* behind an attachment shown in place */
  --spectrum: linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);  /* the hue strip */
  --badge: #c42b1c;       /* the taskbar unread badge: drawn on Windows' own chrome, so one red in both themes */
  /* --av-1 … --av-8: avatar colours; --swatch-1 … --swatch-10: the colours offered in Appearance
     (--swatch-1 is the default purple). Same in both themes; the script reads them. */
  color-scheme: light dark;

  /* depth and motion */
  --shadow-float: 0 8px 24px rgba(0,0,0,.12);
  --shadow-modal: 0 24px 64px rgba(0,0,0,.2);
  --focus: 0 0 0 2px var(--panel), 0 0 0 4px var(--accent);
  --focus-field: 0 0 0 1px var(--accent);   /* with an accent border: a field being typed in, a divider being dragged */
  --t-rail: 120ms;
  --t-busy: 300ms;   /* how long work runs before a spinner or busy dot shows */
  --t-spin: 1s; --t-pulse: 1.6s;   /* indicators that show work is running; not transitions */
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1f1f1f; --panel: #2a2a2a; --raised: #262626; --hover: #353535; --line: #3a3a3a;
    --ink: #ededed; --muted: #a8a8a8; --read-bg: #242424;
    --accent: oklch(from var(--pick) clamp(0.70, calc(l + 0.26), 0.80) calc(c * 0.68) h);
    --tint: color-mix(in srgb, var(--accent) 16%, var(--panel));  /* 11% is too faint on grey panels */
    --on-accent: #111;
    --ok: #5fbf8a; --warn: #e0a84a; --danger: #ff7b6b;
    --shadow-float: 0 8px 24px rgba(0,0,0,.5);
    --shadow-modal: 0 24px 64px rgba(0,0,0,.6);
  }
}
```

The window background in `src/main.rs` (`with_background_color`) must equal `--bg` in each
theme, so a resize never flashes a different colour around the page. Change both together.

## Type

| Role | Size / line | Weight | Use |
|---|---|---|---|
| Title | 20 / 28 | 600 | Conversation subject, dialog title |
| Subtitle | 16 / 22 | 600 | Section headings |
| Body | 14 / 20 | 400; 650 for unread | Almost everything |
| Small | 13 / 18 | 400 | Snippets, secondary lines, status details |
| Caption | 12 / 16 | 400 | Counts, footnotes, column heads |
| Mono | 12.5 / 18 | 400 | Tokens, typed queries, addresses in logs, the Doctor log |

Numbers that line up use `font-variant-numeric: tabular-nums`. Japanese renders in Yu Gothic UI
(Segoe has no Japanese glyphs); keep it in the stack so semibold Japanese matches the Latin text.

## The picked colour

- `--pick` is the only input. It is set from the Appearance dialog, kept per person, shared
  across open windows, and redrawn into the page, window, taskbar and Start Menu icons. Nothing
  else may hard-code an accent value.
- Any colour must work, including the grey swatch (`#55585e`). So an *on* state is never colour
  alone: it is `--tint` behind, `--accent` border and text, and semibold weight.
- Use the accent for: the cursor bar, focus rings, controls and tokens that are on, links,
  selected text in suggestions, and at most one primary button in a dialog. Never for
  decoration, headings or large areas.
- Account colours (`COLORS` in `src/db.rs`) mark accounts in the rail and on avatars. They sit
  beside the picked colour, never in its place, and are never used for state.

## Components

**Buttons.** Two heights only. `--control-h` (32 px) for buttons, toggles, fields, menu items
and actions in a list row; `--control-h-sm` (24 px) for controls inside a field or a line of
text (the scope chip, tokens, "Show pictures", the Doctor's inline buttons). `--r`, panel
background, hairline border; hover is `--hover`. `.quiet` has no border until hovered.
`.primary` (accent fill, `--on-accent` text) at most once per dialog, and only where the default
action is not already a key. Destructive actions are a plain button with `--danger` text.
Icon-only buttons are square at their height and carry an `aria-label`. Colour swatches are
32 px circles.

**Keys on controls.** The `data-key` badge sits after the label: `--mono` at `--fs-key`,
`--r-sm`, hairline border, `--muted`. On an *on* control it takes the control's colour and
`--accent-line` border. A key for a group goes on the group (`Shift+F` on the People /
Automated pair). Icon-only buttons have no room for a badge: their key goes in the tooltip
(`Ctrl+K` on the assistant). Account number keys show on each account when the rail opens.

**Filter toggles.** Off: a plain button. On: `--tint` background, `--accent` border and text,
semibold. People / Automated are a joined pair (`.seg`): at most one on, pressing the other
switches across, `Shift+F` steps through. Never a solid fill for *on*.

**The search field.** Magnifier, then the scope (Inbox / All mail, `g a` / `g i`) as a label
in a `--hover` chip, then the filters that are on as tokens (mono, `--tint`, `--accent`, `--r-sm`,
removable with ×), then what is typed. Typed filters (`is:unread`, `in:all`, `from:…`) turn into
the matching token or scope. Focus: accent border plus a 1 px accent ring. Suggestions float
(`--r-lg`, `--shadow-float`) with the key hints on their last line.

**The rail.** Laid out once at its open width and clipped when closed, so opening it moves
nothing: each row has a leading slot exactly as wide as the closed rail (the logo, an account's
unread pill or dot, a tool's icon), and opening only reveals the labels beside it. The unread
count stays in its coloured pill in that slot, open or closed. It opens after a 120 ms delay and
widens over 120 ms; nothing else in it animates.

**Conversation rows.** Fixed height, set on the row itself (`height: var(--row-h)`), never
taken from its content, so 140,000 rows scroll without layout work and line up at every display
scale. Unread is
`--unread-bg` and weight 650, never colour. The cursor is a 4 px `--cursor` bar the full
height of the row; selection is `--tint` plus a tick over the avatar, so it still shows when
the picked colour is grey; the open conversation is a neutral shade. The outline of rows is
drawn before their data arrives.

**Avatars.** Initials on a colour from a fixed set of avatar tokens (`--av-1`…), chosen by a
hash of the sender, text in `--on-strong`. Defined in `:root`, read by `ui/app.js`; never a
hex in script.

**Menus, suggestions, toasts.** The only floating things besides dialogs: `--r-lg`, hairline,
`--shadow-float`. They appear and go instantly; no fade. Menu items are 32 px; a check shows the
current choice. A toast confirms and offers undo (`z`); it never asks a question, and it goes
after 4 s or on the next key. A toast raised while a dialog is open shows inside that dialog,
and `z` works there too (except while typing in a field). A toast repeats the number and unit
of the control that caused it: "Archive all 120" is followed by "Archived 120 messages", never
a figure counted another way.

**Status.** A 9 px dot: `--ok` working, `--warn` needs a look, `--danger` failing, a hollow
`--muted` ring for idle, a pulsing `--accent` dot for busy. The dot sits on the first line of
its text. A failure shows its full detail inline in `--mono`, `--danger` text; it is not
collapsed, summarised or put in a banner. Group rows by subsystem (in the Doctor: Mail, Assistant,
Other assistants, Search index, Disk), each with an icon, a name and a protocol tag.

Work in progress is detail too: an account row says what it is doing in the same words the rail
uses ("Listing older mail… back to March 2019"), not a summary of it.

The one fill a status colour may take is the **caution about a message's safety** (a phishing
or spoofing warning, the link that does not go where it says): it is meant to interrupt, so it
keeps its `--danger` tint and border. Nothing else gets one.

**Dialogs.** `--r-xl`, `--panel`, `--shadow-modal`, over `--scrim`. Title at 20 px; a muted
line saying what this is, only where the title does not already say it; the content; actions on
the right of the footer, with a summary on the left only where there is a count worth
knowing (Subscriptions: senders, mail still in the inbox). `Esc` closes. The focused default
action shows the focus ring.

**Asking first.** Ask only before something that cannot be undone: removing an account,
deleting downloaded copies. Those may use the system's own question box; it is native and
expected. Everything that can be undone (archive, archive all from a sender, mark, star, move)
happens at once and offers Undo (`z`) in a toast. Never ask "are you sure?" about an undoable
action. Where an action could be made undoable cheaply, prefer that to asking. Where the natural next step is
typing, a 4 s toast is too short a window: starting a new assistant chat keeps the previous one,
and the empty new chat offers "Back to the previous chat" until its first message is sent.
Kept-for-undo state lives in memory; closing the app is a deliberate act and ends it.

## Speed

Motion is a budget, not decoration.

| Duration | For |
|---|---|
| 0 ms | Any state change: on/off, selection, cursor, unread, opening a conversation (it is already rendered), menus and suggestions appearing |
| 120 ms | The rail opening, after a 120 ms delay that confirms the pointer means it |
| 300 ms+ | Only then does a spinner or busy dot appear, if work is still running |

Indicators of running work (`--t-spin`, `--t-pulse`) are not transitions and are allowed, but
only once the 300 ms has passed.

Never animate list content, row height or layout. Prefer `content-visibility`, fixed row
heights and pre-rendered views over transitions.

## Accessibility

- Text is at least 4.5:1 against its ground (3:1 at 20 px and above); `--muted` is chosen to pass
  on `--bg`, `--panel` and `--read-bg` in both themes.
- Focus is always visible: `box-shadow: var(--focus)` (or an outline with the same values) on
  `:focus-visible`, one style for every control.
- Every interactive thing is a real `<button>`, `<a>` or `<input>`; toggles use `aria-pressed`,
  joined pairs a `role="group"` with a label, icon-only buttons an `aria-label`.
- States that must be told apart differ in more than hue (weight, border, a mark).

## Checklist for an interface change

- Only tokens outside `:root`, in CSS and script; no new raw colours, font sizes or durations.
- Looks right in light and dark, and with at least the default purple, the amber and the grey
  swatches.
- Every control shows its key, and the `?` dialog agrees (see CLAUDE.md).
- No new motion beyond the budget above.
- Tried at full size (about 140,000 messages) if it touches the list.
