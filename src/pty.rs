use super::*;
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::AtomicUsize;

fn owned_fd(fd: i32) -> io::Result<OwnedFd> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn set_pty_size(fd: i32, rows: u16, cols: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
    }
}

pub(crate) fn get_tty_size(fd: i32) -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 && ws.ws_col > 0
    {
        (ws.ws_row, ws.ws_col)
    } else {
        (24, 80)
    }
}

// This is used only for best-effort screen cleanup after restoring termios.
// The independently opened terminal descriptor is always nonblocking.
fn write_cleanup(fd: i32, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n > 0 {
            data = &data[n as usize..];
        } else {
            break;
        }
    }
}

fn scroll_region(rows: u16) -> String {
    // DECSTBM needs distinct top/bottom margins; reset it for a one-row terminal.
    let sequence: String = if rows < 2 {
        "\x1b[r".into()
    } else {
        format!("\x1b[1;{rows}r")
    };
    // DECSTBM homes the cursor, including when resetting the margins.
    format!("\x1b7{sequence}\x1b8")
}

fn reserve_bottom(lines: u16) -> String {
    if lines == 0 {
        return String::new();
    }
    // IND preserves the column and scrolls only when it reaches the old bottom.
    // Moving back up leaves an upper cursor unchanged, or moves a bottom prompt
    // up together with its text before the new bar rows are cleared/painted.
    let mut frame = "\x1bD".repeat(lines as usize);
    write!(frame, "\x1b[{lines}A").unwrap();
    frame
}

fn clear_bar(rows: u16, height: u16) -> String {
    let mut frame = String::from("\x1b7");
    for row in rows.saturating_sub(height).saturating_add(1)..=rows {
        write!(frame, "\x1b[{row};1H\x1b[K").unwrap();
    }
    frame.push_str("\x1b8");
    frame
}

fn paint_bar(rows: u16, lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let inner = rows.saturating_sub(lines.len() as u16);
    let mut frame = String::from("\x1b7");
    write!(frame, "\x1b[1;{inner}r").unwrap();
    for (i, line) in lines.iter().enumerate() {
        write!(frame, "\x1b[{};1H{}", inner as usize + 1 + i, line).unwrap();
    }
    frame.push_str("\x1b8");
    frame
}

const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
}

impl Output {
    fn flush(&mut self, fd: i32) -> io::Result<()> {
        let (first, _) = self.bytes.as_slices();
        if first.is_empty() {
            return Ok(());
        }
        let n = unsafe { libc::write(fd, first.as_ptr().cast(), first.len()) };
        if n > 0 {
            self.bytes.drain(..n as usize);
        } else if n < 0 {
            let error = io::Error::last_os_error();
            if !matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                return Err(error);
            }
        } else {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }

    fn frame(&mut self, frame: &str) -> bool {
        if self.bytes.len() + frame.len() > OUTPUT_LIMIT {
            return false;
        }
        self.bytes.extend(frame.bytes());
        true
    }
}

#[derive(Default)]
enum ControlState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    String,
    StringEscape,
}

// Track control boundaries without rewriting the child's output. This avoids
// inserting a bar into a split escape/UTF-8 sequence and suspends it in the
// alternate screen used by editors, pagers and other full-screen programs.
#[derive(Default)]
struct ScreenState {
    control: ControlState,
    csi: Vec<u8>,
    utf8_remaining: u8,
    alternate: bool,
}

impl ScreenState {
    fn ready(&self) -> bool {
        matches!(self.control, ControlState::Ground) && self.utf8_remaining == 0
    }

    fn observe(&mut self, data: &[u8]) {
        for &byte in data {
            match self.control {
                ControlState::Ground => {
                    if self.utf8_remaining > 0 && (0x80..=0xbf).contains(&byte) {
                        self.utf8_remaining -= 1;
                        continue;
                    }
                    self.utf8_remaining = match byte {
                        0xc2..=0xdf => 1,
                        0xe0..=0xef => 2,
                        0xf0..=0xf4 => 3,
                        _ => 0,
                    };
                    if byte == 0x1b {
                        self.control = ControlState::Escape;
                    }
                }
                ControlState::Escape => {
                    self.control = match byte {
                        b'[' => {
                            self.csi.clear();
                            ControlState::Csi
                        }
                        b']' | b'P' | b'X' | b'^' | b'_' => ControlState::String,
                        0x20..=0x2f => ControlState::EscapeIntermediate,
                        0x1b => ControlState::Escape,
                        _ => ControlState::Ground,
                    };
                }
                ControlState::EscapeIntermediate => {
                    if (0x30..=0x7e).contains(&byte) {
                        self.control = ControlState::Ground;
                    }
                }
                ControlState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        if matches!(byte, b'h' | b'l') && self.csi.first() == Some(&b'?') {
                            for mode in self.csi[1..].split(|&b| b == b';') {
                                if matches!(mode, b"47" | b"1047" | b"1049") {
                                    self.alternate = byte == b'h';
                                }
                            }
                        }
                        self.control = ControlState::Ground;
                    } else if self.csi.len() < 128 {
                        self.csi.push(byte);
                    }
                }
                ControlState::String => {
                    if byte == 0x1b {
                        self.control = ControlState::StringEscape;
                    } else if byte == 0x07 {
                        self.control = ControlState::Ground;
                    }
                }
                ControlState::StringEscape => {
                    self.control = if byte == b'\\' || byte == 0x07 {
                        ControlState::Ground
                    } else if byte == 0x1b {
                        ControlState::StringEscape
                    } else {
                        ControlState::String
                    };
                }
            }
        }
    }
}

