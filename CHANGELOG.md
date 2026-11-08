# Changelog

## 0.2.4 — 2026-11-08

- Release the system-disk selection and mixed-GPU label fixes from v0.2.3 as stable.
- Update Homebrew bottles for Apple Silicon and Intel Macs, and ARM64 and x86-64 Linux.
- Simplify repository documentation and add an illustrative model-training demo.

## 0.2.3 — 2026-11-07 (beta)

- Select and label the system disk instead of displaying the first enumerated volume. Prefer the writable Data volume on macOS, with the root filesystem as a fallback.
- Show individual models and memory capacities for mixed GPUs; use a shared capacity only when all GPUs match.
- Add regression coverage for disk selection, disk labels, and mixed GPU displays.
- Document the source beta, contribution process, private vulnerability reporting, and release workflow. Homebrew remains on the stable v0.2.2 release.
- Use local validation for releases and contributions; remove the GitHub CI workflow.

## 0.2.2

- Make the terminal layout regression independent of runner CPU labels and core counts, and verify that all mocked GPUs are present before checking compact-layout resize behavior.

## 0.2.1

- Restore terminal settings on shell exit, SIGTERM, and SIGHUP, and clean up shell and GPU-probe processes.
- Buffer terminal output with a bounded, nonblocking queue so a stalled terminal does not prevent shutdown handling.
- Preserve the shell cursor while changing scroll margins and painting the bar.
- Pause the bar and restore full terminal dimensions for alternate-screen programs; defer painting during incomplete control sequences and UTF-8 characters.
- Collect metrics in a background worker and bound GPU probes so a slow driver does not block terminal I/O.
- Calculate network rates from the elapsed sampling time.
- Fit metrics to narrow and short terminals, with Unicode-aware cell widths.
- Use platform-provided libc time types and handle local-time conversion failures.
- Add macOS and Linux CI, unit tests, and PTY regression coverage for debug and release binaries.
- Use locked builds and install project and dependency license notices alongside the executable.

## 0.2.0

- Add network download and upload throughput to the status bar.

## 0.1.0

- Initial release with CPU, RAM, GPU, and disk metrics.
