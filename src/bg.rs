//! Background art: bg.png/jpg rendered as a dim ASCII char ramp; bg.mp4/webm
//! decoded by piping raw frames through the ffmpeg CLI. The backdrop fills the
//! whole canvas behind the notes; notes overwrite cells painter-style and keep
//! their bright colors.
//!
//! Two styles (`--bg-style`): `CharRamp` — grayscale ramp chars in dark-gray
//! 256-color foregrounds; `TruecolorBg` — space chars with dimmed truecolor
//! painted into the cell backgrounds (needs a truecolor terminal).

use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::render::{Cell, Color};

/// How pixels become cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgStyle {
    /// Grayscale char ramp (` ..:-=+*#%@`) in dark-gray 256-color foregrounds.
    CharRamp,
    /// Space chars with dimmed truecolor painted into cell backgrounds
    /// (assumes a 24-bit-color terminal).
    TruecolorBg,
}

impl BgStyle {
    /// Parse a `--bg-style` value, or None for an unknown style.
    pub fn parse(s: &str) -> Option<BgStyle> {
        match s {
            "ramp" => Some(BgStyle::CharRamp),
            "cells" => Some(BgStyle::TruecolorBg),
            _ => None,
        }
    }
}

// Grayscale char ramp, darkest → brightest. Index 0 = ' ' so black areas stay
// blank (the terminal's own black), keeping the backdrop from looking busy.
const RAMP: [char; 12] = [' ', '.', ',', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
// Must match render::GRAYS (12 entries, codes 232..=243).
const GRAY_LEVELS: usize = 12;

/// Video decode rate for the backdrop (fps fed to ffmpeg); the 60fps render
/// loop only re-converts when a new frame actually arrives.
const VIDEO_FPS: u32 = 12;

/// Brightest ramp index usable at a given `--bg` level (1..=10). Level 5 caps
/// at index 5 (code 237, #303030); level 10 reaches the full ramp (code 243,
/// #5e5e5e) — still darker than Color::Dim (#7f7f7f), so rings/trails stay
/// legible and notes always outshine the backdrop.
fn max_index(level: u8) -> usize {
    ((GRAY_LEVELS - 1) * level as usize / 10).max(1)
}

/// Map luminance [0,1) to a ramp index. A slight gamma lift keeps dark
/// regions visible; a small lift raises black off 0 so the image reads at
/// low levels without letting bright pixels approach note colors.
fn ramp_index(lum: f32, level: u8) -> usize {
    let l = lum.clamp(0.0, 1.0).powf(0.9) * 0.95 + 0.02;
    ((l * max_index(level) as f32).round() as usize).min(max_index(level))
}

/// Rec.709 luma of an RGB triple (0..1).
fn lum(r: u8, g: u8, b: u8) -> f32 {
    0.2126 * r as f32 / 255.0 + 0.7152 * g as f32 / 255.0 + 0.0722 * b as f32 / 255.0
}

/// Map a pixel buffer to a `cw x ch` cell layer. Each cell samples the
/// average of the pixels covering it in source space; with pw == cw and
/// ph == 2*ch that is exactly the 2 vertical pixels of each cell (2:1 char
/// aspect). `bpp` is 4 (RGBA, alpha composites over black — transparent =
/// blank) or 3 (rgb24 from the video pipe, always opaque).
fn cells_from_pixels(
    px: &[u8],
    pw: usize,
    ph: usize,
    cw: usize,
    ch: usize,
    bpp: usize,
    level: u8,
    style: BgStyle,
) -> Vec<Cell> {
    let mut cells = vec![Cell::default(); cw * ch];
    for cy in 0..ch {
        // Source-space vertical span of this row of cells (at least 1 px).
        let y0 = cy * ph / ch;
        let y1 = (((cy + 1) * ph / ch).max(y0 + 1)).min(ph);
        for cx in 0..cw {
            let x0 = cx * pw / cw;
            let x1 = (((cx + 1) * pw / cw).max(x0 + 1)).min(pw);
            // Per-channel averages (alpha-weighted) serve both styles.
            let mut sum = [0.0f32; 3];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * pw + x) * bpp;
                    let a = if bpp == 4 { px[i + 3] as f32 / 255.0 } else { 1.0 };
                    sum[0] += px[i] as f32 * a;
                    sum[1] += px[i + 1] as f32 * a;
                    sum[2] += px[i + 2] as f32 * a;
                    n += 1;
                }
            }
            let inv = if n > 0 { 1.0 / n as f32 } else { 0.0 };
            let avg = [sum[0] * inv, sum[1] * inv, sum[2] * inv];
            cells[cy * cw + cx] = match style {
                BgStyle::CharRamp => {
                    let idx = ramp_index(lum(avg[0] as u8, avg[1] as u8, avg[2] as u8), level) as u8;
                    Cell { ch: RAMP[idx as usize], color: Color::Gray(idx), bg: None }
                }
                BgStyle::TruecolorBg => Cell {
                    ch: ' ',
                    color: Color::Default,
                    bg: Some(dim_rgb(avg, level)),
                },
            };
        }
    }
    cells
}

