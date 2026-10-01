//! Shared helpers for the integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier, Style};

use yagura::app::state::{AppState, Config};
use yagura::app::{Action, reduce::reduce};
use yagura::cli::{BaselineKind, IconStyle};
use yagura::core::fsmodel::{DirEntry, NodeKind};

/// Builds a state for a real directory, with the root listing already loaded.
///
/// The startup effects run through the real effect runner, so the snapshot
/// covers the whole path a real directory listing takes.
pub fn loaded_state(root: &Path, no_color: bool) -> AppState {
    let mut state = plain_state(root, no_color);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Action>(16);
    runtime.block_on(async {
        let runner = yagura::app::runner::EffectRunner::new(root.to_path_buf(), true, tx.clone());
        for effect in state.startup_effects() {
            runner.run(effect);
        }
        // Wait for the directory listing to arrive
        while let Some(action) = rx.recv().await {
            let should_break = matches!(action, Action::DirLoaded { .. });
            reduce(&mut state, action);
            if should_break {
                break;
            }
        }
        // Drain any remaining ticks
        while let Ok(action) = rx.try_recv() {
            if let Action::DirLoaded { .. } = action {
                reduce(&mut state, action);
            }
        }
    });
    state
}

/// A state with the root listing loaded and a file selected, so that the
/// preview pane shows a real document.
///
/// The preview is loaded through the real effect runner, exactly as it is at
/// runtime, so a snapshot covers the whole path from a key press to a body on
/// screen.
pub fn preview_state(root: &Path, rel: &str, no_color: bool) -> AppState {
    let mut state = loaded_state(root, no_color);
    let file = state
        .tree
        .model
        .lookup(Path::new(rel))
        .unwrap_or_else(|| panic!("the fixture has no {rel}"));
    state.tree.select(file);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Action>(16);
    runtime.block_on(async {
        let runner = yagura::app::runner::EffectRunner::new(root.to_path_buf(), true, tx.clone());
        for effect in yagura::features::preview::reducer::sync_target(&mut state) {
            runner.run(effect);
        }
        while let Some(action) = rx.recv().await {
            let done = matches!(action, Action::PreviewLoaded { .. });
            reduce(&mut state, action);
            if done {
                break;
            }
        }
    });
    assert_eq!(
        state.preview.preview,
        yagura::features::preview::PreviewStatus::Ready,
        "the preview of {rel} never arrived"
    );
    state
}

/// A state with nothing loaded yet, as the first frame after startup.
pub fn plain_state(root: &Path, no_color: bool) -> AppState {
    AppState::new(
        Config {
            root: root.to_path_buf(),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color,
            log: None,
            selftest_panic: false,
        },
        (80, 24),
    )
}

/// The state size the snapshots are rendered at.
pub const SIZE: (u16, u16) = (80, 24);

/// Renders a frame as text.
pub fn render_text(state: &AppState) -> String {
    render(state, |symbol, _| symbol.to_owned())
}

/// One style name per run of neighbouring cells, one row per line.
///
/// A style per cell would make the snapshot three times the size of the frame
/// and every diff unreadable; a run per row still pins down which role paints
/// which part of the screen.
pub fn render_row_styles(state: &AppState) -> String {
    let mut terminal =
        Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
    terminal
        .draw(|frame| yagura::app::view::render(frame, state))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            let mut runs: Vec<String> = Vec::new();
            for x in 0..buffer.area.width {
                let mark = style_mark(buffer[(x, y)].style());
                if runs.last() != Some(&mark) {
                    runs.push(mark);
                }
            }
            runs.join(" | ")
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Every color used anywhere in a frame, in the order it first appears.
pub fn colors_in_frame(state: &AppState) -> Vec<Color> {
    let mut terminal =
        Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
    terminal
        .draw(|frame| yagura::app::view::render(frame, state))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut colors = Vec::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let style = buffer[(x, y)].style();
            for color in [style.fg, style.bg].into_iter().flatten() {
                if !colors.contains(&color) {
                    colors.push(color);
                }
            }
        }
    }
    colors
}

fn render(state: &AppState, cell: impl Fn(&str, Style) -> String) -> String {
    let mut terminal =
        Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
    terminal
        .draw(|frame| yagura::app::view::render(frame, state))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    let style = buffer[(x, y)].style();
                    cell(buffer[(x, y)].symbol(), resolve(style))
                })
                .collect::<String>()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn resolve(style: Style) -> Style {
    Style {
        fg: style.fg,
        bg: style.bg,
        add_modifier: style.add_modifier,
        sub_modifier: style.sub_modifier,
        underline_color: style.underline_color,
    }
}

/// A short name for a style, `default` when it paints nothing.
fn style_mark(style: Style) -> String {
    let mut parts = Vec::new();
    match style.fg {
        None | Some(Color::Reset) => {}
        Some(color) => parts.push(format!("fg:{}", color_name(color))),
    }
    match style.bg {
        None | Some(Color::Reset) => {}
        Some(color) => parts.push(format!("bg:{}", color_name(color))),
    }
    for modifier in [
        Modifier::REVERSED,
        Modifier::BOLD,
        Modifier::DIM,
        Modifier::ITALIC,
        Modifier::UNDERLINED,
    ] {
        if style.add_modifier.contains(modifier) {
            parts.push(format!("{modifier:?}").to_lowercase());
        }
    }
    if parts.is_empty() {
        "default".to_owned()
    } else {
        parts.join("+")
    }
}

fn color_name(color: Color) -> String {
    match color {
        Color::Reset => "reset".to_owned(),
        Color::Rgb(r, g, b) => format!("rgb({r},{g},{b})"),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// A small tree used by the snapshots: three directories and two files.
///
/// The root lives in a directory with a fixed name, because the name of the
/// root is drawn in the pane title and in the first row: a random temporary
/// directory name would make every snapshot different.
pub struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    pub fn path(&self) -> &Path {
        &self.root
    }
}

pub fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path().join("root");
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
    std::fs::write(root.join("README.md"), "# yagura\n").expect("write");
    Fixture { _tmp: tmp, root }
}

/// The same fixture entries without touching the filesystem.
pub fn fixture_entries() -> Vec<DirEntry> {
    vec![
        entry("README.md", NodeKind::File),
        entry("docs", NodeKind::Dir),
        entry("src", NodeKind::Dir),
    ]
}

/// `count` entries under the root, for tests that need a list longer than a
/// screen without putting a hundred thousand files on disk.
pub fn many_entries(count: usize) -> Vec<DirEntry> {
    (0..count)
        .map(|index| {
            if index % 10 == 0 {
                entry(&format!("dir{index:04}"), NodeKind::Dir)
            } else {
                entry(&format!("file{index:04}.txt"), NodeKind::File)
            }
        })
        .collect()
}

/// A state whose root listing is `entries`, without touching the filesystem.
pub fn state_with_entries(count: usize) -> AppState {
    let mut state = plain_state(Path::new("root"), false);
    let root = state.tree.model.root_id();
    let generation = state.tree.load_gen[&root];
    reduce(
        &mut state,
        Action::DirLoaded {
            node: root,
            generation,
            result: Ok(many_entries(count)),
        },
    );
    state
}

fn entry(name: &str, kind: NodeKind) -> DirEntry {
    DirEntry {
        name: name.into(),
        kind,
        link_target: None,
        unreadable: false,
    }
}

/// A path outside of any existing directory.
pub fn missing_dir() -> PathBuf {
    PathBuf::from("/definitely/not/here")
}
