Inkline is a Rust, egui, and wgpu vector drawing application.

- Follow the project conventions in `AGENTS.md`.
- After changing Rust or WGSL files, run formatting, clippy, and tests.
- Keep document model changes reversible through the `Command` system.
- Do not commit `target/`, generated binaries, or unrelated dependency changes.
- Make focused changes and preserve the existing module boundaries.
