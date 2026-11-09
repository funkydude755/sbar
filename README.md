# sbar - a terminal system monitor for macOS and Linux

Keep CPU, RAM, GPU, disk, and network stats visible while you work in your shell. A persistent terminal status bar for macOS and Linux, written in Rust.

![Animated illustration of sbar: a model trains on two GPUs above a persistent status bar with changing CPU, RAM, GPU, disk, and network metrics.](assets/demo.svg)

*Illustrative model-training demo with example metrics and output.*

Try the stable release on macOS or Linux:

```bash
brew install funkydude755/tap/sbar
sbar
```

Type `exit` or Ctrl+D to return to your normal shell. [Installation details](#install) · [How it works](#how-it-works) · [Report a bug](https://github.com/funkydude755/sbar/issues/new)

## Install

### Homebrew (macOS and Linux)

The [public tap](https://github.com/funkydude755/homebrew-tap) provides prebuilt bottles for Apple Silicon and Intel Macs on macOS 15 or newer, and for Linux on x86-64 and ARM64. Bottle installs need neither Rust nor a GitHub account.

Without a compatible bottle, or with `--build-from-source`, Homebrew builds the tagged release and installs Rust and its build dependencies. Older Linux distributions may need Homebrew's runtime libraries.

If `sbar` is already installed from another tap, run `brew uninstall sbar` before installing it from this tap.

### From source

Build from source with current stable Rust and Cargo, Git, Make, and a C toolchain/linker. On macOS, install the Xcode Command Line Tools; on Linux, install your distribution's C build tools. Python 3 is needed only for the regression tests.

```bash
git clone https://github.com/funkydude755/sbar.git
cd sbar
git checkout v0.2.4
make install PREFIX="$HOME/.local"
"$HOME/.local/bin/sbar" --version
```

Add `$HOME/.local/bin` to your `PATH`. Set `PREFIX` to choose another installation directory; run `make uninstall` with the same `PREFIX` to remove the executable and its license notices.

See [CHANGELOG.md](CHANGELOG.md) for release changes.

## Commands

| Command | Description |
|---------|-------------|
| `sbar` | Launch shell with status bar |
| `sbar --once` | Print metrics once and exit |
| `sbar --version` | Print version |
| `sbar --help` | Show help |

## How it works

`sbar` runs your `$SHELL` inside a PTY and pins the status bar below it. Metrics update every second in a background worker, with bounded GPU probes. The layout adapts to terminal size, and terminal settings are restored on exit.

CPU, memory, disk, and network metrics use native system APIs. Optional GPU metrics use `ioreg` on Apple Silicon or `nvidia-smi` on Linux. Network rates sum all interfaces. The disk meter selects the system volume (the writable Data volume on macOS), and mixed GPUs show individual models and capacities when space allows.

## Terminal compatibility

Use an interactive terminal with ANSI cursor controls and scroll-region support. The bar pauses for alternate-screen applications and resumes when they exit.

`sbar` forwards shell output; it is not a complete terminal emulator. An ordinary clear-screen command can erase the bar until the next metrics refresh. Programs that manage custom scroll regions or use unusual terminal modes can still disrupt the display; run them outside `sbar` if needed. `sbar --once` prints a snapshot without wrapping a shell.

## Development

With stable Rust, Python 3, Make, and a C toolchain installed, run the checks locally:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
make test
cargo build --release --locked
python3 tests/test_pty.py target/release/sbar
```

Focused pull requests are welcome. Add a regression test for behavior fixes and include your test results and platform. This repository does not run GitHub CI. Update [dependency notices](THIRD_PARTY_LICENSES.md) when dependencies change.

For bugs, include your OS, terminal, shell, `sbar --version`, and reproduction steps in a [GitHub issue](https://github.com/funkydude755/sbar/issues/new). Use [private vulnerability reporting](SECURITY.md) for security issues.

Before a release, update the version in `Cargo.toml` and `Cargo.lock`, the changelog, and installation instructions. Run the checks above plus `cargo package --locked`, inspect `cargo package --list --locked`, and verify installation from a fresh source copy. Merge through a pull request, tag the tested commit with a new version, and publish notes on GitHub; never move an existing release tag. Mark betas as prereleases. Update and test the Homebrew formula and bottles only for stable releases.

## License

[MIT](LICENSE). Notices for the locked macOS and Linux dependencies are in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