struct Terminal {
    fd: OwnedFd,
    original: libc::termios,
    rows: u16,
    bar_height: u16,
    painted_rows: u16,
    active: bool,
    alternate: bool,
}

impl Terminal {
    fn new(fd: i32) -> io::Result<Self> {
        let mut original = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // dup/F_SETFL would change the caller's shared open-file-description
        // flags. Reopen the same TTY instead, leaving inherited descriptors alone.
        let mut name = [0; 1024];
        let result = unsafe { libc::ttyname_r(fd, name.as_mut_ptr(), name.len()) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        let owned = owned_fd(unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        })?;
        Ok(Self {
            fd: owned,
            original,
            rows: get_tty_size(fd).0,
            bar_height: 0,
            painted_rows: get_tty_size(fd).0,
            active: false,
            alternate: false,
        })
    }

    fn enter_raw(&mut self) -> io::Result<()> {
        let mut raw = self.original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.active = true;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.active {
            // Restore input first, even when screen output or child cleanup fails.
            unsafe {
                libc::tcsetattr(self.fd.as_raw_fd(), libc::TCSANOW, &self.original);
            }
            let mut cleanup = String::new();
            if self.alternate {
                cleanup.push_str("\x1b[?1049l\x1b[?1047l\x1b[?47l");
            }
            cleanup.push_str(&scroll_region(self.rows));
            cleanup.push_str(&clear_bar(self.rows, self.bar_height));
            cleanup.push_str(RST);
            write_cleanup(self.fd.as_raw_fd(), cleanup.as_bytes());
        }
    }
}

