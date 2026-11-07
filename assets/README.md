# Repository visuals

- `demo.svg`: animated README illustration of model training on two GPUs, with a static fallback for reduced-motion preferences. Metrics and training output are illustrative, not a benchmark or screen recording. The example `train.py` represents a user's training script; it is not included in sbar.
- `social-preview.png`: 1280 × 640 share card for GitHub's social preview and launch posts.
- `social-preview.svg`: editable vector version of that share card.

Regenerate the SVGs with `python3 scripts/render_visuals.py`. On macOS with ffmpeg installed, `python3 scripts/render_visuals.py --png` also exports the PNG using Quick Look and ffmpeg. On other systems, export the static SVG at its native dimensions using an SVG renderer.

For GitHub link previews, upload `social-preview.png` under the repository's **Settings → General → Social preview → Edit → Upload an image**. Adding the file to the repository alone does not configure the preview. GitHub recommends 1280 × 640 pixels and a file smaller than 1 MB.

All artwork is original and covered by the repository's MIT license. The files contain no external fonts, images, scripts, or tracking requests.
