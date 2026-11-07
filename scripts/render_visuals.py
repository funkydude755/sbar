#!/usr/bin/env python3
"""Generate the README animation and share card from illustrative metric data.

Run with Python 3. Use --png on macOS to export with Quick Look and ffmpeg.
Other systems can export assets/social-preview.svg with an SVG renderer.
No runtime dependencies, external assets, or CI are required for the SVGs.
"""

import argparse
from html import escape
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
INK = "#e6edf6"
MUTED = "#8190a7"
CYAN = "#68e0e8"
PURPLE = "#c3a0f5"
GREEN = "#9bddab"
YELLOW = "#e9cc83"
MONO = "Menlo,Consolas,Liberation Mono,monospace"
SANS = "Helvetica,Arial,sans-serif"


def text(x, y, value, color=INK, size=16, family=MONO, **attrs):
    extra = " ".join(f'{key.replace("_", "-")}="{escape(str(value))}"'
                     for key, value in attrs.items())
    return (f'<text x="{x}" y="{y}" fill="{color}" font-size="{size}" '
            f'font-family="{family}" {extra}>{escape(value)}</text>')


def cell(column, row, value, color=INK, **attrs):
    return text(round(88 + column * 9.64, 2), 408 + row * 25, value,
                color, **attrs)


def bar(column, row, pct, width, color):
    filled = round(pct / 100 * width)
    return cell(column, row, "█" * filled, color) + cell(
        column + filled, row, "░" * (width - filled), "#344054")


SAMPLES = [
    (18, 42, 31, 24, "1.2 MB/s", "340 KB/s"),
    (46, 43, 55, 41, "2.4 MB/s", "480 KB/s"),
    (72, 45, 78, 64, "3.8 MB/s", "620 KB/s"),
    (39, 45, 62, 49, "1.7 MB/s", "410 KB/s"),
    (12, 42, 28, 19, "840 KB/s", "120 KB/s"),
    (18, 42, 31, 24, "1.2 MB/s", "340 KB/s"),
]


def metrics(index):
    cpu, ram, gpu0, gpu1, down, up = SAMPLES[index]
    chunks = [cell(0, 0, "CPU", CYAN, font_weight="bold"),
              cell(5, 0, f"{cpu:3}%", CYAN), bar(11, 0, cpu, 19, CYAN),
              cell(32, 0, "Xeon 8259CL 48c", MUTED),
              cell(55, 0, "RAM", PURPLE, font_weight="bold"),
              cell(60, 0, f"{ram:3}%", PURPLE), bar(66, 0, ram, 16, PURPLE),
              cell(84, 0, f"{round(128 * ram / 100)}/128G", MUTED),
              cell(96, 0, f"17:32:{7 + index:02}", MUTED)]
    for row in [0, 1, 2]:
        chunks.append(cell(52, row, "│", "#445168"))
    for core in range(8):
        percent = (cpu + core * 13) % 97
        col, row = (core % 4) * 12, core // 4 + 1
        chunks.append(cell(col, row, f"{core:2}", MUTED))
        chunks.append(cell(col + 3, row, f"{percent:3}%", CYAN))
    chunks += [cell(55, 1, "DISK /", YELLOW, font_weight="bold"),
               cell(63, 1, "  2%", YELLOW), bar(69, 1, 2, 13, YELLOW),
               cell(84, 1, "11/1858G", MUTED),
               cell(55, 2, "Download", CYAN), cell(65, 2, down),
               cell(78, 2, "Upload", CYAN), cell(86, 2, up)]
    for col, gpu, pct in [(0, 0, gpu0), (38, 1, gpu1)]:
        chunks += [cell(col, 3, f"GPU{gpu}", GREEN, font_weight="bold"),
                   cell(col + 5, 3, f"{pct:3}%", GREEN),
                   bar(col + 11, 3, pct, 19, GREEN),
                   cell(col, 4, f"MEM{gpu}", PURPLE, font_weight="bold"),
                   cell(col + 5, 4, f"{62 - gpu * 11:3}%", PURPLE),
                   bar(col + 11, 4, 62 - gpu * 11, 19, PURPLE)]
    chunks += [cell(34, 3, "│", "#445168"), cell(34, 4, "│", "#445168"),
               cell(78, 3, "NVIDIA A100", MUTED), cell(78, 4, "40G ea", MUTED)]
    return "".join(chunks)