struct Signals {
    terminate: Arc<AtomicUsize>,
    resize: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl Signals {
    fn new() -> io::Result<Self> {
        let mut signals = Self {
            terminate: Arc::new(AtomicUsize::new(0)),
            resize: Arc::new(AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for sig in [libc::SIGTERM, libc::SIGHUP] {
            signals
                .registrations
                .push(signal_hook::flag::register_usize(
                    sig,
                    Arc::clone(&signals.terminate),
                    sig as usize,
                )?);
        }
        signals.registrations.push(signal_hook::flag::register(
            libc::SIGWINCH,
            Arc::clone(&signals.resize),
        )?);
        Ok(signals)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

struct Shell {
    pid: i32,
    master: OwnedFd,
    status: Option<i32>,
    shutdown_signal: i32,
}

impl Shell {
    fn poll_exit(&mut self) -> io::Result<()> {
        if self.status.is_none() {
            let mut status = 0;
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if result > 0 {
                self.status = Some(status);
            } else if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn signal_groups(&self, signal: i32) {
        unsafe {
            let foreground = libc::tcgetpgrp(self.master.as_raw_fd());
            if foreground > 0 && foreground != self.pid {
                libc::kill(-foreground, signal);
            }
            libc::kill(-self.pid, signal);
            libc::kill(self.pid, signal);
        }
    }

    fn finish(&mut self) {
        if self.status.is_some() {
            return;
        }
        self.signal_groups(self.shutdown_signal);
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.status.is_none() && Instant::now() < deadline {
            if self.poll_exit().is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if self.status.is_none() {
            self.signal_groups(libc::SIGKILL);
            let mut status = 0;
            loop {
                if unsafe { libc::waitpid(self.pid, &mut status, 0) } > 0 {
                    self.status = Some(status);
                    break;
                }
                if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.finish();
    }
}

fn spawn_shell(terminal: &Terminal, cols: u16) -> io::Result<Shell> {
    let master =
        owned_fd(unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) })?;
    let master_fd = master.as_raw_fd();
    if unsafe { libc::grantpt(master_fd) } < 0 || unsafe { libc::unlockpt(master_fd) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let path = unsafe { libc::ptsname(master_fd) };
    if path.is_null() {
        return Err(io::Error::last_os_error());
    }
    let slave =
        owned_fd(unsafe { libc::open(path, libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) })?;
    let slave_fd = slave.as_raw_fd();
    // Preserve the user's erase key, control characters and other shell settings.
    if unsafe { libc::tcsetattr(slave_fd, libc::TCSANOW, &terminal.original) } < 0 {
        return Err(io::Error::last_os_error());
    }
    set_pty_size(master_fd, terminal.rows, cols);
    let name = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let name = std::ffi::CString::new(name).map_err(|_| io::Error::other("invalid SHELL"))?;
    env::set_var("SBAR", "1");
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // Start workers only after this fork. No allocation/locks in the child.
        unsafe {
            libc::close(master_fd);
            if libc::setsid() < 0 || libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                libc::_exit(1);
            }
            for fd in 0..=2 {
                if libc::dup2(slave_fd, fd) < 0 {
                    libc::_exit(1);
                }
            }
            if slave_fd > 2 {
                libc::close(slave_fd);
            }
            let argv = [name.as_ptr(), std::ptr::null()];
            libc::execvp(name.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    let shell = Shell {
        pid,
        master,
        status: None,
        shutdown_signal: libc::SIGHUP,
    };
    let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(shell)
}

fn run() -> io::Result<i32> {
    let mut terminal = Terminal::new(0)?;
    let fd = terminal.fd.as_raw_fd();
    let mut cols = get_tty_size(fd).1;
    let mut shell = spawn_shell(&terminal, cols)?;
    let signals = Signals::new()?;
    // The child gets control-key bytes through its PTY, not parent signals.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
    terminal.enter_raw()?;
    let worker = metrics::Worker::start();
    let mut latest = None;
    let mut pending_input: VecDeque<u8> = VecDeque::new();
    let mut output = Output::default();
    let mut screen = ScreenState::default();
    let mut output_finished = false;
    let mut repaint = false;
    let mut resize = false;
    let mut buffer = [0u8; 16384];

    let outcome = (|| -> io::Result<()> {
        loop {
            let signal = signals.terminate.load(Ordering::Relaxed) as i32;
            if signal != 0 {
                shell.shutdown_signal = signal;
                break;
            }
            shell.poll_exit()?;
            if signals.resize.swap(false, Ordering::Relaxed) {
                (terminal.rows, cols) = get_tty_size(fd);
                resize = true;
                repaint = true;
            }
            while let Ok(sample) = worker.receiver.try_recv() {
                latest = Some(sample);
                repaint = true;
            }
            output.flush(fd)?;
            if screen.ready() && screen.alternate != terminal.alternate {
                terminal.alternate = screen.alternate;
                resize = true;
                repaint = true;
            }
            if repaint && screen.ready() && !output_finished {
                let lines = latest
                    .as_ref()
                    .map(|m| render_for_terminal(m, terminal.rows, cols))
                    .unwrap_or_default();
                let height = if terminal.alternate {
                    0
                } else {
                    lines.len() as u16
                };
                let inner = terminal.rows.saturating_sub(height).max(1);
                let change_size = (!terminal.alternate && height != terminal.bar_height) || resize;
                let mut frame = String::new();
                if change_size {
                    if !terminal.alternate {
                        if resize {
                            // A physical resize or alternate-screen return may
                            // reset margins. Start from the whole current screen
                            // and protect both the previous and new bar area.
                            let old_inner = if terminal.bar_height == 0 {
                                terminal.rows
                            } else {
                                terminal
                                    .painted_rows
                                    .saturating_sub(terminal.bar_height)
                                    .max(1)
                            };
                            let safe_bottom = inner.min(old_inner);
                            frame.push_str(&scroll_region(terminal.rows));
                            frame.push_str(&reserve_bottom(terminal.rows - safe_bottom));
                            frame.push_str(&clear_bar(terminal.rows, terminal.rows - safe_bottom));
                        } else {
                            if height > terminal.bar_height {
                                let old_inner =
                                    terminal.rows.saturating_sub(terminal.bar_height).max(1);
                                frame.push_str(&scroll_region(old_inner));
                                frame.push_str(&reserve_bottom(height - terminal.bar_height));
                            }
                            frame.push_str(&clear_bar(terminal.rows, terminal.bar_height));
                        }
                    }
                    frame.push_str(&scroll_region(inner));
                }
                if !terminal.alternate {
                    frame.push_str(&paint_bar(terminal.rows, &lines));
                }
                if output.frame(&frame) {
                    if change_size {
                        set_pty_size(shell.master.as_raw_fd(), inner, cols);
                    }
                    if !terminal.alternate {
                        terminal.bar_height = height;
                        terminal.painted_rows = terminal.rows;
                    }
                    repaint = false;
                    resize = false;
                }
            }

            if output_finished && output.bytes.is_empty() {
                break;
            }
            // Backpressure applies to shell reads as well as writes: never let a
            // stopped outer terminal turn the output queue into unbounded memory.
            let can_read_output =
                !output_finished && output.bytes.len() + buffer.len() <= OUTPUT_LIMIT;

            let mut fds = [
                libc::pollfd {
                    fd,
                    events: (if pending_input.len() < 65536 && !output_finished {
                        libc::POLLIN
                    } else {
                        0
                    }) | if output.bytes.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
                    revents: 0,
                },
                libc::pollfd {
                    fd: if output_finished || (!can_read_output && pending_input.is_empty()) {
                        -1
                    } else {
                        shell.master.as_raw_fd()
                    },
                    events: (if can_read_output { libc::POLLIN } else { 0 })
                        | if pending_input.is_empty() {
                            0
                        } else {
                            libc::POLLOUT
                        },
                    revents: 0,
                },
            ];
            let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 50) };
            if result < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error());
            }
            if signals.terminate.load(Ordering::Relaxed) != 0 {
                continue;
            }
            if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                break;
            }
            if fds[0].revents & libc::POLLOUT != 0 {
                output.flush(fd)?;
            }
            if fds[0].revents & libc::POLLIN != 0 {
                let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                if n == 0 {
                    break;
                }
                if n > 0 {
                    pending_input.extend(&buffer[..n as usize]);
                }
            }
            if !pending_input.is_empty() {
                let input = pending_input.make_contiguous();
                let n = unsafe {
                    libc::write(shell.master.as_raw_fd(), input.as_ptr().cast(), input.len())
                };
                if n > 0 {
                    pending_input.drain(..n as usize);
                } else if n < 0
                    && !matches!(
                        io::Error::last_os_error().kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    )
                {
                    // A shell can close its input before its final output has
                    // been read. Keep draining output rather than dropping it.
                    pending_input.clear();
                }
            }
            if can_read_output && fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                let n = unsafe {
                    libc::read(
                        shell.master.as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                };
                if n > 0 {
                    let data = &buffer[..n as usize];
                    screen.observe(data);
                    output.bytes.extend(data);
                } else if n == 0
                    || !matches!(
                        io::Error::last_os_error().kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    )
                {
                    output_finished = true;
                }
            } else if can_read_output
                && (shell.status.is_some()
                    || fds[1].revents & (libc::POLLERR | libc::POLLNVAL) != 0)
            {
                output_finished = true;
            }
        }
        Ok(())
    })();

    // Restore the real terminal before waiting for workers or child processes.
    drop(terminal);
    drop(worker);
    shell.finish();
    outcome?;
    let signal = signals.terminate.load(Ordering::Relaxed) as i32;
    if signal != 0 {
        return Ok(128 + signal);
    }
    let status = shell.status.unwrap_or(0);
    Ok(if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        128 + libc::WTERMSIG(status)
    })
}

pub(crate) fn cmd_wrap() {
    let code = run().unwrap_or_else(|error| {
        eprintln!("sbar: {error}");
        1
    });
    process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::ScreenState;

    #[test]
    fn alternate_screen_modes_survive_every_chunk_boundary() {
        for mode in [47, 1047, 1049] {
            let enter = format!("\x1b[?{mode}h");
            let leave = format!("\x1b[?{mode}l");
            for split in 0..=enter.len() {
                let mut screen = ScreenState::default();
                screen.observe(&enter.as_bytes()[..split]);
                if split > 0 && split < enter.len() {
                    assert!(!screen.ready());
                    assert!(!screen.alternate);
                }
                screen.observe(&enter.as_bytes()[split..]);
                assert!(screen.ready());
                assert!(screen.alternate);
                screen.observe(leave.as_bytes());
                assert!(screen.ready());
                assert!(!screen.alternate);
            }
        }
    }

    #[test]
    fn control_strings_and_utf8_hold_frames_until_complete() {
        for sequence in [
            "\x1b]0;window title\x07",
            "\x1b]0;window title\x1b\\",
            "\x1bPqdata\x1b\\",
            "\x1b(B",
            "界",
            "🦀",
        ] {
            for split in 1..sequence.len() {
                let mut screen = ScreenState::default();
                screen.observe(&sequence.as_bytes()[..split]);
                assert!(!screen.ready(), "{sequence:?} at byte {split}");
                screen.observe(&sequence.as_bytes()[split..]);
                assert!(screen.ready(), "{sequence:?} at byte {split}");
            }
        }
        let mut screen = ScreenState::default();
        screen.observe(b"\x1b]0;not a command: \x1b[?1049h\x07");
        assert!(screen.ready());
        assert!(!screen.alternate);
    }
}
