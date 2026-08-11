# ORBIT

ORBIT is a Rust terminal application for Linux. Phase 1 is intentionally small:
it opens a desktop window, starts the user's default shell in a real PTY, renders
terminal output, forwards keyboard input, supports window resize, and keeps basic
scrollback.

## Run

This machine was bootstrapped with a project-local Rust install because system
Rust was missing.

```bash
RUSTUP_HOME="$PWD/.rustup" CARGO_HOME="$PWD/.cargo" PATH="$PWD/.cargo/bin:$PATH" cargo run
```

## Phase 1 Stack

- `eframe`/`egui`: desktop application window and drawing surface.
- `portable-pty`: real pseudo-terminal used to run the user's shell.
- `vt100`: ANSI terminal parser and screen state with scrollback.

The terminal starts in `$HOME` when available and falls back to the current
directory. The shell is read from `$SHELL`, falling back to `/bin/bash`.
