# yagura (櫓)

> A lightweight TUI file tree, diff viewer, and change monitor for observing codebases and AI coding agent activities in real time.

When working with autonomous AI coding agents, keeping track of modified files and code changes across a project in real time can be challenging. Heavy IDEs add unnecessary overhead, while standard GUI tools lack keyboard efficiency.

**yagura** (Japanese for *watchtower*) acts as a lightweight TUI watchtower in your terminal. It gives you a bird's-eye view of your repository: watching filesystem events, showing live Git diff line counts (`+N -N ~N`), tracking file change ages (`12s`, `3m`), rendering rich previews (syntax highlighting, formatted Markdown, diffs), and letting you jump or search instantly without leaving your shell.

---

## Features

- 🌳 **Lazy-Loaded File Tree**: Instant startup even on massive projects. Displays Git diff line counts (`+N -N ~N`) and change ages (`12s`, `3m`, `2h`).
- 📄 **Rich Preview Pane**:
  - **Syntax Highlighting**: Automatic language detection and theme coloring.
  - **Formatted Markdown**: Clean rendering for headers, lists, quotes, and code blocks.
  - **Git Diff Modes**: Switch between Normal view, Inline Diff, and Side-by-Side Diff (`s`).
- 🔍 **Telescope-like Overlays**:
  - **File Finder (`Ctrl+P`)**: Instant fuzzy path matching with live preview.
  - **Grep Finder (`Ctrl+G`)**: Fast text search powered by `ripgrep`.
- ⏱️ **Watch Log (`w`)**: Real-time event log tracking all file modifications since startup.
- 🖱️ **Mouse Support**: Scroll focused panes with the wheel (5 lines per notch) and select panes with a left click.
- ✏️ **Editor Integration (`e`)**: Seamless hand-off to `$VISUAL` / `$EDITOR` (or `vi`/`helix`) at the exact line you are previewing.

---

## Installation & Usage

### Building & Installing

Requires Rust 1.88 or newer (edition 2024):

```sh
cargo install --path .
```

### Quick Start

Run `yagura` in your project root or specify a path:

```sh
yagura [PATH] [OPTIONS]
```

### Command Line Options

| Option | Description |
| --- | --- |
| `[PATH]` | Directory to inspect (defaults to current working directory). |
| `--icons nerd\|text` | Icon style. `nerd` (default) uses Nerd Font glyphs; `text` uses plain ASCII markers. |
| `--baseline auto\|head\|snapshot` | Diff baseline. `auto` (default) uses Git `HEAD` inside repositories, falling back to a session snapshot outside. `head` forces Git `HEAD`; `snapshot` forces a session snapshot. |
| `--no-ignore` | Do not respect `.gitignore` or global exclude rules. |
| `--log PATH` | Path to write `tracing` logs (or set `YAGURA_LOG`). |

Set `NO_COLOR=1` in your environment to disable theme colors.

---

## Keyboard & Navigation Guide

### Global Controls

| Key / Input | Action |
| --- | --- |
| `q` / `Ctrl+c` | Quit `yagura` |
| `Tab` | Toggle focus between Tree and Preview |
| `Esc` | Step back / Close overlay |
| `t` | Show or hide the tree (when hidden, all keys scroll the preview) |
| `s` | Cycle preview mode (`NORMAL` → `DIFF` → `SPLIT`) |
| `B` | Toggle diff baseline (`HEAD` ⇄ Session Snapshot) |
| `r` | Reload the shown file |
| `n` / `N` | Jump to the next / previous diff hunk |
| `w` | Open / close the Watch Log overlay |
| `?` | Open / close the Help overlay |
| `e` | Open the current file in your external `$EDITOR` |
| `Mouse Wheel` | Scroll the focused pane (5 lines per notch) |
| `Left Click` | Focus the clicked pane |

### Tree Pane

