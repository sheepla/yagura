//! Frame snapshots.
//!
//! Every case exists twice: once with colors and once with `NO_COLOR`, so that
//! a change in how the theme maps roles to escapes cannot pass unnoticed.

mod support;

use insta::assert_snapshot;
use ratatui::style::Color;

use support::{
    colors_in_frame, fixture, loaded_state, plain_state, preview_state, render_row_styles,
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
