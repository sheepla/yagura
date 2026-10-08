//! Frame snapshots.
//!
//! Every case exists twice: once with colors and once with `NO_COLOR`, so that
//! a change in how the theme maps roles to escapes cannot pass unnoticed.

mod support;

use std::path::Path;

use insta::assert_snapshot;
use ratatui::style::Color;

use support::{
    Fixture, colors_in_frame, fixture, loaded_state, plain_state, preview_state, render_row_styles,
    render_text,
};

#[test]
fn the_startup_frame_shows_the_root() {
    let dir = fixture();
    let state = plain_state(dir.path(), false);
    assert_snapshot!("startup_text", render_text(&state));
    assert_snapshot!("startup_styles", render_row_styles(&state));
}

#[test]
fn the_root_listing_is_shown_after_it_arrives() {
    let dir = fixture();
    let state = loaded_state(dir.path(), false);
    assert_snapshot!("loaded_text", render_text(&state));
    assert_snapshot!("loaded_styles", render_row_styles(&state));
}

#[test]
fn the_root_listing_is_shown_without_colors() {
    let dir = fixture();
    let state = loaded_state(dir.path(), true);
    assert_snapshot!("loaded_no_color_text", render_text(&state));
    assert_snapshot!("loaded_no_color_styles", render_row_styles(&state));
}

#[test]
fn no_color_frames_paint_no_color() {
    let dir = fixture();
    for state in [
        plain_state(dir.path(), true),
        loaded_state(dir.path(), true),
    ] {
        for color in colors_in_frame(&state) {
            assert!(
                matches!(
                    color,
                    Color::Reset | Color::White | Color::Gray | Color::Black
                ),
                "unexpected color {color:?} in a NO_COLOR frame"
            );
        }
    }
}

#[test]
fn a_colored_frame_paints_something() {
    let dir = fixture();
    let colors = colors_in_frame(&loaded_state(dir.path(), false));
    assert!(
        colors.iter().any(|color| !matches!(color, Color::Reset)),
        "the colored frame should not be monochrome: {colors:?}"
    );
}

#[test]
fn a_terminal_that_is_too_small_says_so() {
    let dir = fixture();
    let mut state = loaded_state(dir.path(), false);
    state.size = (50, 12);
    assert_snapshot!("too_small_text", render_text(&state));
}

#[test]
fn hiding_the_tree_gives_the_width_to_the_preview() {
    let dir = fixture();
    let mut state = loaded_state(dir.path(), false);
    state.tree_visible = false;
    assert_snapshot!("tree_hidden_text", render_text(&state));
}

#[test]
fn the_preview_mode_shows_in_the_status_bar() {
    use yagura::app::state::PreviewMode;
    use yagura::app::{Action, Command, reduce::reduce};

    let dir = fixture();
    let mut state = loaded_state(dir.path(), false);
    reduce(&mut state, Action::Command(Command::CyclePreviewMode));
    assert_eq!(state.preview_mode, PreviewMode::InlineDiff);
    assert_snapshot!("preview_diff_text", render_text(&state));
}

#[test]
fn a_selected_file_shows_in_the_preview_pane() {
    let dir = fixture();
    let state = preview_state(dir.path(), "README.md", false);
    assert_snapshot!("preview_content_text", render_text(&state));
}

#[test]
fn a_long_file_scrolls_and_reports_its_position() {
    use yagura::app::state::Focus;
    use yagura::app::{Action, Command, reduce::reduce};

    let dir = fixture();
    // A file longer than the pane, so the scroll commands have somewhere to go.
    let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.path().join("long.txt"), body).expect("write long file");
    let mut state = preview_state(dir.path(), "long.txt", false);
    state.focus = Focus::Preview;
    reduce(&mut state, Action::Command(Command::ScrollPageDown));
    reduce(&mut state, Action::Command(Command::ScrollDown));
    assert_snapshot!("preview_scrolled_text", render_text(&state));
}

#[test]
fn a_binary_file_shows_its_kind_and_size() {
    let dir = fixture();
    std::fs::write(
        dir.path().join("logo.png"),
        [0x89, b'P', b'N', b'G', 0, 1, 2, 3],
    )
    .expect("write");
    let state = preview_state(dir.path(), "logo.png", false);
    assert_snapshot!("preview_binary_text", render_text(&state));
}

