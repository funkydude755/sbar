use std::env;
use std::fmt::Write;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[cfg(unix)]
mod command;
mod metrics;
#[cfg(unix)]
mod pty;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Colors ───────────────────────────────────────────────────────────────────

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RST: &str = "\x1b[0m";
const BG: &str = "\x1b[48;5;235m";

const COLOR_CPU: &str = CYAN;
const COLOR_RAM: &str = MAGENTA;
const COLOR_GPU: &str = GREEN;
const COLOR_DISK: &str = YELLOW;
const COLOR_VRAM: &str = MAGENTA;
const COLOR_NET: &str = CYAN;

const BAR_FILL: char = '█';
const BAR_EMPTY: char = '░';

// ── Metrics ──────────────────────────────────────────────────────────────────

struct GpuInfo {
    pct: f64,
    label: String,
    mem_used_mb: u64,
    mem_total_mb: u64,
}

struct Metrics {
    cpu_pct: f64,
    cpu_cores: usize,
    cpu_name: String,
    per_core_pcts: Vec<f64>,
    ram_pct: f64,
    ram_used_gb: u64,
    ram_total_gb: u64,
    gpus: Vec<GpuInfo>,
    disk_pct: Option<f64>,
    disk_mount_label: &'static str,
    disk_used_gb: u64,
    disk_total_gb: u64,
    net_down_bps: f64,
    net_up_bps: f64,
}

// ── Bar renderer ─────────────────────────────────────────────────────────────

fn render_bar(pct: f64, color: &str, width: usize, bg: &str) -> String {
    let filled = ((pct * width as f64) / 100.0).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    let fill_color = color;
    let rst_bg = format!("{RST}{bg}");
    let mut out = String::with_capacity(width * 4 + 40);
    write!(out, "{bg}{fill_color}").unwrap();
    for _ in 0..filled {
        out.push(BAR_FILL);
    }
    write!(out, "{rst_bg}{DIM}").unwrap();
    for _ in 0..empty {
        out.push(BAR_EMPTY);
    }
    write!(out, "{rst_bg}").unwrap();
    out
}

fn color_pct(pct: f64, default_color: &str) -> &str {
    if pct >= 90.0 {
        RED
    } else if pct >= 75.0 {
        YELLOW
    } else {
        default_color
    }
}

// ── GPU collection ───────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_gpu_info(stop: &AtomicBool) -> Vec<GpuInfo> {
    let output = match command::output(
        process::Command::new("ioreg").args(["-r", "-c", "AGXAccelerator", "-w", "0"]),
        Duration::from_millis(750),
        stop,
    ) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output);
    let mut gpus = Vec::new();
    let marker = "\"Device Utilization %\"=";
    let mut search_from = 0;
    while let Some(stats_start) = text[search_from..].find("\"PerformanceStatistics\" = {") {
        let abs_start = search_from + stats_start;
        if let Some(block_end) = text[abs_start..].find('}') {
            let block = &text[abs_start..abs_start + block_end];
            if let Some(pos) = block.find(marker) {
                let num_start = pos + marker.len();
                let num_str: String = block[num_start..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(pct) = num_str.parse::<f64>() {
                    gpus.push(GpuInfo {
                        pct,
                        label: String::new(),
                        mem_used_mb: 0,
                        mem_total_mb: 0,
                    });
                }
            }
            search_from = abs_start + block_end + 1;
        } else {
            break;
        }
    }
    gpus
}

#[cfg(target_os = "linux")]
fn get_gpu_info(stop: &AtomicBool) -> Vec<GpuInfo> {
    let output = match command::output(
        process::Command::new("nvidia-smi").args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ]),
        Duration::from_millis(750),
        stop,
    ) {
        Ok(o) => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, ',').collect();
        if parts.len() == 4 {
            let pct: f64 = parts[1].trim().parse().unwrap_or(0.0);
            let mem_used: u64 = parts[2].trim().parse().unwrap_or(0);
            let mem_total: u64 = parts[3].trim().parse().unwrap_or(0);
            let name = parts[0].trim().to_string();
            gpus.push(GpuInfo {
                pct,
                label: name,
                mem_used_mb: mem_used,
                mem_total_mb: mem_total,
            });
        }
    }
    gpus
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_gpu_info(_stop: &AtomicBool) -> Vec<GpuInfo> {
    Vec::new()
}