/// Dim a color for the truecolor-cell backdrop: `--bg` level scales brightness
/// (level 10 ≈ 45% of the original, level 1 ≈ 4.5%), then quantize to 5 bits
/// per channel (32 levels) so neighboring cells merge into longer SGR runs —
/// a per-cell 24-bit escape on every frame is expensive to emit.
fn dim_rgb(avg: [f32; 3], level: u8) -> [u8; 3] {
    let scale = 0.045 * level as f32;
    avg.map(|v| ((v * scale) as u8 & 0xF8) | 0x04)
}

/// Detected background source: a decoded still image or a live ffmpeg pipe.
enum Source {
    Image(ImageBg),
    Video(VideoBg),
}

/// Background art state. Owned by the player loop; `layer()` is the single
/// integration point — call it every frame and push `Some` into the renderer.
pub struct Bg {
    level: u8,
    #[allow(dead_code)] // carried for the future TruecolorBg implementation
    style: BgStyle,
    dirty: bool,
    /// Dims of the last layer actually pushed (resize ⇒ push again).
    last_dims: Option<(u16, u16)>,
    src: Option<Source>,
}

impl Bg {
    /// Scan `dir` for background art. Video candidates win over images.
    /// Never fails: anything unopenable/undecodable logs one stderr warning
    /// and yields a disabled `Bg`. `level == 0` skips detection entirely.
    pub fn detect(dir: &Path, level: u8, style: BgStyle) -> Bg {
        let level = level.min(10);
        let mut bg = Bg { level, style, dirty: false, last_dims: None, src: None };
        if level == 0 {
            return bg;
        }

        const VIDEO_EXTS: [&str; 4] = ["mp4", "webm", "mov", "avi"];
        const IMAGE_EXTS: [&str; 3] = ["png", "jpg", "jpeg"];
        for ext in VIDEO_EXTS {
            let path = dir.join(format!("bg.{ext}"));
            if path.exists() {
                if VideoBg::probe().is_some() {
                    bg.src = Some(Source::Video(VideoBg {
                        path,
                        child: None,
                        slot: Arc::new(Mutex::new(Slot { frame: Vec::new(), seq: 0, ended: false })),
                        dims: (0, 0),
                        last_seq: 0,
                    }));
                } else {
                    eprintln!(
                        "warning: bg.{} found but ffmpeg unavailable; playing without background",
                        ext
                    );
                }
                return bg;
            }
        }
        for ext in IMAGE_EXTS {
            let path = dir.join(format!("bg.{ext}"));
            if path.exists() {
                match image::open(&path) {
                    Ok(img) => {
                        bg.src = Some(Source::Image(ImageBg { rgba: img.to_rgba8(), cache: None }));
                        return bg;
                    }
                    Err(e) => {
                        eprintln!("warning: {} unreadable ({e}); playing without background", path.display());
                        return bg;
                    }
                }
            }
        }
        bg
    }

