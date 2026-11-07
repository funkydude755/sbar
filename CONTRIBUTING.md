# Contributing to sbar

sbar is a small terminal utility for macOS and Linux, with stable and beta releases. Bug reports, documentation improvements, and focused pull requests are welcome.

## Report a bug

Open a [GitHub issue](https://github.com/funkydude755/sbar/issues/new) with:

- Your operating system and architecture, terminal application, shell, and `sbar --version`.
- The commands or steps that reproduce the issue, what you expected, and what happened.
- Terminal dimensions and a screenshot when relevant. Remove private terminal contents before sharing.
- GPU model and driver version if the issue concerns GPU metrics.

For a security vulnerability, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## Make a change

1. Fork the repository and create a branch for your change.
2. Install current stable Rust/Cargo, Git, Make, Python 3, and a C toolchain. On macOS, use the Xcode Command Line Tools.
3. Keep the change focused. Add a regression test for a behavior fix and update documentation when behavior changes.
4. Run the checks below, then open a pull request explaining the problem, the change, and how you tested it.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
make test
cargo build --release --locked
python3 tests/test_pty.py target/release/sbar
```

The PTY tests use disposable terminals. CI runs on macOS and Linux; you do not need both systems locally. Never test terminal cleanup against someone else's active terminal.

Discuss substantial features in an issue first. Maintainer review and support are provided as time permits, with no promised response time. Please keep discussions respectful and constructive.

Contributions are provided under the project's [MIT license](LICENSE). If dependency versions change, update the applicable notices in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
