//! Bounded GPU probes. A slow driver must not stall the terminal or hold its PTY open.
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

struct Probe {
    child: Child,
    reaped: bool,
}

impl Drop for Probe {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        // Kill the whole probe group, including descendants holding stdout open.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

pub(crate) fn output(
    command: &mut Command,
    timeout: Duration,
    stop: &AtomicBool,
) -> io::Result<Vec<u8>> {
    if stop.load(Ordering::Relaxed) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let mut child = Probe {
        child: command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?,
        reaped: false,
    };
    let mut stdout = child.child.stdout.take().unwrap();
    let fd = stdout.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let deadline = Instant::now() + timeout;
    let mut result = Vec::new();
    let mut eof = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut buffer = [0; 8192];
        match stdout.read(&mut buffer) {
            Ok(0) => eof = true,
            Ok(n) => {
                // ioreg can be verbose; keep even unexpected output bounded.
                if result.len() + n > 4 * 1024 * 1024 {
                    return Err(io::Error::other("GPU probe output exceeds 4 MiB"));
                }
                result.extend_from_slice(&buffer[..n]);
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        // Keep the leader unreaped until stdout closes, reserving its PID/group
        // while descendants may still need to be killed on timeout.
        if eof {
            if let Some(status) = child.child.try_wait()? {
                child.reaped = true;
                if !status.success() {
                    return Err(io::Error::other("GPU probe failed"));
                }
                return Ok(result);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_output_larger_than_a_pipe() {
        let result = output(
            Command::new("/bin/sh").args(["-c", "dd if=/dev/zero bs=1024 count=128 2>/dev/null"]),
            Duration::from_secs(5),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.len(), 128 * 1024);
    }

    #[test]
    fn times_out_even_when_a_descendant_keeps_stdout_open() {
        let start = Instant::now();
        let error = output(
            Command::new("/bin/sh").args(["-c", "sleep 10 & exit 0"]),
            Duration::from_millis(100),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_stops_an_active_probe() {
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                stop.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            let error = output(
                Command::new("/bin/sh").args(["-c", "sleep 10"]),
                Duration::from_secs(20),
                &stop,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Interrupted);
            assert!(start.elapsed() < Duration::from_secs(2));
        });
    }
}
