# yagura

A TUI file tree watcher for observing what a coding agent is doing. `yagura`
shows a static, lazily loaded tree of a directory on the left, leaves room for a
preview on the right, and puts the mode, the position in the tree and anything
that needs saying on the status bar.

This is milestone M0 (program skeleton, terminal lifecycle, Flux loop, layout),
M1 (the static file tree), M2 (the preview content, with syntax highlighting),
M3 (the git baseline and the diff), M4 (real-time updates: watching, batches,
refreshed statistics, the head check, ages and the `F`/`c`/`o` keys), M5 (the
snapshot baseline, for watching a directory that is not a repository), M6
(the file finder overlay, `Ctrl+P`), M7 (the grep finder, `Ctrl+G`) and M8 (the
watch log, `w`, the help overlay, `?`, and the editor launch, `e`). The markdown
rendering is not implemented yet.

## Status

| Milestone | What it covers | State |
| --- | --- | --- |
| M0 | CLI, terminal lifecycle, Flux loop, theme, layout, store loop | done |
| M1 | `core/fsmodel`, lazy tree, sorting, scrolling, icons, status bar, keymap | done |
| M2 | `core/preview`, lazy document loading, sanitized body, line numbers, scrolling, syntax highlighting | done |
| M3 | git baseline, diff engine, InlineDiff / SideBySide, `M/A/D` in the tree, `+N -N ~N` counts, hunk keys | done |
| M4 | filesystem watching, batched updates, statistics refresh, `.git` head check, ages and freshness, follow / filter / order | done |
| M5 | snapshot baseline, startup scan with its budgets, `auto` fallback outside a repository, the `B` swap | done |
| M6 | file finder overlay: query box, parallel walk, fuzzy match, preview beside the list, the jump into the tree | done |
| M7 | grep finder overlay: `rg --json`, debounce and generations, hits as `path:line: text`, the lit line in the preview | done |
| M8 | watch log overlay (`w`), help overlay (`?`), editor launch (`e`) | done |
| M9 | edge cases, performance and benchmarks, README, CI, release | not started |

## Build

```sh
cargo build --release
cargo install --path .
```

Rust 1.88 or newer, edition 2024.

## Usage

```sh
yagura [PATH] [--icons text|nerd] [--baseline auto|head|snapshot] [--no-ignore] [--log PATH]
```

| Option | Meaning |
| --- | --- |
| `[PATH]` | The directory to show. Defaults to the working directory. |
| `--icons text` | ASCII icons: `d` for a directory, `f` for a file, `l` for a symlink. |
| `--icons nerd` | Nerd Font glyphs. |
| `--baseline auto\|head\|snapshot` | Which baseline to compare against. `auto` picks the git `HEAD` inside a repository and the start of the session outside of one, and `head` outside a repository is an argument error. Without `git` installed anywhere, the baseline says `git not found` and the session snapshot stands in. |
| `--no-ignore` | Ignore `.gitignore`, the global excludes and the user ignores. |
| `--log PATH` | Write a `tracing` log to a file. Also read from `YAGURA_LOG`. Nothing is logged without it. |

`NO_COLOR` in the environment turns the theme off.

## Keys

Everywhere: `q` and `Ctrl+c` quit, `Tab` moves the focus, `Esc` steps back,
`t` shows or hides the tree, `s` cycles the preview mode, `B` switches the
baseline between `HEAD` and the start of the session (inside a repository),
`r` reads the shown file again, `n` / `N` jump to the next or the previous
hunk of the diff, `w` opens the watch log, `?` the help overlay and `e` hands
the file to an editor.

In the tree:

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up |
| `h` | Collapse, or move to the parent |
| `l` | Expand |
| `Enter` | Open a directory, or hand a file to the preview |
| `g` / `G` | First / last row |
| `d` / `u` | Half a screen down / up |
| `f` / `b` | A screen down / up |
| `F` | Follow the newest change, then any change after it |
| `c` | Show only the paths that changed |
| `o` | Order the siblings by name, or by the most recent change |

In the preview:

| Key | Action |
| --- | --- |
| `j` / `k` | A line down / up |
| `g` / `G` | The top / the bottom |
| `d` / `u` | Half a screen down / up |
| `f` / `b`, `Space` | A screen down / up |
| `←` / `→`, `H` / `L` | A column left / right |
| `0` / `$` | The left / the right edge |

The full table lives in `src/input/keymap.rs`, which is also what the help
overlay is built from.

In the finder overlay, opened with `Ctrl+p` (the file finder) or `Ctrl+g`
(the grep finder; the two switch between each other):

| Key | Action |
| --- | --- |
| type | Filter the paths by what you type |
| `↑` / `↓`, `Ctrl+p` / `Ctrl+n` | The row above / below |
| `PgUp` / `PgDn` | A page of rows |
| `Alt+j` / `Alt+k` | Scroll the preview beside the list |
| `Enter` | Take the row: the tree walks to it and the preview shows it |
| `Esc` / `Ctrl+c` | Close the overlay and go back to the panes |

`Esc` and `Alt` are told apart by the terminal, and not every terminal tells
them apart: one that sends an Alt key as `Esc` followed by a letter closes the
overlay instead of scrolling the preview beside the list.

Editing a query is the usual line editing: `←` / `→` or `Ctrl+b` / `Ctrl+f`,
`Alt+b` / `Alt+f` by word, `Home` / `End` or `Ctrl+a` / `Ctrl+e`, `Backspace` /
`Ctrl+h`, `Delete` / `Ctrl+d`, `Ctrl+w` by word, `Ctrl+u` to the head of the
line and `Ctrl+k` to the end. A pasted passage arrives as a single line, capped
at 1024 characters.

