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

## Run

```sh
cd spikes/gpui
cargo build --release
./target/release/seed /private/tmp/gpui-spike/ledger.db   # a new file
./target/release/chaperone-ui-gpui --db /private/tmp/gpui-spike/ledger.db
```

`--db` is required: the spike never falls back to the real ledger.

## Measure (R14 U1 to U4)

From the repo root, with both apps built in release:

```sh
python3 -I spikes/gpui/measure.py spikes/gpui/target/release/seed \
  spikes/gpui/target/release/chaperone-ui-gpui target/release/chaperone-ui
cargo test --release -p chaperone-ui -- --ignored --nocapture commit_to_redraw
```

The script prints U1, U3 and U4 for both apps on one seeded ledger, and the
GPUI U2: commit to the app's `drawn` line (`--trace`), which includes the
render. egui's U2 is its own test, which stops at the repaint request.