    /// Change the dim level live (`b` key). Marks dirty so the next `layer()`
    /// call re-pushes. Level 0 hides the backdrop (the source stays attached).
    pub fn set_level(&mut self, level: u8) {
        let level = level.min(10);
        if level != self.level {
            self.level = level;
            self.dirty = true;
        }
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn enabled(&self) -> bool {
        self.level > 0 && self.src.is_some()
    }

    /// Cells for a `(w, h)` canvas, or None if nothing new to push. Returns
    /// Some when the level changed, the dims changed (e.g. after a resize
    /// rebuilt the renderer), or a video frame arrived.
    pub fn layer(&mut self, w: u16, h: u16) -> Option<Vec<Cell>> {
        if self.level == 0 || self.src.is_none() {
            return None;
        }
        let dims_changed = self.last_dims != Some((w, h));
        if !self.dirty && !dims_changed {
            // Videos still push when a new frame arrived.
            let new_frame = match &self.src {
                Some(Source::Video(v)) => v.new_frame_ready(),
                _ => false,
            };
            if !new_frame {
                return None;
            }
        }
        self.dirty = false;
        self.last_dims = Some((w, h));
        let style = self.style;

        match &mut self.src {
            Some(Source::Image(img)) => Some(img.layer(w, h, self.level, style)),
            Some(Source::Video(v)) => match v.layer(w, h, self.level, style) {
                Ok(cells) => cells,
                Err(()) => {
                    // The decode pipe failed to (re)spawn — give up on video.
                    eprintln!("warning: video background failed to start; playing without background");
                    self.src = None;
                    None
                }
            }
            None => None,
        }
    }
}

// --- Still images -----------------------------------------------------------

struct ImageBg {
    rgba: image::RgbaImage,
    cache: Option<((u16, u16), Vec<Cell>)>,
}

impl ImageBg {
    /// Cover-crop the image to the canvas (at 2× pixel height for the 2:1
    /// char aspect) and convert. Cached per canvas size → zero per-frame cost.
    fn layer(&mut self, w: u16, h: u16, level: u8, style: BgStyle) -> Vec<Cell> {
        if let Some((dims, cells)) = &self.cache
            && *dims == (w, h)
        {
            return cells.clone();
        }
        let (pw, ph) = (w as u32, 2 * h as u32);
        let dyn_img = image::DynamicImage::ImageRgba8(self.rgba.clone());
        let scaled = dyn_img.resize_to_fill(pw, ph, image::imageops::FilterType::Triangle);
        let cells = cells_from_pixels(scaled.to_rgba8().as_raw(), pw as usize, ph as usize, w as usize, h as usize, 4, level, style);
        self.cache = Some(((w, h), cells.clone()));
        cells
    }
}

// --- Video via the ffmpeg CLI ------------------------------------------------

struct VideoBg {
    path: std::path::PathBuf,
    /// None until the first `layer()` spawns the decode pipe.
    child: Option<Child>,
    slot: Arc<Mutex<Slot>>,
    /// Canvas dims the pipe was spawned at (video respawns on resize; (0,0)
    /// until the first `layer` call, since detect() doesn't know the canvas).
    dims: (u16, u16),
    last_seq: u64,
}

struct Slot {
    frame: Vec<u8>, // latest complete rgb24 frame (w * 2h * 3 bytes)
    seq: u64,
    ended: bool, // reader thread hit EOF/short read (video freezes on last frame)
}

impl VideoBg {
    /// Verify ffmpeg is usable (a cheap probe run) without knowing the canvas
    /// dims yet; the real decode pipe spawns on the first `layer()` call.
    fn probe() -> Option<()> {
        let mut probe = Command::new("ffmpeg")
            .args(["-version"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let _ = probe.wait();
        Some(())
    }

    /// Spawn (or respawn after a resize) the decode pipe at `(w, h)`.
    /// stdin is nulled — inheriting the raw-mode TTY would steal keystrokes —
    /// and stderr is nulled to keep ffmpeg chatter off the TUI.
    fn spawn_pipe(&mut self, w: u16, h: u16) -> bool {
        self.stop();
        let (pw, ph) = (w as u32, 2 * h as u32);
        let frame_bytes = w as usize * ph as usize * 3;
        let child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                // Stream at native rate: without -re ffmpeg decodes the whole
                // file into the pipe instantly and the "animation" would be
                // over in a fraction of a second.
                "-re",
                "-an",
                "-sn",
                "-dn",
                "-i",
            ])
            .arg(&self.path)
            .args([
                "-vf",
                &format!("fps={VIDEO_FPS},scale={pw}:{ph}:force_original_aspect_ratio=increase,crop={pw}:{ph}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => return false,
        };
        let stdout: ChildStdout = child.stdout.take().expect("piped stdout");
        let slot = Arc::new(Mutex::new(Slot { frame: Vec::new(), seq: 0, ended: false }));
        let reader_slot = Arc::clone(&slot);
        thread::spawn(move || read_frames(stdout, frame_bytes, reader_slot));
        self.child = Some(child);
        self.slot = slot;
        self.dims = (w, h);
        self.last_seq = 0;
        true
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn new_frame_ready(&self) -> bool {
        // Note: `ended` does NOT suppress this — a frame that arrived before
        // EOF still deserves to be shown (the video freezes on its last frame).
        self.slot
            .lock()
            .map(|s| s.seq != self.last_seq)
            .unwrap_or(false)
    }

    /// Convert the latest frame, respawning the pipe if the canvas changed.
    /// `Err(())` = the pipe could not (re)spawn and the source must be disabled.
    fn layer(&mut self, w: u16, h: u16, level: u8, style: BgStyle) -> Result<Option<Vec<Cell>>, ()> {
        if self.dims != (w, h) && !self.spawn_pipe(w, h) {
            return Err(());
        }
        let (frame, seq) = {
            let guard = self.slot.lock().map_err(|_| ())?;
            if guard.seq == self.last_seq || guard.frame.is_empty() {
                return Ok(None);
            }
            (guard.frame.clone(), guard.seq)
        };
        self.last_seq = seq;
        Ok(Some(cells_from_pixels(&frame, w as usize, 2 * h as usize, w as usize, h as usize, 3, level, style)))
    }
}

impl Drop for VideoBg {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reader thread: pull frame-sized chunks off the pipe and publish them under
/// the lock. Any short read/EOF ends the thread (the video freezes on its last
/// frame); killing the child (Drop / resize) has the same effect.
fn read_frames(mut out: ChildStdout, frame_bytes: usize, slot: Arc<Mutex<Slot>>) {
    let mut scratch = vec![0u8; frame_bytes];
    loop {
        match std::io::Read::read_exact(&mut out, &mut scratch) {
            Ok(()) => {
                let Ok(mut guard) = slot.lock() else { return };
                std::mem::swap(&mut guard.frame, &mut scratch);
                guard.seq += 1;
                // The swap handed us back the previously published frame; grow
                // it back to a full frame so the next read_exact reads real
                // bytes (an empty buffer would make read_exact a no-op).
                scratch.clear();
                scratch.resize(frame_bytes, 0);
            }
            Err(_) => {
                if let Ok(mut guard) = slot.lock() {
                    guard.ended = true;
                }
                return;
            }
        }
    }
}