// ── Collect metrics ──────────────────────────────────────────────────────────

fn system_disk_usage<'a>(
    disks: impl Iterator<Item = (&'a std::path::Path, u64, u64)>,
) -> Option<(&'static str, u64, u64)> {
    disks
        .filter_map(|(mount, total, available)| {
            let (priority, label) = match mount.to_str()? {
                // The writable Data volume holds user files on modern macOS.
                #[cfg(target_os = "macos")]
                "/System/Volumes/Data" => (0, "Data"),
                "/" => (1, "/"),
                _ => return None,
            };
            Some((priority, label, total, available))
        })
        .min_by_key(|disk| disk.0)
        .map(|(_, label, total, available)| (label, total, available))
}

fn collect_metrics(
    sys: &System,
    disks: &Disks,
    gpus: Vec<GpuInfo>,
    cpu_name: &str,
    cpu_cores: usize,
    net_down_bps: f64,
    net_up_bps: f64,
) -> Metrics {
    let cpu_pct = sys.global_cpu_usage() as f64;
    let per_core_pcts: Vec<f64> = sys.cpus().iter().map(|c| c.cpu_usage() as f64).collect();

    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let ram_pct = if total_mem > 0 {
        (used_mem as f64 / total_mem as f64) * 100.0
    } else {
        0.0
    };

    let (disk_pct, disk_mount_label, disk_used_gb, disk_total_gb) = system_disk_usage(
        disks
            .list()
            .iter()
            .map(|d| (d.mount_point(), d.total_space(), d.available_space())),
    )
    .map(|(label, t, available)| {
        let u = t.saturating_sub(available);
        let pct = if t > 0 {
            (u as f64 / t as f64) * 100.0
        } else {
            0.0
        };
        (Some(pct), label, u / 1_073_741_824, t / 1_073_741_824)
    })
    .unwrap_or((None, "", 0, 0));

    Metrics {
        cpu_pct,
        cpu_cores,
        cpu_name: cpu_name.to_string(),
        per_core_pcts,
        ram_pct,
        ram_used_gb: used_mem / 1_073_741_824,
        ram_total_gb: total_mem / 1_073_741_824,
        gpus,
        disk_pct,
        disk_mount_label,
        disk_used_gb,
        disk_total_gb,
        net_down_bps,
        net_up_bps,
    }
}

