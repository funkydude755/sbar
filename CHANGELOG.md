# Changelog

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
