//! The measurements behind `docs/benchmarks.md`, one per row of the
//! performance table of the specification (§11).
//!
//! They are ignored by default because they write a large fixture to disk or
//! spend seconds in front of a wall clock:
//!
//! ```sh
//! cargo test --release --test benchmarks -- --ignored --nocapture
//! ```

mod support;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use yagura::app::{Action, Command, reduce::reduce};
use yagura::core::fswatch::ChangeKind;
use yagura::core::fswatch::coalesce::{Coalescer, Taken};

/// The rows an 80x24 pane has room for.
const ROWS: u16 = 21;

/// What the first frame is allowed to take (spec §11).
const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(100);

/// What one key press is allowed to take before the frame answers (spec §11).
const KEY_BUDGET: Duration = Duration::from_millis(33);

#[test]
#[ignore = "a measurement, run by hand"]
fn the_first_frame_of_a_million_entries() {
    let building = Instant::now();
    let mut state = support::state_with_entries(1_000_000);
    let built = building.elapsed();

    state.tree.set_viewport(ROWS);
    let rows = Instant::now();
    state.tree.refresh();
    let row_time = rows.elapsed();

    let frame = Instant::now();
    let text = support::render_text(&state);
    let frame_time = frame.elapsed();

    assert!(text.contains("dir0000"), "the frame shows the root");
    println!(
        "first frame of 1,000,000 entries: listing {built:?}, visible rows {row_time:?}, \
         frame {frame_time:?} (budget {FIRST_FRAME_BUDGET:?})"
    );
    assert!(
        frame_time < FIRST_FRAME_BUDGET,
        "the frame took {frame_time:?}"
    );
}

#[test]
#[ignore = "a measurement, run by hand"]
fn the_time_from_a_key_to_the_frame() {
    let mut state = support::state_with_entries(100_000);
    state.tree.set_viewport(ROWS);
    state.tree.refresh();

    const KEYS: usize = 200;
    let start = Instant::now();
    for _ in 0..KEYS {
        reduce(&mut state, Action::Command(Command::PageDown));
        support::render_text(&state);
    }
    let per_key = start.elapsed() / KEYS as u32;

    println!("key to frame over {KEYS} presses: {per_key:?} each (budget {KEY_BUDGET:?})");
    assert!(per_key < KEY_BUDGET, "a key took {per_key:?}");
}

#[test]
#[ignore = "a measurement, run by hand"]
fn ten_thousand_events_a_second_settle_in_the_coalescer() {
    // Ten seconds of a hot directory: the same thousand paths saved over and
    // over. The map holds paths, so the batches stay small and nothing keeps
    // growing (spec §11, §2 step 2).
    const EVENTS: usize = 100_000;
    const DISTINCT: usize = 1_000;
    let paths: Vec<PathBuf> = (0..DISTINCT)
        .map(|index| PathBuf::from(format!("src/module{index:03}.rs")))
        .collect();
    let base = Instant::now();

    let mut coalescer = Coalescer::new();
    let mut batches = 0usize;
    let mut biggest = 0usize;
    let start = Instant::now();
    for event in 0..EVENTS {
        // One event every 100 microseconds of the simulated clock: 10,000/s
        // for ten seconds, without waiting for the wall clock to agree.
        let at = base + Duration::from_micros(event as u64 * 100);
        coalescer.push(paths[event % DISTINCT].clone(), ChangeKind::Modify, at);
        while coalescer.ready(at) {
            match coalescer.flush(at) {
                Some(Taken::Batch(changes)) => {
                    batches += 1;
                    biggest = biggest.max(changes.len());
                }
                Some(Taken::Rescan) | None => break,
            }
        }
    }
    let push_and_drain = start.elapsed();

    // Ten seconds of the same clock are drained in far less than a second of
    // the real one, which is what keeps the UI ahead of the flood.
    println!(
        "coalescer, {EVENTS} events over a simulated ten seconds: {push_and_drain:?} \
         of wall clock, {batches} batches, biggest batch {biggest} paths"
    );
    assert!(batches > 0, "a steady stream must flush on the deadline");

    // Every path of a directory that grows faster than the map can hold it:
    // the map is thrown away rather than allowed to grow (spec §11).
    const OVERFLOW: usize = 150_000;
    let paths: Vec<PathBuf> = (0..OVERFLOW)
        .map(|index| PathBuf::from(format!("generated/file{index:06}.txt")))
        .collect();
    let start = Instant::now();
    let mut coalescer = Coalescer::new();
    for (index, path) in paths.iter().enumerate() {
        let at = base + Duration::from_micros(index as u64);
        coalescer.push(path.clone(), ChangeKind::Create, at);
    }
    let flood = start.elapsed();
    assert!(
        coalescer.rescan(),
        "the map is bounded and asks for a rescan instead of growing"
    );
    println!(
        "coalescer, {OVERFLOW} distinct paths in {flood:?}: bounded, the tree is \
         asked for a rescan"
    );
    assert_eq!(coalescer.flush(base), Some(Taken::Rescan));
}

#[test]
#[ignore = "a measurement, run by hand"]
fn a_directory_of_a_hundred_thousand_entries_on_disk() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).expect("mkdir");
    let making = Instant::now();
    for index in 0..100_000 {
        std::fs::write(root.join(format!("file{index:06}.txt")), b"x").expect("write");
    }
    println!("making the fixture: {:?}", making.elapsed());

    let start = Instant::now();
    let state = support::loaded_state(&root, false);
    let startup = start.elapsed();
    let frame = Instant::now();
    let text = support::render_text(&state);
    let frame_time = frame.elapsed();

    assert!(text.contains("file000000"), "the first rows are drawn");
    println!(
        "startup of a directory with 100,000 entries: {startup:?} until the listing \
         arrived, frame {frame_time:?} (budget {FIRST_FRAME_BUDGET:?})"
    );
    assert!(
        frame_time < FIRST_FRAME_BUDGET,
        "the frame took {frame_time:?}"
    );
}
