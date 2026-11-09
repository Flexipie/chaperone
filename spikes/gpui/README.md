# GPUI spike (#134)

chaperone-ui's board in [GPUI](https://www.gpui.rs) and GPUI Kit
(`gpui-component`), to decide whether GPUI replaces egui. It is its own
Cargo workspace: the root workspace, CI and other lanes never compile it.

- Pins: `gpui-component =0.9.1`, `gpui-kit =0.9.1` (its platform, the
  `component` feature only) and the `gpui-pre =0.3.12` snapshot both pin.
  No `gpui-fast` crate is in `Cargo.lock`.
- Toolchain: `rust-toolchain.toml` pins `nightly-2026-10-20`, because
  gpui-pre 0.3.11 and 0.3.12 call `Option::map_or_default`, which is
  unstable in the root's Rust 1.95. Keeping GPUI means bumping the root pin.
- "Needs you" is chaperone-ui's own `needs.rs`, compiled here with
  `#[path]`, together with its `board.rs`. The ledger is opened read-only.
- `src/theme.rs` derives every colour from three tokens (base, accent,
  contrast) and writes them onto GPUI Kit's theme.
- The lane page (`src/page.rs`) reads core's `lane_page`, the lane's `stats`
  row and `repo_median` on the watcher's connection, keyed on the ledger
  version and the lane, and `git::lane_files` once on a background thread
  (no area column). Its word helpers are copies of chaperone-ui's `lane.rs`,
  `numbers.rs` and `diff.rs`, which import egui, each marked with its
  source. The repo median is read again at most every 60 s, as chaperone-ui
  does.
- ⌘K opens GPUI Kit's command palette (Go with ⌘1 and ⌘2, every repo,
  every open lane); a lane's Merge opens a confirm that writes nothing.

## Run

```sh
cd spikes/gpui
cargo build --release
./target/release/seed /private/tmp/gpui-spike/ledger.db   # a new file
./target/release/chaperone-ui-gpui --db /private/tmp/gpui-spike/ledger.db
```

`--db` is required: the spike never falls back to the real ledger.
Hidden start flags open what would otherwise take a click, for screenshots
and U4: `--lane <id>` (that lane's page), `--palette`, and `--lane <id>
--dialog` (its Merge confirm).

## Measure (R14 U1 to U4)

From the repo root, with both apps built in release:

```sh
python3 -I spikes/gpui/measure.py spikes/gpui/target/release/seed \
  spikes/gpui/target/release/chaperone-ui-gpui target/release/chaperone-ui
cargo test --release -p chaperone-ui -- --ignored --nocapture commit_to_redraw
```

The script prints U1, U3 and U4 for both apps on one seeded ledger, U3 and
U4 for GPUI on lane 1's page (egui cannot open it without a click), and the
GPUI U2: commit to the app's `notify` line (`--trace`), the board handed to
the view as it asks for a redraw, where egui's own U2 test stops too. It
includes GPUI's hop from the background read to the main thread, not the
render: GPUI skips rendering a window that is not drawing.
