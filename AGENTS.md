# AGENTS.md

## Project

Inkline is a native vector drawing application written in Rust with egui and
wgpu. It uses pressure-aware tessellated strokes, layers with blend modes,
selection and transform tools, undo/redo, and SVG/PNG/JPEG export.

## Commands

Always run the full validation set after changing Rust or WGSL files:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Use `cargo run --release` for manual verification.

## Architecture

- `src/main.rs`: native entry point and window setup
- `src/app.rs`: UI, input handling, tool state, file dialogs, and export
- `src/model.rs`: `Document`, `Layer`, `Stroke`, and blend modes
- `src/geometry.rs`: stroke outline generation and GPU geometry tessellation
- `src/render.rs`: wgpu pipelines, layer textures, and compositing
- `src/selection.rs`: R-tree based hit-testing and rectangle selection
- `src/commands.rs`: reversible command model used by undo/redo
- `shaders/`: WGSL shaders for solid stroke rendering and layer compositing

## Conventions

- Keep the public document model serializable through `bincode`.
- Keep undo/redo changes represented as reversible `Command` values.
- Do not commit `target/` or generated binaries such as the root `test` file.
- Preserve the existing separation between document state and GPU state.
- Prefer focused tests near the relevant model, geometry, or command behavior.
- When changing shaders or rendering, verify both native rendering and export
  paths.

## Scope

Make only the changes required by the current request. Avoid unrelated
refactors, formatting churn, and dependency upgrades.
