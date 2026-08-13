# Would ratatui, the Elm Architecture, or tui-realm benefit git-switch?

Research note, August 2026. **Recommendation: no to ratatui and tui-realm; a partial, optional yes
to the Elm Architecture as a pattern, and even that is not worth a rewrite today.**

There is no existing convention in this repo for research notes — `docs/` held only `adr/` and
`agents/` before this file (`docs/adr/`, `docs/agents/`) — so this lives at
`docs/research/ratatui-tea-and-tui-realm.md`, matching the ADRs' style: no frontmatter, one `#`
title, `##` sections.

## What git-switch actually is today

### Terminal machinery

Five runtime dependencies, all direct (`Cargo.toml:12-17`):

| Crate | Requirement | Resolved | Role |
| --- | --- | --- | --- |
| `console` | `0.16` | 0.16.4 (`Cargo.lock`) | `Term`, `Key`, styling, `measure_text_width`, `clear_last_lines` |
| `crossterm` | `0.29` | 0.29.0 (`Cargo.lock`) | raw mode + key event parsing only |
| `ctrlc` | `3` | 3.5.2 | signal handling |
| `indicatif` | `0.18` | 0.18.6 | fetch spinner |
| `thiserror` | `2` | 2.0.20 | error enum |

Dev-dependencies are `portable-pty` and `tempfile` (`Cargo.toml:19-21`).

There is no full-screen machinery anywhere: no alternate screen, no double buffer, no widget tree.
Rendering is `eprint!` of pre-formatted strings, one line at a time, with an explicit `\r\n` because
raw mode disables newline translation (`src/app.rs:1249-1254`). Redraw is "count the visual rows I
printed, then `Term::clear_last_lines(n)` and print again" (`src/app.rs:822`, `src/app.rs:893-894`,
`src/app.rs:1298-1320`). Everything goes to **stderr**, because stdout is reserved for the `cd`
handoff to the shell wrapper (`src/app.rs:1227-1239`).

`crossterm` is already present but used for exactly two things: `enable_raw_mode`/`disable_raw_mode`
behind an RAII guard, and `event::read()` translated into `console::Key`
(`src/app.rs:75-153`). The comment there records why: `console::read_key` re-arms raw mode on every
keystroke and was fragile around split escape sequences (`src/app.rs:72-74`).

### How the pickers are structured

Two pickers, both in `src/app.rs`:

- `pick` — single-select with fuzzy filter, sections, sticky headings, scrolling
  (`src/app.rs:803-896`).
- `multi_select` — checkbox list with select-all/none (`src/app.rs:1258-1329`).

They are called from four sites: branch selection (`src/app.rs:615-635`), the stale-branch cleanup
prompt (`src/app.rs:1078-1083`), worktree removal (`src/app/wt.rs:231`) and worktree selection
(`src/app/wt.rs:368`).

State is **local variables in the loop** — `filter`, `cursor`, `view`, `drawn`
(`src/app.rs:812-822`); `selected`, `cursor`, `drawn` (`src/app.rs:1264-1302`). There is no `Model`
struct and no `Message` enum. What *does* exist is a pure state-derivation function:
`build_view(sections, filter, opts) -> View` (`src/app.rs:738-779`), plus pure helpers
`fuzzy_match`, `cursor_selection`, `selectable_position`, `format_row` (`src/app.rs:722-736`,
`781-797`, `976-1007`).

The input seam is the `KeySource` trait (`src/app.rs:33-35`), whose doc comment states its purpose
outright: "Abstracting input behind a trait lets the event loops be driven by a scripted sequence in
tests". The real implementation is `TermKeys` (`src/app.rs:39-51`); the test implementation is
`ScriptedKeys`, which yields `Escape` once exhausted so an under-specified script bails rather than
hanging (`src/app.rs:1336-1350`). Both pickers take `keys: impl KeySource` **by value** so raw mode
is released when the picker returns — otherwise a caller's later `\n` would staircase
(`src/app.rs:799-808`, `src/app.rs:1256-1263`). This is the "let the pickers own the key source"
commit (2f22eeb).

Presentation strings live in `src/app/reporting.rs`, whose module doc says: "Nothing here runs a git
process or writes to a stream — every function takes values and returns the lines its caller prints
— so the wording is asserted against values rather than by running the binary"
(`src/app/reporting.rs:11-13`). That's commit 2ea25d6.

### How it is tested

Three layers, all already in place:

1. **Scripted-key unit tests** over the real event loops — filter-then-enter, arrow wrap-around,
   create-from-filter, heading skipping, escape semantics, multi-select toggling
   (`src/app.rs:1393-1502`). These drive `pick`/`multi_select` end-to-end with no terminal.
