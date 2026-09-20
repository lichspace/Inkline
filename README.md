# Inkline

Inkline is a lightweight, GPU-accelerated vector drawing application built with
Rust, egui, and wgpu. It focuses on fluid pressure-sensitive strokes, layered
editing, and clean SVG or raster export.

## Features

- Pressure-sensitive vector brush with GPU tessellation
- Layer management with visibility, opacity, and blend modes
- Brush, marquee selection, and transform tools
- Undo and redo for document and layer operations
- Project files with compressed binary serialization
- SVG, PNG, and JPEG export
- Recent-project history

## Downloads

The latest binaries are published to the
[latest release](https://github.com/lichspace/Inkline/releases/latest) after
every successful `main` build.

- [Windows x64](https://github.com/lichspace/Inkline/releases/download/latest/inkline-windows-x64.exe)
- [macOS](https://github.com/lichspace/Inkline/releases/download/latest/inkline-macos)
- [Linux](https://github.com/lichspace/Inkline/releases/download/latest/inkline-linux)

## Requirements

- Rust 1.85 or newer
- A GPU and drivers compatible with wgpu

## Run

```sh
cargo run --release
```

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The main source modules are:

- `src/app.rs`: application state, UI, tools, and file actions
- `src/model.rs`: document, layer, and stroke model
- `src/geometry.rs`: pressure-aware stroke tessellation and SVG paths
- `src/render.rs`: wgpu pipeline and layer compositing
- `src/selection.rs`: spatial hit-testing and rectangle selection
- `src/commands.rs`: reversible commands for undo and redo

## License

MIT. See [LICENSE](LICENSE).
