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

Use a token for every colour, size, radius, shadow and duration. A raw hex value belongs only in
`:root`. The one exception is account colours, which come from the database (`--c`).

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
  --fs-caption: 12px; --fs-small: 13px; --fs-body: 14px; --fs-subtitle: 16px; --fs-title: 20px; --fs-mono: 12.5px;

  /* space (4 px grid), shape, size */
  --s1: 4px; --s2: 8px; --s3: 12px; --s4: 16px; --s5: 24px; --s6: 32px;
  --r-sm: 4px;   /* tokens, kbd, small chips */
  --r: 6px;      /* buttons, fields, rows, toggles */
  --r-lg: 8px;   /* menus, suggestions, messages, panels */
  --r-xl: 10px;  /* dialogs */
  --control-h: 32px;
  --rail: 50px;

  /* depth and motion */
  --shadow-float: 0 8px 24px rgba(0,0,0,.12);
  --shadow-modal: 0 24px 64px rgba(0,0,0,.2);
  --focus: 0 0 0 2px var(--panel), 0 0 0 4px var(--accent);
  --t-menu: 80ms; --t-rail: 120ms;
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

**Buttons.** 32 px high, `--r`, panel background, hairline border; hover is `--hover`.
`.quiet` has no border until hovered. `.primary` (accent fill, `--on-accent` text) at most once
per dialog, and only where the default action is not already a key. Destructive actions are a
plain button with `--danger` text. Icon-only buttons are 32 × 32 and carry an `aria-label`.

**Keys on controls.** The `data-key` badge sits after the label: mono 11 px, `--r-sm`, hairline
border, `--muted`. On an *on* control it takes the control's colour and `--accent-line` border.

**Filter toggles.** Off: a plain button. On: `--tint` background, `--accent` border and text,
semibold. People / Automated are a joined pair (`.seg`): at most one on, pressing the other
switches across, `Shift+F` steps through. Never a solid fill for *on*.

**The search field.** Magnifier, then the scope (Inbox / All mail, `g a` / `g i`) as a label
in a `--hover` chip, then the filters that are on as tokens (mono, `--tint`, `--accent`, `--r-sm`,
removable with ×), then what is typed. Typed filters (`is:unread`, `in:all`, `from:…`) turn into
the matching token or scope. Focus: accent border plus a 1 px accent ring. Suggestions float
(`--r-lg`, `--shadow-float`) with the key hints on their last line.

**Conversation rows.** Fixed height, so 140,000 rows scroll without layout work. Unread is
`--unread-bg` and weight 650, never colour. The cursor is a 4 px `--cursor` bar the full
height of the row; selection is `--tint`; the open conversation is a neutral shade. The outline
of rows is drawn before their data arrives.

**Menus, suggestions, toasts.** The only floating things besides dialogs: `--r-lg`, hairline,
`--shadow-float`. Menu items are 32 px; a check shows the current choice. A toast confirms and
offers undo (`z`); it never asks a question, and it goes after 4 s or on the next key.

**Status.** A 9 px dot: `--ok` working, `--warn` needs a look, `--danger` failing, a hollow
`--muted` ring for idle, a pulsing `--accent` dot for busy. The dot sits on the first line of
its text. A failure shows its full detail inline in `--mono`, `--danger` text; it is not
collapsed, summarised or put in a banner. Group rows by subsystem (in the Doctor: Mail, Assistant,
Other assistants, Search index, Disk), each with an icon, a name and a protocol tag.

**Dialogs.** `--r-xl`, `--panel`, `--shadow-modal`, over a dimmed window. Title at 20 px; a
muted line saying what this is; the content; a footer with a summary on the left and actions on
the right. `Esc` closes. The focused default action shows the focus ring.

## Speed

Motion is a budget, not decoration.

| Duration | For |
|---|---|
| 0 ms | Any state change: on/off, selection, cursor, unread, opening a conversation (it is already rendered) |
| 80 ms | Menus and suggestions appearing |
| 120 ms | The rail opening, after a 120 ms delay that confirms the pointer means it |
| 300 ms+ | Only then does a spinner appear, if work is still running |

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

- Only tokens outside `:root`; no new raw colours, sizes or durations.
- Looks right in light and dark, and with at least the default purple, the amber and the grey
  swatches.
- Every control shows its key, and the `?` dialog agrees (see CLAUDE.md).
- No new motion beyond the budget above.
- Tried at full size (about 140,000 messages) if it touches the list.