2. **Pure-value tests** of labels, alignment, markers, quoting (`src/app.rs:1526-1707`).
3. **One real-PTY integration test** for the property that only a terminal can show — that every
   newline written to a tty is a CRLF, so deletion outcomes don't staircase after the picker
   (`tests/integration.rs:1878-1957`). It drives the picker over `portable-pty`, polling for the
   drawn rows before sending each key.

`tests/integration.rs` is 1957 lines; `src/app.rs` is 1708.

### Size and dependency posture

`CLAUDE.md` says "Keep dependencies minimal". The release profile backs that up:
`opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`
(`Cargo.toml:26-32`).

Measured on this checkout with `cargo tree -e normal --prefix none | sort -u`:

| Tree | Unique normal-dependency crates (incl. root) |
| --- | --- |
| git-switch as it stands | **41** |
| a hello-world binary depending only on `ratatui = "0.30"` | **70** |
| a hello-world binary depending only on `tuirealm = "4"` | **74** |

By lockfile package count (`grep -c '^\[\[package\]\]'`, which also resolves optional deps):
git-switch 109 (including dev-deps), bare-ratatui 182, bare-tuirealm 103.

Either way, adopting ratatui roughly doubles the runtime dependency graph of a tool whose stated
posture is minimalism. For reference, `cargo build --release` on this checkout produces a
555,744-byte binary (macOS arm64, with the profile above).

### Constraining ADRs

- [ADR 0001 — warned means forceable](../adr/0001-warned-means-forceable.md): "In a picker, the row
  markers are that warning… so ticking a marked row forces." The *rendering* of a row is
  load-bearing for a destructive operation's authorisation. Any renderer swap must preserve exactly
  which marker is drawn on which row.
- [ADR 0002 — staleness is anchored to the default branch](../adr/0002-staleness-is-anchored-to-the-default-branch.md):
  about git semantics, not UI. Not a constraint here.

Neither ADR forbids a TUI framework, but ADR 0001 raises the bar on any change to row rendering, and
`CONTEXT.md` fixes the vocabulary (*Marker*, *Risk*, *License*) that such a change would have to keep
intact.

## ratatui