#[test]
fn a_file_that_cannot_be_read_says_so() {
    let dir = fixture();
    let mut state = loaded_state(dir.path(), false);
    // The tree still points at a file the read cannot reach, which is what a
    // permission error looks like from the pane.
    let file = state
        .tree
        .model
        .lookup(std::path::Path::new("README.md"))
        .expect("the listing knows the file");
    state.tree.select(file);
    state.preview.target(std::path::PathBuf::from("README.md"));
    state.preview.set_failed();
    state.set_error("cannot preview README.md: permission denied (os error 13)");
    assert_snapshot!("preview_failed_text", render_text(&state));
}

#[test]
fn a_program_is_shown_with_a_color_per_meaning() {
    let dir = fixture();
    // Written at the root, because the fixture only ever expands the root and
    // a nested file would have no node to be selected by.
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() {\n    let n = 42; // count\n}\n",
    )
    .expect("write");
    let state = preview_state(dir.path(), "main.rs", false);
    assert_snapshot!("preview_program_text", render_text(&state));
    assert_snapshot!("preview_program_styles", render_row_styles(&state));
}

#[test]
fn a_program_without_colors_paints_no_colors() {
    let dir = fixture();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").expect("write");
    let state = preview_state(dir.path(), "main.rs", true);
    assert_snapshot!("preview_program_no_color_styles", render_row_styles(&state));
    for color in colors_in_frame(&state) {
        assert!(
            matches!(
                color,
                Color::Reset | Color::White | Color::Gray | Color::Black
            ),
            "unexpected color {color:?} in a NO_COLOR frame"
        );
    }
}

