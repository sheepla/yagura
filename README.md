# yagura

A TUI file tree watcher for observing what a coding agent is doing. `yagura`
shows a static, lazily loaded tree of a directory on the left, leaves room for a
preview on the right, and puts the mode, the position in the tree and anything
that needs saying on the status bar.

This is milestone M0 (program skeleton, terminal lifecycle, Flux loop, layout),
M1 (the static file tree), M2 (the preview content, with syntax highlighting),
M3 (the git baseline and the diff) and M4 (real-time updates: watching,
batches, refreshed statistics, the head check, ages and the `F`/`c`/`o` keys).
The snapshot baseline, the finders, the help overlay, the editor launch and
the markdown rendering are not implemented yet.

## Status

| Milestone | What it covers | State |
| --- | --- | --- |
| M0 | CLI, terminal lifecycle, Flux loop, theme, layout, store loop | done |
| M1 | `core/fsmodel`, lazy tree, sorting, scrolling, icons, status bar, keymap | done |
| M2 | `core/preview`, lazy document loading, sanitized body, line numbers, scrolling, syntax highlighting | done |
| M3 | git baseline, diff engine, InlineDiff / SideBySide, `M/A/D` in the tree, `+N -N ~N` counts, hunk keys | done |
| M4 | filesystem watching, batched updates, statistics refresh, `.git` head check, ages and freshness, follow / filter / order | done |
| M5+ | snapshot baseline, finders, help, editor, markdown | not started |

## Build

```sh
cargo build --release
cargo install --path .
```

Rust 1.88 or newer, edition 2024.

## Usage

```sh
yagura [PATH] [--icons text|nerd] [--baseline auto|none] [--no-ignore] [--log LEVEL]
```

| Option | Meaning |
| --- | --- |
| `[PATH]` | The directory to show. Defaults to the working directory. |
| `--icons text` | ASCII icons: `d` for a directory, `f` for a file, `l` for a symlink. |
| `--icons nerd` | Nerd Font glyphs. |
| `--baseline auto\|none` | Which baseline to compare against. `auto` picks the git `HEAD` inside a repository and nothing outside of one. |
| `--no-ignore` | Ignore `.gitignore`, the global excludes and the user ignores. |
| `--log LEVEL` | Write a `tracing` log to a file. Also read from `YAGURA_LOG`. Nothing is logged without it. |

`NO_COLOR` in the environment turns the theme off.

## Keys

Everywhere: `q` and `Ctrl+c` quit, `Tab` moves the focus, `Esc` steps back,
`t` shows or hides the tree, `s` cycles the preview mode, `B` switches the
baseline (the snapshot side of it lands with M5), `r` reads the shown file
again, `n` / `N` jump to the next or the previous hunk of the diff.

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
overlay will be built from.

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