# Inkline

Inkline is a lightweight, GPU-accelerated vector drawing application built with
Rust, egui, and wgpu. It focuses on fluid pressure-sensitive strokes, layered
editing, and clean SVG or raster export.

## Features

- Pressure-sensitive vector brush with GPU tessellation
- Layer management with visibility, opacity, and blend modes
- Brush and marquee selection/move tools
- Undo and redo for document and layer operations
- Project files with compressed binary serialization
- SVG, PNG, and JPEG export
- Recent-project history

## Downloads

The latest installers are published to the
[latest release](https://github.com/lichspace/Inkline/releases/latest) after
every successful `main` build.

- [Windows x64 installer](https://github.com/lichspace/Inkline/releases/download/latest/inkline-windows-x64.msi)
- [macOS universal disk image](https://github.com/lichspace/Inkline/releases/download/latest/inkline-macos.dmg)
- [Linux Debian package](https://github.com/lichspace/Inkline/releases/download/latest/inkline-linux-amd64.deb)

On macOS, open the disk image and drag Inkline into `Applications`. The disk
image supports both Apple silicon and Intel Macs. If an unnotarized build is
blocked, try to open Inkline once, then use **System Settings → Privacy &
Security → Open Anyway**, or Control-click the app and choose **Open**. On
Linux, install the package with
`sudo apt install ./inkline-linux-amd64.deb`.

## Requirements

- Rust 1.92 or newer (the checked-in toolchain file installs the required iOS targets)
- A GPU and drivers compatible with wgpu
- Xcode for iPhone and iPad builds

## Run

```sh
cargo run --release
```

### iPhone and iPad

Open `ios/Inkline.xcodeproj` in Xcode, select the `Inkline` scheme and an
iPhone or iPad simulator, then press Run. The Xcode build phase compiles and
links the Rust static library automatically.

For a physical device, select your development team under Signing &
Capabilities and use a unique bundle identifier before pressing Run. Inkline
supports iOS 16 and later. Projects and exports are available in
Files → On My iPhone/iPad → Inkline.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

### macOS release signing

The release workflow always signs the complete app bundle before creating the
disk image. Without release credentials it uses an ad hoc signature, which
keeps the bundle structurally valid but still requires the user's explicit
Gatekeeper approval. To publish a build that opens without that warning,
configure these GitHub Actions secrets:

- `MACOS_CERTIFICATE_BASE64`: base64-encoded Developer ID Application `.p12`
- `MACOS_CERTIFICATE_PASSWORD`: password for the `.p12`
- `MACOS_SIGNING_IDENTITY`: certificate identity, such as
  `Developer ID Application: Example (TEAMID)`
- `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, and `APPLE_TEAM_ID`: credentials
  used by Apple's notary service

The main source modules are:

- `src/app.rs`: application state, UI, tools, and file actions
- `src/model.rs`: document, layer, and stroke model
- `src/geometry.rs`: pressure-aware stroke tessellation and SVG paths
- `src/render.rs`: wgpu pipeline and layer compositing
- `src/selection.rs`: spatial hit-testing and rectangle selection
- `src/commands.rs`: reversible commands for undo and redo

## License

MIT. See [LICENSE](LICENSE).