/// The fixture inside a real repository with one commit, so the baseline has
/// something to compare the working tree against.
fn git_fixture() -> Fixture {
    let dir = fixture();
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    // Keeps the tests independent from a signing setup on the machine.
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs the effects and reduces everything they send until `done`.
async fn run_until(
    runner: &yagura::app::runner::EffectRunner,
    rx: &mut tokio::sync::mpsc::Receiver<yagura::app::Action>,
    state: &mut yagura::app::state::AppState,
    effects: Vec<yagura::app::Effect>,
    done: impl Fn(&yagura::app::Action) -> bool,
) {
    use std::time::Duration;

    use yagura::app::reduce::reduce;
    for effect in effects {
        runner.run(effect);
    }
    loop {
        let action = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the effects finished in time")
            .expect("the store side of the channel stays open");
        let matched = done(&action);
        reduce(state, action);
        if matched {
            return;
        }
    }
}

#[test]
fn a_changed_file_shows_a_diff_against_head() {
    use std::cell::Cell;

    use yagura::app::state::PreviewMode;
    use yagura::app::{Action, Command, reduce::reduce};
    use yagura::features::preview::reducer::sync_target;

    let dir = git_fixture();
    // The committed line and the file on disk now differ.
    std::fs::write(dir.path().join("README.md"), "# yagura!\n").expect("write");
    let mut state = plain_state(dir.path(), false);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Action>(16);
    runtime.block_on(async {
        let runner = yagura::app::runner::EffectRunner::new(dir.path().to_path_buf(), true, tx);

        // The listing and the baseline build both report in before the
        // preview can ask either of them anything.
        let listed = Cell::new(false);
        let built = Cell::new(false);
        let startup = state.startup_effects();
        run_until(&runner, &mut rx, &mut state, startup, |action| {
            listed.set(listed.get() || matches!(action, Action::DirLoaded { .. }));
            built.set(built.get() || matches!(action, Action::BaselineReady { .. }));
            listed.get() && built.get()
        })
        .await;

        let file = state
            .tree
            .model
            .lookup(Path::new("README.md"))
            .expect("the fixture has a README");
        state.tree.select(file);
        let load = sync_target(&mut state);
        run_until(&runner, &mut rx, &mut state, load, |action| {
            matches!(action, Action::PreviewLoaded { .. })
        })
        .await;

        // One step of the cycle brings up the inline diff, which re-reads the
        // file against the baseline the build just handed over.
        let cycle = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        run_until(&runner, &mut rx, &mut state, cycle, |action| {
            matches!(action, Action::PreviewLoaded { .. })
        })
        .await;
    });

    assert_eq!(state.preview_mode, PreviewMode::InlineDiff);
    let doc = state.preview.doc.as_deref().expect("the preview arrived");
    assert!(doc.diff.is_some(), "the changed file has no diff");

    assert_snapshot!("preview_diff_rows_text", render_text(&state));
    assert_snapshot!("preview_diff_rows_styles", render_row_styles(&state));

    // Wide enough for the two columns, and the rows pair up again.
    assert!(
        reduce(
            &mut state,
            Action::Resize {
                width: 120,
                height: 30,
            }
        )
        .is_empty()
    );
    assert!(
        reduce(&mut state, Action::Command(Command::CyclePreviewMode)).is_empty(),
        "the split draws the rows already loaded"
    );
    assert_eq!(state.preview_mode, PreviewMode::SideBySide);
    assert!(
        state.visible_message().is_none(),
        "the terminal is wide enough for the split"
    );
    assert_snapshot!("preview_split_text", render_text(&state));
}

/// The cycle goes content → inline diff → split → content, and the frame the
/// pane draws while the content read is on its way must not draw the diff
/// rows as plain lines: they are two rows of another view (spec §6).
#[test]
fn cycling_back_to_normal_never_draws_its_rows_as_content() {
    use std::cell::Cell;

    use yagura::app::state::PreviewMode;
    use yagura::app::{Action, Command, reduce::reduce};
    use yagura::features::preview::reducer::sync_target;

    let dir = git_fixture();
    // The committed line and the file on disk now differ.
    std::fs::write(dir.path().join("README.md"), "# yagura!\n").expect("write");
    let mut state = plain_state(dir.path(), false);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Action>(16);
    runtime.block_on(async {
        let runner = yagura::app::runner::EffectRunner::new(dir.path().to_path_buf(), true, tx);

        // The listing and the baseline build both report in before the
        // preview can ask either of them anything.
        let listed = Cell::new(false);
        let built = Cell::new(false);
        let startup = state.startup_effects();
        run_until(&runner, &mut rx, &mut state, startup, |action| {
            listed.set(listed.get() || matches!(action, Action::DirLoaded { .. }));
            built.set(built.get() || matches!(action, Action::BaselineReady { .. }));
            listed.get() && built.get()
        })
        .await;

        // Wide enough for every step of the cycle to mean what it says.
        assert!(
            reduce(
                &mut state,
                Action::Resize {
                    width: 120,
                    height: 30,
                },
            )
            .is_empty()
        );

        let file = state
            .tree
            .model
            .lookup(Path::new("README.md"))
            .expect("the fixture has a README");
        state.tree.select(file);
        let load = sync_target(&mut state);
        run_until(&runner, &mut rx, &mut state, load, |action| {
            matches!(action, Action::PreviewLoaded { .. })
        })
        .await;

        // The inline diff, and the split drawn from the rows it already has.
        let cycle = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        run_until(&runner, &mut rx, &mut state, cycle, |action| {
            matches!(action, Action::PreviewLoaded { .. })
        })
        .await;
        assert!(
            state
                .preview
                .doc
                .as_deref()
                .is_some_and(|doc| doc.diff.is_some()),
            "the inline diff arrived"
        );
        assert!(
            reduce(&mut state, Action::Command(Command::CyclePreviewMode)).is_empty(),
            "the split draws the rows already loaded"
        );
        assert_eq!(state.preview_mode, PreviewMode::SideBySide);

        // Back to content: the body on screen was built as a diff, so the
        // pane waits for the read that matches instead of drawing it.
        let cycle = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::Content);
        assert!(!cycle.is_empty(), "the read for content was asked for");

        let waiting = render_text(&state);
        assert!(
            waiting.contains("loading…"),
            "the pane waits instead of drawing: {waiting}"
        );
        assert!(
            !waiting.lines().any(|line| line.contains("# yagura")),
            "the diff rows were drawn as content: {waiting}"
        );

        run_until(&runner, &mut rx, &mut state, cycle, |action| {
            matches!(action, Action::PreviewLoaded { .. })
        })
        .await;
    });

    assert_eq!(state.preview_mode, PreviewMode::Content);
    let doc = state.preview.doc.as_deref().expect("the content arrived");
    assert!(doc.diff.is_none(), "content carries no diff");

    let frame = render_text(&state);
    assert!(
        frame.lines().any(|line| line.contains("yagura!")),
        "the file on disk is shown: {frame}"
    );
    assert!(
        !frame.lines().any(|line| line.contains("# yagura")),
        "a diff row was drawn as content: {frame}"
    );
}