**What it is.** "A crate for cooking up terminal user interfaces in Rust. It is a lightweight
library that provides a set of widgets and utilities to build complex Rust TUIs"
(<https://docs.rs/ratatui/latest/ratatui/>). The site's pitch is "fast, lightweight, and rich
terminal user interfaces… Sub-millisecond rendering with zero-cost abstractions and immediate-mode
rendering" (<https://ratatui.rs/>).

**What it does not give you.** The crate docs are explicit: "Ratatui does not include any input
handling. Instead event handling can be implemented by calling backend library methods directly"
(<https://docs.rs/ratatui/latest/ratatui/>). No event loop, no application architecture, no state
management. Those are exactly the pieces git-switch already has, working, and tested.

**Version and MSRV.** 0.30.2, published 2026-06-19, `rust-version = 1.88.0`
(<https://crates.io/api/v1/crates/ratatui>). Note git-switch is `edition = "2024"`
(`Cargo.toml:5`), which already implies ≥1.85, so 1.88 is a small bump, not a blocker.

**Backends.** Crossterm (default), Termion, Termina, Termwiz
(<https://docs.rs/ratatui/latest/ratatui/>). Since 0.30.0 the crate is a modular workspace
(`ratatui-core`, `ratatui-widgets`, `ratatui-crossterm`, …) "to improve compilation times and
dependency management" (<https://docs.rs/ratatui/latest/ratatui/>).

**Maintenance.** Very healthy. 22,212 stars, not archived, 270 pages of contributors at one per
page (so ~270 contributors), last push 2026-08-10; the most recent commits are Dependabot bumps
(<https://api.github.com/repos/ratatui/ratatui>,
<https://api.github.com/repos/ratatui/ratatui/commits>). Coordinated releases across the workspace
crates on 2026-06-19 (<https://api.github.com/repos/ratatui/ratatui/releases>). 43.6M downloads
(<https://crates.io/api/v1/crates/ratatui>).

**The inline question.** ratatui does *not* force alternate-screen. `Viewport::Inline(u16)` "lets
the UI appear within a larger command-line flow instead of taking over the entire terminal. The
viewport spans the full terminal width and its top-left corner is anchored to column 0 of the
current cursor row when the terminal is created" — with the caveat that "if the cursor is near the
bottom of the screen, this may scroll the terminal so the viewport remains fully visible", and the
height is clamped to the terminal height
(<https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html>). So the inline-picker shape *is*
expressible. But note the fixed-height requirement: git-switch's pickers currently size themselves
from the filtered row count and only clamp against the terminal
(`src/app.rs:930-944`, `src/app.rs:1280-1287`), so an `Inline` viewport would mean choosing a height
up front and living with it, or recreating the terminal on every filter keystroke.

## The Elm Architecture, as ratatui.rs describes it

**It is a pattern, not a library.** Nothing on the page requires you to take a dependency for it.
The page prescribes: a `Model` struct holding all app state; a `Message` enum of everything the app
can be told; an `update(model, message)` function; and a `view(model)` pure render. The loop is
"Listen for input → Map to Message → Call `update()` → Call `view()` → Render"
(<https://ratatui.rs/concepts/application-patterns/the-elm-architecture/>). That loop shape is
implementable over `console` + `crossterm` today; ratatui only supplies the final "Render" step.

**Stated trade-offs on that page.** It concedes that strict immutability is negotiable in Rust
("Rust developers can use mutable references when beneficial"); that with immediate-mode rendering
"the `view` function is only aware of the area available to draw in at render time", with
workarounds that "introduce trade-offs like frame delays"; and that `StatefulWidget`s force `&mut
Model` into the view, breaking the purity the pattern is sold on
(<https://ratatui.rs/concepts/application-patterns/the-elm-architecture/>).

**It is not presented as *the* recommended pattern.** It is one of three sibling pages under
`/concepts/application-patterns/` — The Elm Architecture, Component Architecture, Flux Architecture
— and the index frames them as "several patterns one can use for their application", with no
endorsement of one over the others
(<https://ratatui.rs/concepts/application-patterns/>). The TEA page itself points at tui-realm as an
implementation of it.

## tui-realm

**What it is.** "A ratatui framework to build stateful applications with a React/Elm inspired
approach" (<https://api.github.com/repos/veeso/tui-realm>). The crate docs describe five pieces:
`MockComponent`/`Component` (reusable elements with properties and states); `View` (mounting,
unmounting, focus, event routing); `Application` (the engine); a `Msg`/`Event` system with an
Elm-inspired update; and `EventListener` + `PollStrategy` for input
(<https://docs.rs/tuirealm/latest/tuirealm/>). Over bare ratatui it adds focus and state
management, event routing, subscriptions, and a component standard library
(`tui-realm-stdlib`) (<https://github.com/veeso/tui-realm>).

**Version and MSRV.** 4.1.0, published 2026-05-02, `rust-version = 1.88`. 4.0.0 shipped 2026-04-18
and the changelog entry for it describes "numerous breaking API changes"
(<https://crates.io/api/v1/crates/tuirealm>,
<https://api.github.com/repos/veeso/tui-realm/commits>).

**Dependencies.** Non-optional: `bitflags 2`, `dyn-clone 1`, `lazy-regex 3`, `ratatui 0.30`,
`thiserror 2`. Optional: `async-trait`, `crossterm 0.29`, `futures-util`, `serde`, `termion`,
`termwiz`, `tokio`, `tokio-util`, `tuirealm_derive`
(<https://crates.io/api/v1/crates/tuirealm/4.1.0/dependencies>). Default features are `derive`,
`serialize` and `crossterm` (<https://docs.rs/tuirealm/latest/tuirealm/>) — so a default install
drags in serde and a proc-macro crate.

**Maintenance and cadence.** Not archived; last push 2026-07-29. Releases: v2.2.0 (2025-05-15),
v3.0.0 (2025-05-21), v3.0.1, v3.1.0 (2025-08-26), v3.2.0 (2025-11-10), v3.3.0 (2025-12-20), v4.0.0
(2026-04-18), v4.1.0 (2026-05-02) — roughly quarterly, with two majors in a year
(<https://api.github.com/repos/veeso/tui-realm/releases>). Activity is bursty: 361 commits in the
last 52 weeks spread over only 21 active weeks
(<https://api.github.com/repos/veeso/tui-realm/stats/participation>). Since 4.1.0 in May, the
commits are housekeeping — a `just` runner, LICENSE placement, removing a Codeberg mirror
(<https://api.github.com/repos/veeso/tui-realm/commits>). 982 stars; 220,789 downloads all-time
against ratatui's 43.6M (<https://crates.io/api/v1/crates/tuirealm>). It is effectively a
two-person project: the top two contributors have 343 and 150 commits, the third has 6
(<https://api.github.com/repos/veeso/tui-realm/contributors>).

**How it is tested.** Dev-dependencies are `insta`, `pretty_assertions`, `tempfile`, `tokio`,
`toml` (<https://crates.io/api/v1/crates/tuirealm/4.1.0/dependencies>) — i.e. snapshot testing of
rendered output.

## Fit

### What each option would actually buy git-switch

**ratatui.** The renderer. git-switch's hand-rolled renderer is genuinely the fiddliest code in the
project: visual-row counting for wrapped lines (`src/app.rs:1241-1247`), a reserved trailing line so
a full-height draw doesn't scroll the prompt out of `clear_last_lines`' reach
(`src/app.rs:925-930`), sticky-heading arithmetic (`src/app.rs:934-964`), and the CRLF rule
(`src/app.rs:1249-1254`). A double-buffered diffing renderer removes all of that. That is the real
argument in favour, and it is not nothing.

Against it: the renderer is ~150 lines, already written, already debugged, and already has a
regression test pinning its nastiest failure mode (`tests/integration.rs:1878-1957`). The cost is
~30 extra crates in the runtime graph against a `CLAUDE.md` rule that says keep them minimal, a
release profile explicitly tuned for size (`Cargo.toml:26-32`), and a rewrite of the exact rows
ADR 0001 makes authorisation-bearing. `Viewport::Inline` needs a height chosen at terminal-creation
time (<https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html>), which fights a filter-driven
list whose height changes on every keystroke. And ratatui by its own admission hands back nothing
for input or the loop (<https://docs.rs/ratatui/latest/ratatui/>) — so the pieces git-switch would
keep are the pieces it already likes.

**TEA.** Testability is the usual argument, and here it is already banked. The `KeySource` seam
(`src/app.rs:33-35`) plus `ScriptedKeys` (`src/app.rs:1346-1350`) let the unit tests drive the real
event loops from a key script and assert on the returned `Selection` — see
`type_to_filter_then_enter_selects_match` (`src/app.rs:1393-1400`) and the multi-select cases
(`src/app.rs:1466-1502`). TEA's `update()` would give a slightly sharper seam (assert on `Model`
rather than on the loop's return value), but the thing TEA is normally adopted *for* — testing
interaction without a terminal — is already true here.

What TEA would genuinely tidy: the `pick` loop currently mixes navigation, filtering, and a
`preserved`/`filter_changed` dance to keep the cursor on the same item across a filter change
(`src/app.rs:824-895`). A `Message` enum and an `update` would make that a data transformation
rather than a sequence of mutations. But it is one loop, in one file, ~70 lines. Adopting a whole
architecture for it is exactly the ceremony `CLAUDE.md` warns against ("Keep things simple. Channel
'yagni' energy").

**tui-realm.** Everything it adds — focus management across mounted components, subscriptions, a
component standard library, event routing between components
(<https://docs.rs/tuirealm/latest/tuirealm/>) — presupposes multiple simultaneously-visible,
independently-focusable components. git-switch has one list on screen at a time and no concept of
focus. It would also inherit ratatui's dependency cost *plus* tui-realm's own, a serde default
feature, an MSRV of 1.88, a two-majors-a-year breaking cadence, and a bus factor of two
(<https://api.github.com/repos/veeso/tui-realm/releases>,
<https://api.github.com/repos/veeso/tui-realm/contributors>). This is the clearest no of the three.

### Recommendation

1. **Do not adopt tui-realm.** Wrong shape (component/focus framework for a single-list prompt),
   worst dependency and churn profile of the three.
2. **Do not adopt ratatui now.** It solves a real problem — the hand-rolled inline renderer — but
   that problem is already solved, tested, and small, while the cost lands squarely on the project's
   two stated constraints (minimal dependencies, size-tuned release profile) and on rows that ADR
   0001 makes security-relevant.
3. **TEA is available for free, and worth taking only in the small.** It is a pattern, not a
   dependency (<https://ratatui.rs/concepts/application-patterns/the-elm-architecture/>), and
   ratatui.rs itself presents it as one of three options rather than a recommendation
   (<https://ratatui.rs/concepts/application-patterns/>). If `pick`'s loop
   (`src/app.rs:824-895`) ever gets harder to reason about, refactoring *that one loop* into a
   `Message` enum plus an `update` is a contained, dependency-free improvement. Doing it project-wide
   today buys nothing the `KeySource` seam does not already provide.

### What would flip the answer

Any of these would make ratatui (and TEA alongside it) the right call:

- **A second pane.** A preview of the selected branch's log, or a side-by-side worktree detail
  panel — anything requiring layout, focus, or two regions updating independently. That is where a
  widget library and (for focus) tui-realm start paying for themselves.
- **Going full-screen.** If the picker stops printing above the prompt and takes over the terminal,
  the inline-viewport friction disappears and the hand-rolled `clear_last_lines` redraw loses its
  reason to exist.
- **Renderer bugs recurring.** The staircase bug already cost a PTY regression test
  (`tests/integration.rs:1878-1957`). If wrapping, sticky headings, or scroll arithmetic keep
  producing defects, the ~30-crate cost starts looking cheap against the maintenance.
- **Rich cell content.** Per-cell styling, unicode-width edge cases beyond what
  `measure_text_width` handles (`src/app.rs:1241-1247`), mouse support, or scrollbars.
- **The dependency rule relaxing.** If `CLAUDE.md`'s "Keep dependencies minimal" and the
  size-tuned release profile (`Cargo.toml:26-32`) stop being priorities, the main argument against
  ratatui goes with them.

None of these is true today.