def shell(index):
    chunks = [text(90, 216, "❯", CYAN), text(112, 216, "sbar"),
              text(90, 246, "❯", CYAN), text(112, 246, "cargo build --release")]
    if index in (1, 2, 3, 4, 5):
        chunks += [text(112, 277, "Compiling", GREEN),
                   text(219, 277, "my-project v0.1.0", MUTED)]
    if index in (3, 4, 5):
        chunks += [text(112, 307, "Finished", GREEN),
                   text(219, 307, "release profile in 4.2s", MUTED),
                   text(90, 338, "❯", CYAN),
                   '<rect class="cursor" x="112" y="324" width="8" height="17" fill="#68e0e8"/>']
    else:
        chunks.append(text(112, 318, "Building", YELLOW))
        chunks.append(text(219, 318, "[" + "━" * (index * 4 + 4) + "─" * (16 - index * 4) + "]", MUTED))
    return "".join(chunks)


def document(animated):
    style = "text { font-kerning: none; }"
    if animated:
        style += """
.sample { opacity: 0; }
.sample.first { opacity: 1; }
.cursor { animation: blink 1.2s step-end infinite; }
@keyframes blink { 0%, 50%, 100% { opacity: 1; } 51%, 99% { opacity: 0; } }
"""
        for i in range(6):
            start, end = i * 100 / 6, (i + 1) * 100 / 6
            if i == 0:
                keyframes = f"0%,{end - .01:.3f}%{{opacity:1}} {end:.3f}%,100%{{opacity:0}}"
            elif i == 5:
                keyframes = f"0%,{start - .01:.3f}%{{opacity:0}} {start:.3f}%,100%{{opacity:1}}"
            else:
                keyframes = (f"0%,{start - .01:.3f}%{{opacity:0}} "
                             f"{start:.3f}%,{end - .01:.3f}%{{opacity:1}} "
                             f"{end:.3f}%,100%{{opacity:0}}")
            style += f".s{i}{{animation:sample{i} 12s step-end infinite}}@keyframes sample{i}{{{keyframes}}}"
        style += """
@media (prefers-reduced-motion: reduce) {
  .sample { animation: none !important; opacity: 0 !important; }
  .sample.first { opacity: 1 !important; }
  .cursor { animation: none !important; }
}
"""
    parts = [f'''<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="640" viewBox="0 0 1280 640" role="img" aria-labelledby="title description">
<title id="title">sbar — live stats, same terminal</title>
<desc id="description">Illustrative terminal demo: a command runs above a persistent status bar with updating CPU, memory, GPU, disk, and network metrics. Install with brew install funkydude755/tap/sbar. Supports macOS and Linux. The metrics and build output are examples.</desc>
<defs>
  <linearGradient id="background" x2="1" y2="1"><stop stop-color="#0a1220"/><stop offset="1" stop-color="#11101e"/></linearGradient>
  <linearGradient id="edge"><stop stop-color="#68e0e8"/><stop offset=".54" stop-color="#7994e5"/><stop offset="1" stop-color="#c3a0f5"/></linearGradient>
  <pattern id="grid" width="32" height="32" patternUnits="userSpaceOnUse"><path d="M32 0H0V32" fill="none" stroke="#8092b7" stroke-opacity=".055"/></pattern>
  <clipPath id="terminal"><rect x="64" y="136" width="1152" height="390" rx="14"/></clipPath>
</defs>
<style>{style}</style>
<rect width="1280" height="640" rx="20" fill="url(#background)"/>
<rect width="1280" height="640" rx="20" fill="url(#grid)"/>
<rect x="64" y="42" width="5" height="49" rx="2.5" fill="url(#edge)"/>
''', text(84, 85, "sbar", INK, 56, font_weight="bold"),
             text(264, 67, "Live stats. Same terminal.", INK, 32, SANS, font_weight="bold"),
             text(264, 99, "CPU, RAM, GPU, disk and network — right under your shell.", MUTED, 18, SANS),
             text(1214, 52, "OPEN SOURCE / MIT", MUTED, 12, text_anchor="end", letter_spacing="1.2"),
             '<rect x="64" y="144" width="1152" height="390" rx="14" fill="#000" opacity=".3"/>',
             '<rect x="64" y="136" width="1152" height="390" rx="14" fill="#0d1420" stroke="#334054"/>',
             '<g clip-path="url(#terminal)">',
             '<rect x="64" y="136" width="1152" height="40" fill="#1a2230"/>',
             '<path d="M64 176H1216" stroke="#2c3748"/>',
             '<circle cx="87" cy="156" r="5" fill="#de747b"/>',
             '<circle cx="106" cy="156" r="5" fill="#e9cc83"/>',
             '<circle cx="125" cy="156" r="5" fill="#9bddab"/>',
             text(640, 161, "sbar — ~/project", MUTED, 13, text_anchor="middle"),
             '<circle cx="1145" cy="156" r="3" fill="#68e0e8"/>',
             text(1158, 160, "LIVE", CYAN, 11, letter_spacing="1"),
             '<rect x="65" y="382" width="1150" height="144" fill="#171e2a"/>',
             '<path d="M65 382H1215" stroke="url(#edge)" stroke-opacity=".7"/>']
    if animated:
        for i in range(6):
            parts += [f'<g class="sample s{i}{" first" if i == 0 else ""}">', shell(i), metrics(i), '</g>']
    else:
        parts += [shell(3), metrics(3)]
    parts += ['</g>',
              text(64, 557, "Your commands keep running. Your system stays in view.", INK, 17, SANS),
              '<rect x="64" y="576" width="536" height="38" rx="8" fill="#172232" stroke="#2d4054"/>',
              text(82, 601, "$", CYAN, 16),
              text(106, 601, "brew install funkydude755/tap/sbar", INK, 16),
              text(1216, 600, "macOS + Linux  ·  Written in Rust", INK, 16, SANS, text_anchor="end"),
              text(1216, 624, "Illustrative demo · example metrics and build output", MUTED, 11, SANS, text_anchor="end"),
              '</svg>']
    return "\n".join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--png", action="store_true", help="Export the PNG with macOS Quick Look and ffmpeg")
    args = parser.parse_args()
    assets = ROOT / "assets"
    assets.mkdir(exist_ok=True)
    (assets / "demo.svg").write_text(document(animated=True))
    (assets / "social-preview.svg").write_text(document(animated=False))
    if args.png:
        if not shutil.which("ffmpeg") or not shutil.which("qlmanage"):
            parser.error("--png needs macOS and ffmpeg; alternatively export the static SVG with your SVG renderer")
        with tempfile.TemporaryDirectory(prefix="sbar-share-card-") as temporary:
            render = Path(temporary) / "card.svg"
            # Quick Look's SVG thumbnail viewport is 768 CSS pixels wide. Keep
            # the viewBox intact so its 1280px thumbnail contains the whole card.
            render.write_text(document(animated=False).replace(
                'width="1280" height="640" viewBox',
                'width="768" height="384" viewBox', 1))
            subprocess.run(["qlmanage", "-t", "-s", "1280", "-o", temporary,
                            str(render)], check=True, stdout=subprocess.DEVNULL)
            subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i",
                            str(render) + ".png", "-vf", "crop=1280:640:0:0", "-frames:v", "1",
                            str(assets / "social-preview.png")], check=True)
    print("Generated assets/demo.svg and assets/social-preview.svg")


if __name__ == "__main__":
    main()