| Key | Action |
| --- | --- |
| `j` / `k` | Move cursor down / up |
| `h` | Collapse directory or move to parent |
| `l` | Expand directory |
| `Enter` | Open directory or send file to Preview |
| `g` / `G` | Jump to top / bottom of visible tree |
| `d` / `u` | Scroll half page down / up |
| `f` / `b` | Scroll full page down / up |
| `F` | Toggle Follow mode (auto-scroll to newest change) |
| `c` | Toggle Filter mode (show only changed paths) |
| `o` | Cycle sibling sort order (By Name ⇄ By Most Recent Change) |

### Preview Pane

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll down / up by one line |
| `g` / `G` | Jump to top / bottom |
| `d` / `u` | Scroll half page down / up |
| `f` / `b`, `Space` | Scroll full page down / up |
| `←` / `→`, `H` / `L` | Scroll left / right by column |
| `0` / `$` | Jump to far left / far right edge |

### File & Grep Finders (`Ctrl+P` / `Ctrl+G`)

Press `Ctrl+P` for File Finder or `Ctrl+G` for Grep Finder (you can switch between them directly):

| Key | Action |
| --- | --- |
| Printable chars | Type query into text box |
| `↑` / `↓`, `Ctrl+p` / `Ctrl+n` | Select previous / next match in list |
| `PgUp` / `PgDn` | Page up / down through matches |
| `Alt+j` / `Alt+k` | Scroll the side preview pane |
| `Enter` | Accept selection (expands tree to file and moves focus to Preview) |
| `Esc` / `Ctrl+c` | Close finder |

Query editing supports standard GNU Readline shortcuts (`Ctrl+a`/`e`, `Ctrl+b`/`f`, `Alt+b`/`f`, `Backspace`/`Ctrl+h`, `Ctrl+u`/`k`, etc.) and single-line pastes up to 1024 characters.

### Watch Log (`w`)

Tracks all filesystem events under root since startup:

| Key | Action |
| --- | --- |
| `j` / `k` | Move up / down through events |
| `g` / `G` | Jump to oldest / newest event |
| `Enter` | Jump to modified file in tree |
| `w` / `Esc` | Close Watch Log |

---

## Architecture & Developer Notes

### Flux Architecture

`yagura` is built as a single, pure Flux loop. All inputs and background tasks produce `Action` values. A pure `reduce` function updates `AppState` and returns `Effect` descriptors, executed asynchronously by an `EffectRunner`. Reducers never perform I/O, touch the clock, or block the UI thread.

```
crossterm thread ──actions──┐
                            ▼
                        Store loop ──effects──► EffectRunner ──actions──┐
                            │                                              │
                            ▼                                             │
                         reduce ──► AppState ──► render ──────────────────┘
```

### Core Subsystems

- **Lazy Tree & FS Model**: Directories and files are read lazily on demand in background worker tasks using generation numbers (I3) to prevent stale overwrites. The UI renders only visible rows.
- **Preview & Syntax Highlighting**: Files are truncated at 1 MiB / 2000 lines for safe rendering. Control sequences are sanitized. `syntect` parses highlighting asynchronously up to 10,000 lines or 150 ms.
- **Markdown Renderer**: Formats headings, lists, quotes, and code blocks via `pulldown-cmark` while maintaining strict 1:1 line mapping for gutter numbers and diff overlays.
- **Git & Snapshot Baselines**: Compares against Git `HEAD` (`git status`, `git cat-file`) or a memory snapshot (for non-repo directories). DiffStats (`+N -N ~N`) update dynamically.
- **Finder & Grep Workers**: Asynchronous filesystem scanning and `ripgrep` (`rg --json`) worker integration with query debouncing and generation tracking.
- **Editor Hand-off**: Suspends raw terminal mode, hands TTY over to `$VISUAL`/`$EDITOR` (passing current line number), and restores state smoothly on exit.

---

## Development

### Running Checks & Tests

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Snapshot & Benchmark Testing

- **UI Snapshots**: UI frames are verified with `insta`.
- **Performance Benchmarks**:
  ```sh
  cargo test --release --test benchmarks -- --ignored --nocapture
  ```