The watch log, opened with `w` and closed with the same key or `Esc`, lists what
has changed under the root since the program started, even while the log is
closed:

| Key | Action |
| --- | --- |
| `j` / `k` | The row below / above |
| `g` / `G` | The first / the last row |
| `Enter` | Take the row: the tree walks to it, whether the file is still there or not |
| `w` / `Esc` | Close the log |

The help overlay, opened with `?` and closed the same way, is the keymap table
itself: `j` / `k` move a row, `f` / `b` a page, `g` / `G` the ends.

`e` opens the file the cursor is on in the tree, or the one the preview is
showing when the focus is there, in `$VISUAL`, then `$EDITOR`, then `vi`, with
the line the preview is showing where the editor can be told one (`+N file` for
vi, nano and emacs, `file:N` for helix, nothing for an editor that is not in
the table). The terminal is handed over for as long as the editor runs and
taken back afterwards.

## How it is put together

The program is one Flux loop. Input and background work produce `Action`
values, a pure `reduce` applies them to the `AppState` and returns `Effect`
values, and an `EffectRunner` carries the effects out. Nothing in a reducer
touches the filesystem, a clock or a channel.

```
crossterm thread ──actions──┐
                            ▼
                        Store loop ──effects──► EffectRunner ──actions──┐
                            │                                              │
                            ▼                                             │
                         reduce ──► AppState ──► render ──────────────────┘
```

Directories are listed lazily: a directory is read the first time it is opened,
in a blocking task, and the result carries a generation number so that a listing
that comes back late is dropped instead of overwriting a newer one. A frame only
ever builds the rows that fit on the screen, so a directory with a hundred
thousand entries still starts quickly.

Files are read the same way. The body is sanitized before it is ever stored, so
a control sequence in a file cannot reach the terminal; a file is cut at 1 MiB
and 2000 lines, and the header says so rather than letting a short body look
like a whole file. Binary files are recognised by a NUL byte in their first
8 KiB and shown by kind and size. `r` reads the shown file again, keeping the
body and the scroll position in place until the new document has arrived.

The same read also gives every line its meaning. `syntect` parses the file on
that blocking task and the theme maps the scopes onto the terminal's 16 colors,
so a keyword, a string, a comment, a number, a type and a function are told
apart without the program ever choosing a color itself. The syntax definitions
load the first time something is highlighted rather than at startup, and the
run stops at 10,000 lines or 150 ms, with `highlight truncated` in the header
when it does.

Inside a git repository the baseline is the `HEAD` commit. `git status` names
the files that differ, `git cat-file` reads the versions to compare against,
and each of them gets a status letter in the tree (`M`, `A`, `D`) and a count
of changed lines on the right end of the row. A directory sums up the rows
below it even before it has been opened. The preview shows the two versions
side by side or interleaved, folded to a few lines around each hunk, and `n` /
`N` jump from hunk to hunk. A file that is too large to count says `?` rather
than a wrong number.

Outside a repository there is no commit to compare against, so the first build
also scans the tree in the background and keeps what it read as the contents at
the start of the session. Edits, new files and deletions after that point get
the same letters and counts, `auto` falls back to that snapshot with a message
in the status bar, and `B` swaps between the two without blanking the tree. The
scan keeps at most 1 MiB per file and 64 MiB in total, favouring source files
and small ones: a file past that budget answers with no baseline, so its row
says `?` and the preview says `no baseline`. A change made before the program
started cannot be noticed at all, because there is no earlier state to compare
with. Without `git` on the machine there is no `HEAD` to compare against
either: the status bar says `git not found`, `B` has nothing to swap to and
`auto` stands on the snapshot.

The finder overlay keeps one list of every path below the root. Opening it
starts a walk that runs beside the interface and answers in chunks, so the list
is worth reading before the walk is over; each chunk goes into a fuzzy matcher
and only the rows that fit are ever drawn. With nothing typed the changed paths
come first, newest first. The row under the selection is previewed while the
overlay is up, and `Enter` walks the tree down to the file, expanding the
directories on the way, and hands the preview over.

`Ctrl+g` turns the same overlay into a search of what is inside the files. It
runs `rg` as a child process from the root, reads its output as JSON and shows
the hits as `path:line: text`, two characters at the shortest. Every keystroke
starts a new run after cancelling the old one, and the answer carries the
generation it was asked for, so a search that comes back late says nothing.
A thousand hits stop the run. `Enter` on a hit does what it does for a path,
and lights the line it landed on in the preview, centred in the pane. If `rg`
is not installed the overlay says so and will not open again until the program
is restarted.

The watch log keeps the last ten thousand changes under the root, whether or
not the overlay is up: a change of the same kind to the same path within a
second is one row with a count rather than ten of them. The log follows the
newest row until the reader moves up; from then on it counts what arrived while
the reader was elsewhere until `G` takes it back to the end.

`e` gives the terminal back to an editor. The input pump stops reading, the
alternate screen and raw mode go away, and the child runs while the store keeps
taking in what the filesystem is doing, without drawing a frame. The screen is
drawn again from scratch once the editor is gone, and the keys it was given are
not answered a second time.

The terminal is restored on quit, on a panic and on `SIGTERM` or `SIGHUP`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`tests/ui_snapshot.rs` holds the frame snapshots, `tests/tree_properties.rs`
the invariants of the tree, `tests/performance.rs` the size of a large
directory. Review a snapshot with `cargo insta review`.

`tests/benchmarks.rs` holds the measurements behind the performance table of
the specification. They are ignored by default, because they take their time:

```sh
cargo test --release --test benchmarks -- --ignored --nocapture
```