/// Format bytes/sec into a human-readable rate string
fn format_rate(bps: f64) -> String {
    if bps >= 1_000_000_000.0 {
        format!("{:.1} GB/s", bps / 1_000_000_000.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.0} KB/s", bps / 1_000.0)
    } else {
        format!("{:.0}  B/s", bps)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Shorten CPU brand string: "Intel(R) Xeon(R) Platinum 8259CL CPU @ 2.50GHz" → "Xeon 8259CL"
fn shorten_cpu_name(name: &str) -> String {
    let s = name.replace("(R)", "").replace("(TM)", "");
    let s = s.replace("Intel ", "").replace("AMD ", "");
    // Remove "CPU" and everything after "@"
    let s = if let Some(pos) = s.find(" CPU") {
        &s[..pos]
    } else {
        &s
    };
    let s = if let Some(pos) = s.find(" @") {
        &s[..pos]
    } else {
        s
    };
    let s = s
        .replace("Platinum ", "")
        .replace("Gold ", "")
        .replace("Silver ", "");
    s.trim().to_string()
}

// ── Layout engine ────────────────────────────────────────────────────────────
//
// Two-panel layout:
//   Left panel:  CPU total bar (long) + per-core grid underneath
//   Right panel: RAM bar + clock (line 1), DISK bar (line 2)
//   Full width:  GPU utilization row, GPU VRAM row (aligned)

/// Build a right-panel line: "{LABEL} {NNN}% {bar} {suffix}"
fn render_right_cell(
    label: &str,
    color: &str,
    pct: f64,
    bar_w: usize,
    suffix: &str,
    bg: &str,
) -> (String, usize) {
    let rb = format!("{RST}{bg}");
    let mut o = String::new();
    let show = bar_w >= 3;
    write!(o, "{BOLD}{color}{label}{rb} {color}{pct:3.0}%{rb}").unwrap();
    let mut vis = label.width() + 1 + 4; // "LABEL NNN%"
    if show {
        write!(o, " ").unwrap();
        o.push_str(&render_bar(pct, color, bar_w, bg));
        vis += 1 + bar_w;
    }
    if !suffix.is_empty() {
        write!(o, " {DIM}{suffix}{rb}").unwrap();
        vis += 1 + suffix.width();
    }
    (o, vis)
}

/// Compute shared GPU column layout so GPU + MEM rows align perfectly.
fn gpu_col_layout(n: usize, suffix_width: usize, cols: usize) -> (usize, usize, bool) {
    if n == 0 {
        return (0, 0, false);
    }
    let d = if n == 1 {
        0
    } else if n > 9 {
        2
    } else {
        1
    };
    let per_fixed = 1 + 3 + d + 1 + 4 + 1; // " GPU[n] NNN% "
    let seps = if n > 1 { (n - 1) * 3 } else { 0 };
    let suffix_total = if suffix_width > 0 {
        3 + suffix_width
    } else {
        0
    };
    let total_fixed = per_fixed * n + seps + suffix_total;
    let bar_budget = cols.saturating_sub(total_fixed);
    let bw = bar_budget / n;
    let extra = bar_budget % n;
    (bw, extra, bw >= 3)
}

fn gpu_model_label(gpus: &[GpuInfo]) -> String {
    let Some(first) = gpus.first() else {
        return String::new();
    };
    if gpus.iter().all(|gpu| gpu.label == first.label) {
        return first.label.clone();
    }
    gpus.iter()
        .enumerate()
        .map(|(i, gpu)| format!("GPU{i} {}", gpu.label))
        .collect::<Vec<_>>()
        .join(", ")
}

fn gpu_vram_label(gpus: &[GpuInfo]) -> String {
    let Some(first) = gpus.first() else {
        return String::new();
    };
    let capacity = |mb: u64| {
        if mb == 0 {
            "unknown".to_string()
        } else {
            format!("{:.0}G", mb as f64 / 1024.0)
        }
    };
    if gpus
        .iter()
        .all(|gpu| gpu.mem_total_mb == first.mem_total_mb)
    {
        let label = capacity(first.mem_total_mb);
        return if gpus.len() > 1 {
            format!("{label} ea")
        } else {
            label
        };
    }
    gpus.iter()
        .enumerate()
        .map(|(i, gpu)| format!("MEM{i} {}", capacity(gpu.mem_total_mb)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// GPU utilization row.
fn render_gpu_util_row(gpus: &[GpuInfo], bw: usize, extra: usize, show: bool, bg: &str) -> String {
    let rb = format!("{RST}{bg}");
    let sep = format!("{rb} {DIM}│{rb} ");
    let model = gpu_model_label(gpus);
    let mut o = String::new();
    write!(o, "{bg}").unwrap();
    for (i, gpu) in gpus.iter().enumerate() {
        if i > 0 {
            o.push_str(&sep);
        } else {
            o.push(' ');
        }
        if gpus.len() == 1 {
            write!(
                o,
                "{BOLD}{COLOR_GPU}GPU{rb} {COLOR_GPU}{:3.0}%{rb} ",
                gpu.pct
            )
            .unwrap();
        } else {
            write!(
                o,
                "{BOLD}{COLOR_GPU}GPU{i}{rb} {COLOR_GPU}{:3.0}%{rb} ",
                gpu.pct
            )
            .unwrap();
        }
        if show {
            let w = if i < extra { bw + 1 } else { bw };
            o.push_str(&render_bar(gpu.pct, COLOR_GPU, w, bg));
        }
    }
    if !model.is_empty() {
        write!(o, "{sep}{DIM}{model}{rb}").unwrap();
    }
    if !bg.is_empty() {
        write!(o, "\x1b[K{RST}").unwrap();
    }
    o
}

/// GPU VRAM row (aligned to GPU row)
fn render_gpu_vram_row(gpus: &[GpuInfo], bw: usize, extra: usize, show: bool, bg: &str) -> String {
    let rb = format!("{RST}{bg}");
    let sep = format!("{rb} {DIM}│{rb} ");
    let vram_label = gpu_vram_label(gpus);
    let mut o = String::new();
    write!(o, "{bg}").unwrap();
    for (i, gpu) in gpus.iter().enumerate() {
        let mp = if gpu.mem_total_mb > 0 {
            (gpu.mem_used_mb as f64 / gpu.mem_total_mb as f64) * 100.0
        } else {
            0.0
        };
        if i > 0 {
            o.push_str(&sep);
        } else {
            o.push(' ');
        }
        if gpus.len() == 1 {
            write!(o, "{BOLD}{COLOR_VRAM}MEM{rb} {COLOR_VRAM}{mp:3.0}%{rb} ").unwrap();
        } else {
            write!(o, "{BOLD}{COLOR_VRAM}MEM{i}{rb} {COLOR_VRAM}{mp:3.0}%{rb} ").unwrap();
        }
        if show {
            let w = if i < extra { bw + 1 } else { bw };
            o.push_str(&render_bar(mp, COLOR_VRAM, w, bg));
        }
    }
    write!(o, "{sep}{DIM}{vram_label}{rb}").unwrap();
    if !bg.is_empty() {
        write!(o, "\x1b[K{RST}").unwrap();
    }
    o
}

/// Preserve complete SGR sequences while clipping content to terminal cells.
fn fit_line(line: &str, cols: usize, bg: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    let mut width = 0;
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.next() == Some('[') {
                let mut sequence = String::from("\x1b[");
                for next in chars.by_ref() {
                    sequence.push(next);
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                // Only retain styling; device names cannot inject cursor commands.
                if sequence.ends_with('m') {
                    out.push_str(&sequence);
                }
            }
        } else if !ch.is_control() {
            let cells = ch.width().unwrap_or(0);
            if width + cells > cols {
                break;
            }
            out.push(ch);
            width += cells;
        }
    }
    if !bg.is_empty() {
        for _ in width..cols {
            out.push(' ');
        }
    }
    out.push_str(RST);
    out
}

fn render_compact(m: &Metrics, cols: usize, bg: &str) -> Vec<String> {
    let mut lines = vec![format!(
        " {COLOR_CPU}CPU {:3.0}%{RST}  {COLOR_RAM}RAM {:3.0}%{RST}",
        m.cpu_pct, m.ram_pct
    )];
    if let Some(disk) = m.disk_pct {
        lines.push(format!(
            " {COLOR_DISK}DISK {} {disk:3.0}%{RST} {}/{}G",
            m.disk_mount_label, m.disk_used_gb, m.disk_total_gb
        ));
    }
    if m.net_down_bps > 0.0 || m.net_up_bps > 0.0 {
        let down = format_rate(m.net_down_bps);
        let up = format_rate(m.net_up_bps);
        let network = format!(" Down {down}  Up {up}");
        if network.width() <= cols {
            lines.push(network);
        } else {
            lines.extend([format!(" Down {down}"), format!(" Up {up}")]);
        }
    }
    for (i, gpu) in m.gpus.iter().enumerate() {
        lines.push(format!(" {COLOR_GPU}GPU{i} {:3.0}%{RST}", gpu.pct));
    }
    lines
        .into_iter()
        .map(|line| fit_line(&format!("{bg}{line}"), cols, bg))
        .collect()
}

fn render_lines(m: &Metrics, cols: usize, bg: &str) -> Vec<String> {
    let rb = format!("{RST}{bg}");

    // ── Right panel dimensions ───────────────────────────────────────────
    let ram_sfx = format!("{}/{}G", m.ram_used_gb, m.ram_total_gb);
    let disk_sfx = m
        .disk_pct
        .map(|_| format!("{}/{}G", m.disk_used_gb, m.disk_total_gb));
    let disk_label = format!("DISK {}", m.disk_mount_label);
    let clock = chrono_time();

    // Reserve enough width for the disk's mount label as well as the RAM label.
    // RAM  line suffix: " sfx  clock" → 1 + ram_sfx + 2 + 8
    // DISK line suffix: " sfx"        → 1 + disk_sfx
    let ram_suffix_vis = 1 + ram_sfx.len() + 2 + clock.len();
    let disk_suffix_vis = disk_sfx.as_ref().map(|s| 1 + s.len()).unwrap_or(0);
    let right_prefix = if m.disk_pct.is_some() {
        disk_label.width().max(4) + 6
    } else {
        10
    };
    let right_suffix_max = ram_suffix_vis.max(disk_suffix_vis);
    let down = format_rate(m.net_down_bps);
    let up = format_rate(m.net_up_bps);
    let has_network = m.net_down_bps > 0.0 || m.net_up_bps > 0.0;
    // Account for the minimum field widths used by the network row.
    let network_width = 1 + 8 + 1 + down.len().max(10) + 1 + 6 + 1 + up.len().max(10);
    let right_fixed =
        (right_prefix + right_suffix_max).max(if has_network { network_width } else { 0 });

    // ── Left panel dimensions ────────────────────────────────────────────
    let cpu_short = shorten_cpu_name(&m.cpu_name);
    let cpu_sfx = format!("{} {}c", cpu_short, m.cpu_cores);
    // " CPU NNN% [bar] sfx" → 1 + 3 + 1 + 4 + 1 + bar + 1 + sfx
    let left_prefix = 1 + 3 + 1 + 4 + 1; // " CPU NNN% "
    let left_suffix = 1 + cpu_sfx.width(); // " sfx"
    let left_fixed = left_prefix + left_suffix;

    // ── Split columns ────────────────────────────────────────────────────
    let sep_w = 3; // " │ "
    let total_fixed = left_fixed + sep_w + right_fixed;
    if cols < total_fixed {
        return render_compact(m, cols, bg);
    }
    let total_bar_budget = cols.saturating_sub(total_fixed);

    // Give ~55% to CPU bar, ~45% to RAM/DISK bars
    let right_extra = total_bar_budget * 45 / 100;
    let left_bar_w = total_bar_budget.saturating_sub(right_extra);
    let left_w = left_fixed + left_bar_w;
    let right_w = right_fixed + right_extra;
    let right_bar_w = right_w.saturating_sub(right_prefix + right_suffix_max);

    // ── Build left panel lines ───────────────────────────────────────────
    // Line 0: CPU bar
    let mut left_lines: Vec<(String, usize)> = Vec::new();
    {
        let mut o = String::new();
        write!(
            o,
            " {BOLD}{COLOR_CPU}CPU{rb} {COLOR_CPU}{:3.0}%{rb}",
            m.cpu_pct
        )
        .unwrap();
        let mut vis = left_prefix - 1;
        if left_bar_w >= 3 {
            write!(o, " ").unwrap();
            o.push_str(&render_bar(m.cpu_pct, COLOR_CPU, left_bar_w, bg));
            vis += 1 + left_bar_w;
        }
        write!(o, " {DIM}{cpu_sfx}{rb}").unwrap();
        vis += left_suffix;
        left_lines.push((o, vis));
    }

    // Lines 1+: core grid
    let nd = m.per_core_pcts.len().saturating_sub(1).to_string().len();
    let ew = nd + 6; // visible width per entry
    let per_row = (left_w / ew).max(1);

    for cs in (0..m.per_core_pcts.len()).step_by(per_row) {
        let ce = (cs + per_row).min(m.per_core_pcts.len());
        let entries = ce - cs;
        let mut o = String::new();
        for i in cs..ce {
            let p = m.per_core_pcts[i];
            let c = color_pct(p, COLOR_CPU);
            write!(o, " {DIM}{i:>nd$}{rb} {c}{p:3.0}{rb}{DIM}%{rb}").unwrap();
        }
        left_lines.push((o, entries * ew));
    }

    // ── Build right panel lines ──────────────────────────────────────────
    let mut right_lines: Vec<(String, usize)> = Vec::new();

    // RAM + clock
    {
        let sfx = format!("{ram_sfx}  {clock}");
        let (content, vis) = render_right_cell(" RAM", COLOR_RAM, m.ram_pct, right_bar_w, &sfx, bg);
        right_lines.push((content, vis));
    }

    // DISK
    if let (Some(dp), Some(ref ds)) = (m.disk_pct, &disk_sfx) {
        let (content, vis) = render_right_cell(&disk_label, COLOR_DISK, dp, right_bar_w, ds, bg);
        right_lines.push((content, vis));
    }

    // NET
    if has_network {
        let net_text = format!(
            " {BOLD}{COLOR_NET}Download{rb} {COLOR_NET}{down:>10}{rb} {BOLD}{COLOR_NET}Upload{rb} {COLOR_NET}{up:>10}{rb}"
        );
        right_lines.push((net_text, network_width));
    }

    // ── Combine left + right into output lines ──────────────────────────
    let n_panel = left_lines.len().max(right_lines.len());
    let mut lines = Vec::new();

    for i in 0..n_panel {
        let mut line = String::with_capacity(cols * 4);
        write!(line, "{bg}").unwrap();

        // Left content
        if let Some((ref content, vis)) = left_lines.get(i) {
            line.push_str(content);
            let pad = left_w.saturating_sub(*vis);
            for _ in 0..pad {
                line.push(' ');
            }
        } else {
            for _ in 0..left_w {
                line.push(' ');
            }
        }

        // Separator
        write!(line, " {DIM}│{rb} ").unwrap();

        // Right content
        if let Some((ref content, vis)) = right_lines.get(i) {
            line.push_str(content);
            let pad = right_w.saturating_sub(*vis);
            for _ in 0..pad {
                line.push(' ');
            }
        }

        if !bg.is_empty() {
            write!(line, "\x1b[K{RST}").unwrap();
        }
        lines.push(line);
    }
    // ── GPU rows (full width) ────────────────────────────────────────────
    if !m.gpus.is_empty() {
        let n = m.gpus.len();
        let model = gpu_model_label(&m.gpus);
        let vram_label = gpu_vram_label(&m.gpus);
        let has_vram = m.gpus.iter().any(|g| g.mem_total_mb > 0);
        let suffix_w = if has_vram {
            model.width().max(vram_label.len())
        } else {
            model.width()
        };
        let (bw, extra, show) = gpu_col_layout(n, suffix_w, cols);
        lines.push(render_gpu_util_row(&m.gpus, bw, extra, show, bg));
        if has_vram {
            lines.push(render_gpu_vram_row(&m.gpus, bw, extra, show, bg));
        }
    }

    lines
        .into_iter()
        .map(|line| fit_line(&line, cols, bg))
        .collect()
}

fn render_for_terminal(m: &Metrics, rows: u16, cols: u16) -> Vec<String> {
    let budget = rows.saturating_sub(2) as usize;
    let mut lines = render_lines(m, cols as usize, BG);
    if lines.len() > budget {
        lines = render_compact(m, cols as usize, BG);
    }
    lines.truncate(budget);
    lines
}

fn chrono_time() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let local_secs = secs as i64 + local_utc_offset();
    let time_of_day = ((local_secs % 86400) + 86400) % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

fn local_utc_offset() -> i64 {
    #[cfg(unix)]
    {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        local_utc_offset_at(seconds).unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(unix)]
fn local_utc_offset_at(seconds: u64) -> Option<i64> {
    let time = libc::time_t::try_from(seconds).ok()?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    let tm = unsafe {
        if libc::localtime_r(&time, tm.as_mut_ptr()).is_null() {
            return None;
        }
        tm.assume_init()
    };
    // C long is 32 bits on some supported Unix targets and 64 bits on others.
    #[allow(clippy::unnecessary_cast)]
    let offset = tm.tm_gmtoff as i64;
    Some(offset)
}

fn cmd_once() {
    let mut collector = metrics::Collector::new();
    thread::sleep(Duration::from_millis(500));
    let m = collector.collect(&AtomicBool::new(false));
    #[cfg(unix)]
    let cols = {
        let (_, c) = pty::get_tty_size(2);
        c as usize
    };
    #[cfg(not(unix))]
    let cols = 80usize;
    for line in &render_lines(&m, cols, "") {
        println!("{}", line);
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match cmd {
        "--once" | "once" => cmd_once(),
        "--version" | "-v" | "version" => println!("sbar {}", VERSION),
        "--help" | "-h" | "help" => {
            println!(
                r#"sbar — persistent status bar for your terminal

Usage:
  sbar                     Launch shell with status bar
  sbar --once              Print metrics once and exit

Options:
  -v, --version            Print version
  -h, --help               Show this help"#
            );
        }
        "" | "wrap" => {
            #[cfg(unix)]
            pty::cmd_wrap();
            #[cfg(not(unix))]
            {
                eprintln!("sbar wrap requires a Unix system");
                process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown command: {}", other);
            eprintln!("Run 'sbar --help' for usage.");
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn local_time_rejects_unrepresentable_timestamps() {
        assert!(local_utc_offset_at(0).is_some());
        assert_eq!(local_utc_offset_at(u64::MAX), None);
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn local_time_handles_libc_range_errors() {
        assert_eq!(local_utc_offset_at(i64::MAX as u64), None);
    }

    fn sample(cores: usize, network: bool, gpus: usize) -> Metrics {
        Metrics {
            cpu_pct: 12.0,
            cpu_cores: cores,
            cpu_name: "Apple M3 Max".into(),
            per_core_pcts: vec![12.0; cores],
            ram_pct: 42.0,
            ram_used_gb: 54,
            ram_total_gb: 128,
            gpus: (0..gpus)
                .map(|_| GpuInfo {
                    pct: 45.0,
                    label: "NVIDIA A100".into(),
                    mem_used_mb: 8192,
                    mem_total_mb: 40960,
                })
                .collect(),
            disk_pct: Some(2.0),
            disk_mount_label: "/",
            disk_used_gb: 11,
            disk_total_gb: 1858,
            net_down_bps: if network { 1_200_000.0 } else { 0.0 },
            net_up_bps: if network { 340_000.0 } else { 0.0 },
        }
    }

    fn visible(line: &str) -> String {
        let mut result = String::new();
        let mut chars = line.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                assert_eq!(chars.next(), Some('['));
                for ch in chars.by_ref() {
                    if ('@'..='~').contains(&ch) {
                        break;
                    }
                }
            } else {
                result.push(ch);
            }
        }
        result
    }

    #[test]
    fn system_disk_ignores_boot_and_external_volumes_regardless_of_order() {
        use std::path::Path;
        let mut disks = [
            (Path::new("/boot"), 100, 50),
            (Path::new("/"), 1000, 250),
            (Path::new("/Volumes/Backup"), 2000, 1500),
        ];
        for _ in 0..disks.len() {
            assert_eq!(system_disk_usage(disks.into_iter()), Some(("/", 1000, 250)));
            disks.rotate_left(1);
        }
        assert_eq!(system_disk_usage(std::iter::empty()), None);
        assert_eq!(
            system_disk_usage([(Path::new("/boot"), 100, 50)].into_iter()),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_disk_prefers_writable_data_volume_over_system_snapshot() {
        use std::path::Path;
        let mut disks = [
            (Path::new("/"), 1000, 250),
            (Path::new("/System/Volumes/Data"), 1000, 200),
        ];
        for _ in 0..disks.len() {
            assert_eq!(
                system_disk_usage(disks.into_iter()),
                Some(("Data", 1000, 200))
            );
            disks.reverse();
        }
    }

    #[test]
    fn disk_mount_is_visible_in_compact_and_wide_layouts() {
        let mut metrics = sample(16, true, 0);
        for mount in ["/", "Data"] {
            metrics.disk_mount_label = mount;
            for cols in [40, 80, 160] {
                let lines: Vec<_> = render_lines(&metrics, cols, BG)
                    .iter()
                    .map(|line| visible(line))
                    .collect();
                let disk = lines.iter().find(|line| line.contains("DISK")).unwrap();
                assert!(disk.contains(&format!("DISK {mount}")));
                assert!(disk.contains("11/1858G"));
                assert!(lines.iter().all(|line| line.width() <= cols));
            }
        }
    }

    #[test]
    fn mixed_gpus_display_individual_models_and_capacities() {
        let mut metrics = sample(16, false, 2);
        metrics.gpus[1].label = "NVIDIA RTX 3090".into();
        metrics.gpus[1].mem_total_mb = 24 * 1024;
        let lines: Vec<_> = render_lines(&metrics, 160, BG)
            .iter()
            .map(|line| visible(line))
            .collect();
        let gpu = lines.iter().find(|line| line.contains("GPU0")).unwrap();
        assert!(gpu.contains("GPU0 NVIDIA A100, GPU1 NVIDIA RTX 3090"));
        let memory = lines.iter().find(|line| line.contains("MEM0")).unwrap();
        assert!(memory.contains("MEM0 40G, MEM1 24G"));
        assert!(!memory.contains(" ea"));
        assert!(lines.iter().all(|line| line.width() <= 160));
        metrics.gpus[1].mem_total_mb = 0;
        assert_eq!(gpu_vram_label(&metrics.gpus), "MEM0 40G, MEM1 unknown");
    }

    #[test]
    fn only_matching_gpu_capacities_share_an_each_label() {
        let metrics = sample(16, false, 2);
        assert_eq!(gpu_vram_label(&metrics.gpus), "40G ea");
        assert_eq!(gpu_vram_label(&metrics.gpus[..1]), "40G");
        assert_eq!(gpu_model_label(&metrics.gpus), "NVIDIA A100");
    }

    #[test]
    fn layouts_fit_narrow_and_wide_terminals() {
        for cols in [0, 1, 10, 20, 40, 60, 80, 100, 120, 160, 240] {
            for network in [false, true] {
                for gpus in [0, 1, 8] {
                    for line in render_lines(&sample(128, network, gpus), cols, BG) {
                        assert!(
                            visible(&line).width() <= cols,
                            "{cols} columns: {}",
                            visible(&line)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn network_fields_remain_complete_at_eighty_columns() {
        let lines: Vec<_> = render_lines(&sample(16, true, 0), 80, BG)
            .iter()
            .map(|s| visible(s))
            .collect();
        let network = lines.iter().find(|s| s.contains("Download")).unwrap();
        assert!(network.contains("1.2 MB/s"));
        assert!(network.contains("Upload"));
        assert!(network.contains("340 KB/s"));
        assert!(network.width() <= 80);
    }

    #[test]
    fn short_terminals_preserve_shell_rows() {
        for rows in 1..=24 {
            let lines = render_for_terminal(&sample(256, true, 8), rows, 80);
            assert!(lines.len() <= rows.saturating_sub(2) as usize);
        }
    }

    #[test]
    fn clipping_preserves_unicode_cells_and_complete_color_sequences() {
        let line = fit_line("\x1b[31m界界abc\x1b[0m", 5, BG);
        assert_eq!(visible(&line), "界界a");
        assert!(line.ends_with(RST));
        assert_eq!(visible(&fit_line("a\x1b[2Jb", 2, "")), "ab");
    }
}
