//! ASCII circle renderer. Notes spawn near the center and travel outward to
//! their button on the outer ring, flashing at hit time.

use crate::chart::{Kind, NoteEvent, Position, SlideShape, StarKind, Zone};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    Default,
    Dim,       // gray (ring / labels)
    Pink,      // default tap notes
    Blue,      // star notes + slide lines + touch notes
    DimBlue,   // faded slide path (blue, not bright)
    Orange,    // break notes (256-color bright orange)
    Magenta,   // EX notes
    Yellow,    // simultaneous (≥2 at same time) + firework
    White,     // touch-note hit border (bright white)
    Rainbow(u8), // touch-holds: index into RAINBOW (24-step hue wheel)
}

// 24-step 256-color hue wheel (red → orange → yellow → green → cyan → blue →
// magenta → red), used for rainbow touch-holds.
const RAINBOW: [&str; 24] = [
    "\x1b[38;5;196m", "\x1b[38;5;202m", "\x1b[38;5;208m", "\x1b[38;5;214m",
    "\x1b[38;5;220m", "\x1b[38;5;226m", "\x1b[38;5;190m", "\x1b[38;5;154m",
    "\x1b[38;5;118m", "\x1b[38;5;46m",  "\x1b[38;5;48m",  "\x1b[38;5;50m",
    "\x1b[38;5;51m",  "\x1b[38;5;45m",  "\x1b[38;5;39m",  "\x1b[38;5;33m",
    "\x1b[38;5;27m",  "\x1b[38;5;21m",  "\x1b[38;5;57m",  "\x1b[38;5;93m",
    "\x1b[38;5;129m", "\x1b[38;5;165m", "\x1b[38;5;201m", "\x1b[38;5;198m",
];

impl Color {
    fn ansi(self) -> &'static str {
        match self {
            Color::Default => "\x1b[0m",
            Color::Dim => "\x1b[90m",
            Color::Pink => "\x1b[38;5;213m",
            Color::Blue => "\x1b[94m",
            Color::DimBlue => "\x1b[34m",
            Color::Orange => "\x1b[38;5;208m",
            Color::Magenta => "\x1b[95m",
            Color::Yellow => "\x1b[93m",
            Color::White => "\x1b[97m",
            Color::Rainbow(i) => RAINBOW[(i as usize) % RAINBOW.len()],
        }
    }
}

/// Map a hue parameter `t` in [0,1) to a Rainbow color around the 24-step wheel.
fn rainbow_color(t: f32) -> Color {
    let i = (t.rem_euclid(1.0) * RAINBOW.len() as f32).floor() as i32;
    let i = ((i % RAINBOW.len() as i32) + RAINBOW.len() as i32) % RAINBOW.len() as i32;
    Color::Rainbow(i as u8)
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    color: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', color: Color::Default }
    }
}

/// HUD line shown beside the circle.
pub struct Hud {
    pub title: String,
    pub difficulty: String,
    pub bpm: f32,
    pub time: f32,
    pub offset: f32,
    pub speed: u8,
    pub paused: bool,
    pub muted: bool,
}

const BASE: f32 = 6.0; // approach window = BASE / speed  (speed 5 → 1.2s)
const FLASH: f32 = 0.16; // post-hit flash duration
// Slide path: the full path fades in over FADE_IN seconds before motion starts,
// then the star traces it out brightly during [motion_start, motion_end].
const FADE_IN: f32 = 0.6;
// Note disc radii (in rows; col radius ≈ 2× this for a 2:1 circle).
const NOTE_R: f32 = 1.2; // standard tap / EX
const BREAK_R: f32 = 1.5; // break notes are bigger
const STAR_R: f32 = 1.3; // star tap
const TOUCH_HOLD_R: f32 = 1.7; // touch-hold body (center C) — nice and big
const HOLD_RING_R: f32 = 2.6; // touch-hold timer ring row-radius (around the body)
const FLASH_R: f32 = 1.7; // enlarged disc at hit time
// Fan (`w`) slide ray thickness (in rows; col radius ≈ 2× this). Thick filled
// `●` bars so the fan reads as a bold solid shape rather than a thin line.
const RAY_R: f32 = 0.9;
// Touch "shutter": four pyramids around the touch point that close in to the
// hit timing. Half-size of the shutter square (rows / cols, 2:1 aspect).
const SHUTTER_R: f32 = 2.0;
const SHUTTER_C: f32 = 4.0;

pub struct Renderer {
    width: i32,
    height: i32,
    cx: f32,
    cy: f32,
    r_cols: f32,
    r_rows: f32,
    r0_cols: f32,
    r0_rows: f32,
    static_layer: Vec<Cell>,
    /// Button index (1..=8) -> (col, row).
    button_pos: [(i32, i32); 9],
}

impl Renderer {
    pub fn new(width: u16, height: u16) -> Self {
        let width = width as i32;
        let height = height as i32;
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;
        // Aspect compensation: chars are ~2x tall as wide, so col radius ≈ 2× row
        // radius. Fit the circle to BOTH dimensions (height-limited and
        // width-limited), then snap to an integer row radius so buttons sit
        // symmetrically on the ring.
        let by_height = ((height as f32 / 2.0) - 2.0).floor();
        let by_width = ((width as f32 / 4.0) - 1.0).floor();
        let r_rows = by_height.min(by_width).max(4.0);
        let r_cols = r_rows * 2.0;
        let r0_cols = r_cols * 0.12; // "near the center but not exactly"
        let r0_rows = r_rows * 0.12;

        let mut button_pos = [(0i32, 0i32); 9];
        for i in 1..=8u8 {
            // Snap each button onto the ellipse outline at its row so it sits
            // exactly on the ring rather than beside it.
            let a = button_angle(i);
            let row = (cy + r_rows * a.sin()).round() as i32;
            let dy = ((row as f32 - cy) / r_rows).clamp(-1.0, 1.0);
            let dx = r_cols * (1.0 - dy * dy).sqrt();
            let col = (cx + if a.cos() >= 0.0 { dx } else { -dx }).round() as i32;
            button_pos[i as usize] = (col, row);
        }

        let mut static_layer = vec![Cell::default(); (width * height) as usize];
        // Concentric reference rings: outer button ring + the 3 touch sensor
        // rings (A/D outer ~0.80R, E middle ~0.60R, B inner ~0.45R), matching
        // the 5 maimai touch zones (C is the center marker drawn next).
        draw_ellipse(&mut static_layer, width, height, cx, cy, r_cols, r_rows, '.', Color::Dim);
        draw_ellipse(&mut static_layer, width, height, cx, cy, r_cols * 0.80, r_rows * 0.80, '·', Color::Dim); // A/D
        draw_ellipse(&mut static_layer, width, height, cx, cy, r_cols * 0.60, r_rows * 0.60, '∘', Color::Dim); // E
        draw_ellipse(&mut static_layer, width, height, cx, cy, r_cols * 0.45, r_rows * 0.45, ',', Color::Dim); // B
        // Center marker.
        put(&mut static_layer, width, height, cx as i32, cy as i32, 'C', Color::Dim);
        // Button labels sit on the outer ring (overwrite the dot at that cell).
        for i in 1..=8u8 {
            let (c, r) = button_pos[i as usize];
            put(&mut static_layer, width, height, c, r, char::from_digit(i as u32, 10).unwrap(), Color::Dim);
        }

        Self {
            width,
            height,
            cx,
            cy,
            r_cols,
            r_rows,
            r0_cols,
            r0_rows,
            static_layer,
            button_pos,
        }
    }

    /// Build the frame as a single string with ANSI colors.
    pub fn frame(&self, events: &[NoteEvent], now: f32, speed: u8, hud: &Hud) -> String {
        let mut cells = self.static_layer.clone();
        let window = BASE / speed as f32;

        // Mark notes that hit at the same time as another note (≥2 at the same
        // `time`) so they can be drawn yellow.
        let sim = simultaneous_flags(events);

        for (i, ev) in events.iter().enumerate() {
            self.draw_event(&mut cells, ev, now, window, sim[i]);
        }

        // Compose the frame string. Use CRLF between lines: in raw mode the
        // terminal does NOT translate bare `\n` to `\r\n` (OPOST off), so a bare
        // `\n` advances one row but keeps the column → staircase/wrap. CRLF keeps
        // every row anchored at column 1. The very last line gets no newline so we
        // never push the cursor past the bottom row (which would scroll).
        let mut out = String::with_capacity((self.width * self.height * 3) as usize);
        out.push_str("\x1b[H"); // cursor home
        let mut last_color = Color::Default;
        for row in 0..self.height {
            for col in 0..self.width {
                let cell = &cells[(row * self.width + col) as usize];
                if cell.color != last_color {
                    out.push_str(cell.color.ansi());
                    last_color = cell.color;
                }
                out.push(cell.ch);
            }
            out.push_str("\x1b[K"); // clear to end of line (no stale trailing chars)
            out.push_str("\r\n");
            out.push_str(Color::Default.ansi());
            last_color = Color::Default;
        }
        out.push_str(Color::Default.ansi());

        // HUD line below the circle (truncated to the canvas width so a long title
        // never wraps onto a second row on narrow terminals).
        let hud_line = format!(
            " {title}  [{diff}]  BPM {bpm:.0}  t {time:6.1}s  offset {off:+.2}  speed {spd}  {paused}{muted}",
            title = hud.title,
            diff = hud.difficulty,
            bpm = hud.bpm,
            time = hud.time,
            off = hud.offset,
            spd = hud.speed,
            paused = if hud.paused { "[PAUSED] " } else { "" },
            muted = if hud.muted { "[MUTED] " } else { "" },
        );
        out.push_str(&truncate(&hud_line, self.width as usize));
        out.push_str("\x1b[K\r\n");
        let keys = " keys: q/Esc quit  +/- offset  Up/Down speed  p pause  m mute";
        out.push_str(&truncate(keys, self.width as usize));
        out.push_str("\x1b[K");
        out
    }

    fn draw_event(&self, cells: &mut [Cell], ev: &NoteEvent, now: f32, window: f32, sim: bool) {
        match &ev.kind {
            Kind::Tap { brk, ex, star } => {
                if matches!(ev.pos, Position::Touch(_, _)) {
                    self.draw_touch(cells, ev, now, window, ev.firework, sim);
                } else {
                    self.draw_tap(cells, ev, now, window, *brk, *ex, *star, sim, false);
                }
            }
            Kind::Hold { end, brk, ex } => {
                // The hold bar appears the moment the note spawns and grows with
                // the approaching head: during approach it grows from the spawn
                // point toward the button (head at its tip), is full at hit,
                // then retracts toward the button over the hold duration. Ring
                // holds only (touch holds are `Kind::TouchHold`).
                let is_ring = matches!(ev.pos, Position::Button(_));
                if is_ring {
                    let (tc, tr, sc, sr, _, _) = self.spawn_target(ev.pos);
                    let color = note_color(*brk, *ex, StarKind::None, sim, false);
                    let rdisc = if *brk { BREAK_R } else { NOTE_R };
                    let tcf = tc as f32;
                    let trf = tr as f32;
                    if now >= ev.time - window && now < ev.time {
                        // Approach: bar grows from spawn toward the head.
                        let p_a = ((now - (ev.time - window)) / window).clamp(0.0, 1.0);
                        self.draw_bar(cells, sc, sr, tcf, trf, p_a, rdisc * 0.7, color);
                    } else if now >= ev.time && now <= *end {
                        // Hold: bar retracts from full (button→spawn) to 0.
                        let dur = (*end - ev.time).max(0.001);
                        let p = ((now - ev.time) / dur).clamp(0.0, 1.0);
                        self.draw_bar(cells, tcf, trf, sc, sr, 1.0 - p, rdisc * 0.7, color);
                    }
                }
                // Head/flash on top of the bar: approach travel + hit flash.
                self.draw_tap(cells, ev, now, window, *brk, *ex, StarKind::None, sim, false);
                // Sustained head disc at the button after the flash fades, held
                // through the hold's end (draw_tap returns early past t+FLASH).
                if is_ring && now > ev.time + FLASH && now <= *end {
                    let (tc, tr, _, _, _, _) = self.spawn_target(ev.pos);
                    let color = note_color(*brk, *ex, StarKind::None, sim, false);
                    let rdisc = if *brk { BREAK_R } else { NOTE_R };
                    put_disc(cells, self.width, self.height, tc, tr, rdisc, '●', color);
                }
            }
            Kind::TouchHold { end, firework } => {
                // Approach: same closing-shutter behaviour as a touch tap.
                if now < ev.time {
                    self.draw_touch(cells, ev, now, window, *firework, sim);
                    return;
                }
                // Held phase: a rainbow body sits at the touch point (usually
                // center C, but wherever the touch zone is) and a small ring
                // sweeps clockwise from the top as a hold-duration indicator.
                if now <= *end {
                    let (tc, tr) = self.touch_xy(ev.pos);
                    let dur = (*end - ev.time).max(0.001);
                    let p = ((now - ev.time) / dur).clamp(0.0, 1.0);
                    // Rainbow body whose hue slowly cycles with time.
                    let body = rainbow_color(now * 0.6);
                    put_disc(cells, self.width, self.height, tc, tr, TOUCH_HOLD_R, '●', body);
                    self.draw_hold_ring(cells, tc, tr, p);
                }
            }
            Kind::Slide { star, brk, ex, parts, .. } => {
                // Star tap approaches like a tap (blue/orange/yellow), unless tapless.
                self.draw_tap(cells, ev, now, window, *brk, *ex, *star, sim, true);
                // Each leg: full path fades in, then the star traces it out.
                let head_color = note_color(*brk, *ex, *star, sim, false);
                for part in parts {
                    if part.shape == SlideShape::W {
                        // Fans split into three expanding rays (see `draw_fan`).
                        self.draw_fan(cells, part, now, head_color);
                    } else {
                        self.draw_slide_leg(cells, part, now, *brk, head_color);
                    }
                }
            }
        }
    }

    fn draw_tap(
        &self,
        cells: &mut [Cell],
        ev: &NoteEvent,
        now: f32,
        window: f32,
        brk: bool,
        ex: bool,
        star: StarKind,
        sim: bool,
        is_slide_star: bool,
    ) {
        if star == StarKind::None && is_slide_star {
            // tapless slide — no star tap rendered
            return;
        }
        let t = ev.time;
        let start = t - window;
        if now < start || now > t + FLASH {
            return;
        }
        let glyph = note_glyph(brk, ex, star);
        let color = note_color(brk, ex, star, sim, false);
        let (target_c, target_r, sc, sr, _s_cols, _s_rows) = self.spawn_target(ev.pos);
        let radius = note_radius(brk, star, is_slide_star);

        if now >= t {
            // Flash phase: enlarged bright disc at the target button.
            put_disc(cells, self.width, self.height, target_c, target_r, FLASH_R, flash_glyph(glyph), color);
            return;
        }

        let p = ((now - start) / window).clamp(0.0, 1.0);
        let col = sc + (target_c as f32 - sc) * p;
        let row = sr + (target_r as f32 - sr) * p;
        put_disc(cells, self.width, self.height, col.round() as i32, row.round() as i32, radius, glyph, color);
        // Dim trail behind.
        if p > 0.15 {
            let p2 = (p - 0.15).max(0.0);
            let tc = sc + (target_c as f32 - sc) * p2;
            let tr = sr + (target_r as f32 - sr) * p2;
            put_disc(cells, self.width, self.height, tc.round() as i32, tr.round() as i32, radius * 0.65, glyph, dim(color));
        }
    }

    fn draw_touch(&self, cells: &mut [Cell], ev: &NoteEvent, now: f32, window: f32, firework: bool, sim: bool) {
        let t = ev.time;
        let start = t - window;
        if now < start || now > t + FLASH {
            return;
        }
        let (target_c, target_r, _sc, _sr, _, _) = self.spawn_target(ev.pos);
        // Firework or simultaneous touches are yellow; otherwise blue.
        let color = if firework || sim { Color::Yellow } else { Color::Blue };
        if now >= t {
            // Hit: flash a white border (closed shutter in bright white) to
            // mark the moment you're meant to hit, then it goes away.
            self.draw_shutter(cells, target_c, target_r, 1.0, Color::White);
            return;
        }
        // Approach: four pyramids close in toward the touch point.
        let p = ((now - start) / window).clamp(0.0, 1.0);
        self.draw_shutter(cells, target_c, target_r, p, color);
    }

    /// Draw the touch "shutter": four pyramids (▲▼◀▶) around (cx,cy) that close
    /// in as p → 1. At p=0 they sit at the shutter square's edges (open); at
    /// p=1 their tips meet at the center (closed).
    fn draw_shutter(&self, cells: &mut [Cell], cx: i32, cy: i32, p: f32, color: Color) {
        let w = self.width;
        let h = self.height;
        let off_r = (1.0 - p) * SHUTTER_R;
        let off_c = (1.0 - p) * SHUTTER_C;
        let top = (cy as f32 - off_r).round() as i32; // top tip row
        let bot = (cy as f32 + off_r).round() as i32; // bottom tip row
        let lft = (cx as f32 - off_c).round() as i32; // left tip col
        let rgt = (cx as f32 + off_c).round() as i32; // right tip col

        // Top pyramid: ▼ pointing down, base one row above the tip.
        for dc in -1..=1 {
            put(cells, w, h, cx + dc, top - 1, '▼', color);
        }
        put(cells, w, h, cx, top, '▼', color);
        // Bottom pyramid: ▲ pointing up, base one row below the tip.
        put(cells, w, h, cx, bot, '▲', color);
        for dc in -1..=1 {
            put(cells, w, h, cx + dc, bot + 1, '▲', color);
        }
        // Left pyramid: ▶ pointing right, base one col left of the tip.
        for dr in -1..=1 {
            put(cells, w, h, lft - 1, cy + dr, '▶', color);
        }
        put(cells, w, h, lft, cy, '▶', color);
        // Right pyramid: ◀ pointing left, base one col right of the tip.
        put(cells, w, h, rgt, cy, '◀', color);
        for dr in -1..=1 {
            put(cells, w, h, rgt + 1, cy + dr, '◀', color);
        }
    }

    fn draw_slide_leg(&self, cells: &mut [Cell], part: &crate::chart::SlidePart, now: f32, brk: bool, head_color: Color) {
        // Sample the full leg path once (dense enough to be a connected line).
        let path = sample_path(self, part);
        if path.is_empty() {
            return;
        }
        // Slide lines are little arrows along the travel direction. Break slides
        // are orange; otherwise blue (star's line color).
        let arrow = if brk { Color::Orange } else { Color::Blue };
        let arrow_dim = if brk { Color::Dim } else { Color::DimBlue };

        // Phase 1 — fade-in: the whole path appears dimly before motion starts.
        if now < part.motion_start {
            let fade_start = part.motion_start - FADE_IN;
            if now < fade_start {
                return;
            }
            let fp = ((now - fade_start) / FADE_IN).clamp(0.0, 1.0);
            // Two-step fade: faint gray arrows, then dim-blue arrows.
            let c = if fp < 0.5 { Color::Dim } else { arrow_dim };
            for &(cc, rr, dc, dr) in &path {
                put(cells, self.width, self.height, cc, rr, arrow_for(dc, dr), c);
            }
            return;
        }
        if now > part.motion_end {
            return;
        }

        // Phase 2 — trace: the star travels start→end, lighting the path behind
        // it; the un-traced remainder stays dim from the fade-in.
        let dur = (part.motion_end - part.motion_start).max(0.001);
        let p = ((now - part.motion_start) / dur).clamp(0.0, 1.0);
        let traced = (p * (path.len() - 1) as f32).round() as usize;
        for (i, &(cc, rr, dc, dr)) in path.iter().enumerate() {
            let c = if i <= traced { arrow } else { arrow_dim };
            put(cells, self.width, self.height, cc, rr, arrow_for(dc, dr), c);
        }
        // Bright star head at the current position.
        let (hc, hr, _, _) = path[traced];
        put_disc(cells, self.width, self.height, hc, hr, STAR_R, '★', head_color);
    }

    /// Draw a fan (Wi-Fi / `w`) slide: three rays from the source button `from`
    /// out to the three consecutive destination sensors `to−1`, `to`, `to+1`.
    /// The star splits at the source and the three rays expand outward
    /// simultaneously — a big opening fan — reaching full extension at
    /// `motion_end`, where the three destination sensors flash. Rays are thick
    /// filled `●` bars (bright for the traveled portion, dim ahead of the
    /// leading tip); a bright `★` hub sits at the source and a `★` head rides
    /// each tip. Color comes from `head_color` (blue/orange/yellow); the dim
    /// remainder uses `dim(head_color)`.
    fn draw_fan(&self, cells: &mut [Cell], part: &crate::chart::SlidePart, now: f32, head_color: Color) {
        let tm1 = wrap1_8(part.to.wrapping_sub(1));
        let tp1 = wrap1_8(part.to.wrapping_add(1));
        let dests = [tm1, part.to, tp1];
        let (fc, fr) = self.button_pos[part.from as usize];
        let (fc, fr) = (fc as f32, fr as f32);
        let dimc = dim(head_color);

        // Phase 1 — fade-in: the full three-ray fan appears dimly before motion.
        if now < part.motion_start {
            let fade_start = part.motion_start - FADE_IN;
            if now < fade_start {
                return;
            }
            let fp = ((now - fade_start) / FADE_IN).clamp(0.0, 1.0);
            let c = if fp < 0.5 { Color::Dim } else { dimc };
            for d in dests {
                let (tc, tr) = self.button_pos[d as usize];
                self.draw_bar(cells, fc, fr, tc as f32, tr as f32, 1.0, RAY_R, c);
            }
            put(cells, self.width, self.height, fc.round() as i32, fr.round() as i32, '★', c);
            return;
        }

        // Past the fan's life: nothing to draw.
        if now > part.motion_end + FLASH {
            return;
        }

        let dur = (part.motion_end - part.motion_start).max(0.001);
        let p = ((now - part.motion_start) / dur).clamp(0.0, 1.0);
        // Hold the fully-open fan through [motion_end, motion_end + FLASH] so the
        // three-sensor landing reads as a deliberate climax, not a flicker.
        let fully_open = now >= part.motion_end;
        let traced = if fully_open { 1.0 } else { p };

        for d in dests {
            let (tc, tr) = self.button_pos[d as usize];
            let (tc, tr) = (tc as f32, tr as f32);
            let tipc = fc + (tc - fc) * traced;
            let tipr = fr + (tr - fr) * traced;
            // Bright traveled portion: source → tip.
            self.draw_bar(cells, fc, fr, tipc, tipr, 1.0, RAY_R, head_color);
            if !fully_open {
                // Dim remainder: tip → destination, with a star head at the tip.
                self.draw_bar(cells, tipc, tipr, tc, tr, 1.0, RAY_R, dimc);
                put(cells, self.width, self.height, tipc.round() as i32, tipr.round() as i32, '★', head_color);
            }
        }
        // Bright hub at the source — the fan's pivot.
        put(cells, self.width, self.height, fc.round() as i32, fr.round() as i32, '★', head_color);

        if fully_open {
            // The fan has opened onto three sensors — flash each one big.
            for d in dests {
                let (tc, tr) = self.button_pos[d as usize];
                put_disc(cells, self.width, self.height, tc, tr, FLASH_R, '✦', head_color);
            }
        }
    }

    /// Draw a hold's tail: a bar of filled discs from the button (tc,tr) inward
    /// toward the spawn point (sc,sr), covering `frac` of that distance (the
    /// remaining hold fraction). The button cell itself is left untouched so the
    /// head/flash glyph there is preserved. `rrows` sets the bar thickness.
    /// Stamp `frac` of the segment from `(from_c, from_r)` toward `(to_c, to_r)`
    /// as a chain of `●` discs (radius `rrows`, aspect-corrected). `frac` ∈
    /// (0, 1] controls how much of the span is filled: 1.0 = full bar, 0.5 =
    /// half from the `from` end. Used for hold bars (grow during approach,
    /// retract during hold).
    fn draw_bar(
        &self,
        cells: &mut [Cell],
        from_c: f32,
        from_r: f32,
        to_c: f32,
        to_r: f32,
        frac: f32,
        rrows: f32,
        color: Color,
    ) {
        if frac <= 0.0 {
            return;
        }
        let dc = to_c - from_c;
        let dr = to_r - from_r;
        // Cell-distance estimate (cols dominate at 2:1 aspect); step ~0.5 cell.
        let dist = dc.abs().max(dr.abs() * 2.0).max(1.0);
        let n = ((dist * frac) * 2.0).ceil() as i32;
        for i in 1..=n {
            let t = (i as f32 / n as f32) * frac; // (0, frac]
            let c = from_c + dc * t;
            let r = from_r + dr * t;
            put_disc(cells, self.width, self.height, c.round() as i32, r.round() as i32, rrows, '●', color);
        }
    }

    /// Draw a touch-hold's timer ring: a small rainbow arc that starts at the
    /// top (12 o'clock) and sweeps clockwise around (tc,tr), covering `p` of a
    /// full turn (p = elapsed hold fraction). The swept arc is rainbow-colored
    /// along its length so the growing ring reads as a progress indicator.
    fn draw_hold_ring(&self, cells: &mut [Cell], tc: i32, tr: i32, p: f32) {
        if p <= 0.0 {
            return;
        }
        let rr: f32 = HOLD_RING_R; // row radius of the ring (cols = 2·rr for 2:1 aspect)
        let rc: f32 = rr * 2.0;
        let start = std::f32::consts::PI * 1.5; // screen angle for "up" (top)
        let sweep = p * 2.0 * std::f32::consts::PI;
        let step = 0.5 / rc.max(rr); // ~0.5 cell per sample → connected arc
        let mut a = 0.0f32;
        while a <= sweep {
            let ang = start + a;
            let c = tc as f32 + rc * ang.cos();
            let r = tr as f32 + rr * ang.sin();
            let hue = a / (2.0 * std::f32::consts::PI); // rainbow around the circle
            put(cells, self.width, self.height, c.round() as i32, r.round() as i32, '●', rainbow_color(hue));
            a += step;
        }
    }

    /// Returns (target_col, target_row, spawn_col, spawn_row, spawn_cols, spawn_rows).
    fn spawn_target(&self, pos: Position) -> (i32, i32, f32, f32, f32, f32) {
        match pos {
            Position::Button(b) => {
                let (tc, tr) = self.button_pos[b as usize];
                let ang = button_angle(b);
                let sc = self.cx + self.r0_cols * ang.cos();
                let sr = self.cy + self.r0_rows * ang.sin();
                (tc, tr, sc, sr, self.r0_cols, self.r0_rows)
            }
            Position::Touch(zone, idx) => {
                if matches!(zone, Zone::C) {
                    // Center touch (hold) sits exactly at C — no inner-ring offset.
                    return (self.cx as i32, self.cy as i32, self.cx as f32, self.cy as f32, 0.0, 0.0);
                }
                let (ang, frac) = touch_geometry(zone, idx);
                let tc = (self.cx + self.r_cols * frac * ang.cos()).round() as i32;
                let tr = (self.cy + self.r_rows * frac * ang.sin()).round() as i32;
                let sc = self.cx + self.r0_cols * ang.cos();
                let sr = self.cy + self.r0_rows * ang.sin();
                (tc, tr, sc, sr, self.r0_cols, self.r0_rows)
            }
        }
    }

    fn touch_xy(&self, pos: Position) -> (i32, i32) {
        if let Position::Touch(zone, idx) = pos {
            if matches!(zone, Zone::C) {
                return (self.cx as i32, self.cy as i32);
            }
            let (ang, frac) = touch_geometry(zone, idx);
            let c = (self.cx + self.r_cols * frac * ang.cos()).round() as i32;
            let r = (self.cy + self.r_rows * frac * ang.sin()).round() as i32;
            (c, r)
        } else {
            (self.cx as i32, self.cy as i32)
        }
    }
}

/// Screen angle (radians, y-down so positive = clockwise) for button i.
fn button_angle(i: u8) -> f32 {
    // Eight buttons 45° apart, rotated half a step (22.5°) so none sits on a
    // cardinal direction. Clockwise from just past top: 1=upper-right, 2=right-
    // upper, 3=right-lower, 4=lower-right (so 1-4 occupy the right half), then
    // 5=lower-left, 6=left-lower, 7=left-upper, 8=upper-left (5-8 left half).
    // The layout is mirrored across both axes → symmetric all around.
    let deg = (i as f32 - 1.0) * 45.0 + 292.5;
    deg.to_radians()
}

/// Touch sensor ring radius as a fraction of the outer button ring `R`.
/// A/D share the outer ring (just inside the buttons); E is the middle ring;
/// B is the inner ring (matches the existing `inner_cols`/`inner_rows`); C is
/// the center (radius 0).
fn zone_radius_frac(zone: Zone) -> f32 {
    match zone {
        Zone::C => 0.0,
        Zone::B => 0.45,
        Zone::E => 0.60,
        Zone::A | Zone::D => 0.80,
    }
}

/// Touch sensor geometry: (screen angle, ring-radius fraction) for a zone+index.
/// A/B sit at the button angles (aligned with the buttons); D/E sit at the
/// midpoints between buttons (22.5° = π/8 CCW of the same-numbered button).
/// C returns (0, 0) — callers special-case it to the center.
fn touch_geometry(zone: Zone, idx: u8) -> (f32, f32) {
    let i = idx.clamp(1, 8).max(1);
    let frac = zone_radius_frac(zone);
    match zone {
        Zone::C => (0.0, 0.0),
        Zone::A | Zone::B => (button_angle(i), frac),
        Zone::D | Zone::E => (button_angle(i) - std::f32::consts::FRAC_PI_8, frac),
    }
}

/// Resolve the start/end screen angles for a ring-arc slide (`>`, `<`, `^`).
/// `>`/`<` follow the start-lane flip rule: the arrow glyph's rotational meaning
/// depends on which half of the playfield the slide starts from (upper half →
/// `>` is clockwise / `<` counter-clockwise; lower half → flipped). `^` takes
/// the shorter arc. Returns (a0, a1) with a0 = ang_from; a1 may be > 2π or < 0
/// for long directed arcs (trig handles it).
fn arc_direction(ang_from: f32, ang_to: f32, shape: SlideShape) -> (f32, f32) {
    let two_pi = 2.0 * std::f32::consts::PI;
    let cw_dist = (ang_to - ang_from).rem_euclid(two_pi);  // clockwise distance
    let ccw_dist = (ang_from - ang_to).rem_euclid(two_pi); // counter-clockwise
    let upper = ang_from.sin() < 0.0; // start in the top half of the playfield
    let (cw, dist) = match shape {
        SlideShape::ArcRight => (upper, if upper { cw_dist } else { ccw_dist }),
        SlideShape::ArcLeft => (!upper, if !upper { cw_dist } else { ccw_dist }),
        SlideShape::AutoCircle => (cw_dist <= ccw_dist, cw_dist.min(ccw_dist)),
        _ => (true, cw_dist),
    };
    let a1 = if cw { ang_from + dist } else { ang_from - dist };
    (ang_from, a1)
}

/// Glyph for a note body (color is resolved separately by `note_color`).
fn note_glyph(brk: bool, _ex: bool, star: StarKind) -> char {
    if star != StarKind::None {
        return if star == StarKind::Spin { '✦' } else { '★' };
    }
    let _ = brk;
    '●'
}

/// Resolve a note's color. Priority: break → orange; simultaneous → yellow;
// star/touch → blue; EX → magenta; default tap → pink.
fn note_color(brk: bool, ex: bool, star: StarKind, sim: bool, is_touch: bool) -> Color {
    if brk {
        return Color::Orange;
    }
    if sim {
        return Color::Yellow;
    }
    if is_touch || star != StarKind::None {
        return Color::Blue;
    }
    if ex {
        return Color::Magenta;
    }
    Color::Pink
}

/// Disc radius (in rows) for a note, scaled up for breaks and stars.
fn note_radius(brk: bool, star: StarKind, is_slide_star: bool) -> f32 {
    if star != StarKind::None || is_slide_star {
        STAR_R
    } else if brk {
        BREAK_R
    } else {
        NOTE_R
    }
}

fn flash_glyph(g: char) -> char {
    match g {
        '●' => '◉',
        '★' => '✦',
        '✦' => '✦',
        _ => g,
    }
}

fn dim(c: Color) -> Color {
    match c {
        Color::Pink => Color::Dim,
        Color::Blue => Color::DimBlue,
        Color::Orange => Color::Dim,
        Color::Magenta => Color::Dim,
        Color::Yellow => Color::Dim,
        _ => Color::Dim,
    }
}

/// Per-event flag: true if ≥2 events share the same `time` (simultaneous / EACH).
/// Events are assumed sorted by time.
fn simultaneous_flags(events: &[NoteEvent]) -> Vec<bool> {
    let mut out = vec![false; events.len()];
    let mut i = 0;
    while i < events.len() {
        let mut j = i + 1;
        while j < events.len() && events[j].time == events[i].time {
            j += 1;
        }
        if j - i >= 2 {
            for k in i..j {
                out[k] = true;
            }
        }
        i = j;
    }
    out
}

/// Arrow glyph for a travel direction (dc, dr) in cell units. Accounts for the
/// ~2:1 char aspect so diagonal arrows read correctly.
fn arrow_for(dc: f32, dr: f32) -> char {
    let dy = dr * 2.0; // aspect: a row is ~2 cols tall
    if dc == 0.0 && dy == 0.0 {
        return '>';
    }
    let ang = dy.atan2(dc); // screen space, y-down (positive = clockwise)
    // Nearest of 8 octants (boundaries at ±22.5°, ±67.5°, …), normalized to 0..7.
    const OCT: f32 = std::f32::consts::FRAC_PI_4;
    let oct = ((ang / OCT).round() as i32 + 8) % 8;
    match oct {
        0 => '>',   // east
        1 => '↘',   // south-east
        2 => 'v',   // south
        3 => '↙',   // south-west
        4 => '<',   // west
        5 => '↖',   // north-west
        6 => '^',   // north
        _ => '↗',   // north-east (7)
    }
}

fn put(cells: &mut [Cell], width: i32, height: i32, c: i32, r: i32, ch: char, color: Color) {
    if c < 0 || c >= width || r < 0 || r >= height {
        return;
    }
    let idx = (r * width + c) as usize;
    if idx < cells.len() {
        cells[idx] = Cell { ch, color };
    }
}

/// Stamp a filled disc (a "proper circle" note) centered at (c,r). Row radius
/// `rrows`, col radius ≈ 2·rrows to respect the ~2:1 char aspect.
fn put_disc(cells: &mut [Cell], width: i32, height: i32, c: i32, r: i32, rrows: f32, ch: char, color: Color) {
    let rcols = rrows * 2.0;
    let rr = rrows.ceil() as i32;
    let rc = rcols.ceil() as i32;
    for dy in -rr..=rr {
        let fy = dy as f32 / rrows;
        let fy2 = fy * fy;
        if fy2 > 1.0 {
            continue;
        }
        for dx in -rc..=rc {
            let fx = dx as f32 / rcols;
            if fx * fx + fy2 <= 1.0 {
                put(cells, width, height, c + dx, r + dy, ch, color);
            }
        }
    }
}

/// One piece of a slide path. Coordinates are in canvas cells (col, row); arc
/// angles are screen radians around the playfield center.
#[derive(Clone, Copy)]
enum Seg {
    /// Straight chord from (fc, fr) to (tc, tr).
    Line(f32, f32, f32, f32),
    /// Elliptical arc on the playfield, centered (cxc, cyr) with col/row radii
    /// (rc, rr), from screen angle a0 to a1 (a1 may exceed 2π / be negative for
    /// directed long arcs).
    Arc { cxc: f32, cyr: f32, rc: f32, rr: f32, a0: f32, a1: f32 },
}

/// Wrap a button index into 1..=8 (0 → 8, 9 → 1).
fn wrap1_8(b: u8) -> u8 {
    if b == 0 { 8 } else if b > 8 { 1 } else { b }
}

/// Geometric length of a segment in cell-ish units (rows count double for the
/// ~2:1 char aspect), used only to spread samples proportionally.
fn seg_len(s: &Seg) -> f32 {
    match s {
        Seg::Line(fc, fr, tc, tr) => {
            let dc = tc - fc;
            let dr = (tr - fr) * 2.0;
            (dc * dc + dr * dr).sqrt()
        }
        Seg::Arc { rc, rr, a0, a1, .. } => {
            let da = (a1 - a0).abs();
            // average aspect-corrected radius × swept angle
            let avg = ((rc + rr * 2.0) / 2.0).max(1.0);
            avg * da
        }
    }
}

/// Position (col, row) and continuous travel direction (dc, dr) at parameter
/// `t ∈ [0,1]` along a segment. Direction is in raw cell units (the caller
/// applies the 2:1 aspect when orienting arrows).
fn seg_point_dir(s: &Seg, t: f32) -> (f32, f32, f32, f32) {
    match s {
        Seg::Line(fc, fr, tc, tr) => {
            let c = fc + (tc - fc) * t;
            let r = fr + (tr - fr) * t;
            (c, r, tc - fc, tr - fr)
        }
        Seg::Arc { cxc, cyr, rc, rr, a0, a1 } => {
            let a = a0 + (a1 - a0) * t;
            let c = cxc + rc * a.cos();
            let r = cyr + rr * a.sin();
            // Tangent d(pos)/dt; sign follows (a1 - a0).
            let da = a1 - a0;
            let dc = -rc * a.sin() * da;
            let dr = rr * a.cos() * da;
            (c, r, dc, dr)
        }
    }
}

/// Build the geometric segments for one slide leg, per the simai shape spec.
/// `B(b)` = button cell, `I(θ)` = inner-ring point at angle θ, `R` = outer ring.
fn slide_segments(r: &Renderer, part: &crate::chart::SlidePart) -> Vec<Seg> {
    let cx = r.cx;
    let cy = r.cy;
    let rc = r.r_cols;
    let rr = r.r_rows;
    let bpt = |b: u8| -> (f32, f32) {
        let (c, ro) = r.button_pos[b as usize];
        (c as f32, ro as f32)
    };
    let ang_from = button_angle(part.from);
    let ang_to = button_angle(part.to);
    let two_pi = 2.0 * std::f32::consts::PI;

    let (fc, fr) = bpt(part.from);
    let (tc, tr) = bpt(part.to);

    match part.shape {
        SlideShape::Line => vec![Seg::Line(fc, fr, tc, tr)],

        SlideShape::ArcRight | SlideShape::ArcLeft | SlideShape::AutoCircle => {
            let (a0, a1) = arc_direction(ang_from, ang_to, part.shape);
            vec![Seg::Arc { cxc: cx, cyr: cy, rc, rr, a0, a1 }]
        }

        // V-shape: polyline bending through the center.
        SlideShape::V => vec![
            Seg::Line(fc, fr, cx, cy),
            Seg::Line(cx, cy, tc, tr),
        ],

        // L-shape: polyline bending through the turning-point button (fall back
        // to a V through the center if the turning digit is missing).
        SlideShape::VBig => {
            if part.turn == 0 {
                vec![Seg::Line(fc, fr, cx, cy), Seg::Line(cx, cy, tc, tr)]
            } else {
                let (mc, mr) = bpt(part.turn);
                vec![Seg::Line(fc, fr, mc, mr), Seg::Line(mc, mr, tc, tr)]
            }
        }

        // U-loop around the center on a small (p/q) or large (pp/qq) inner ring.
        // `p` = ccw, `q` = cw; the loop covers the directed distance from→to.
        SlideShape::P | SlideShape::Q | SlideShape::PP | SlideShape::QQ => {
            let (ri_c, ri_r) = match part.shape {
                SlideShape::PP | SlideShape::QQ => (rc * 0.60, rr * 0.60),
                _ => (rc * 0.30, rr * 0.30),
            };
            let cw = matches!(part.shape, SlideShape::Q | SlideShape::QQ);
            let dist = if cw {
                (ang_to - ang_from).rem_euclid(two_pi)
            } else {
                (ang_from - ang_to).rem_euclid(two_pi)
            };
            let a1 = if cw { ang_from + dist } else { ang_from - dist };
            let ifrom = (cx + ri_c * ang_from.cos(), cy + ri_r * ang_from.sin());
            let ito = (cx + ri_c * ang_to.cos(), cy + ri_r * ang_to.sin());
            vec![
                Seg::Line(fc, fr, ifrom.0, ifrom.1),
                Seg::Arc { cxc: cx, cyr: cy, rc: ri_c, rr: ri_r, a0: ang_from, a1 },
                Seg::Line(ito.0, ito.1, tc, tr),
            ]
        }

        // Thunder zigzag: 3-segment polyline with perpendicular offsets that
        // alternate sign between `s` (ccw bulge) and `z` (cw, mirrored).
        SlideShape::S | SlideShape::Z => {
            let vx = tc - fc;
            let vy = tr - fr;
            // Perpendicular to the chord, aspect-aware (rows count double).
            let pc = -vy * 2.0;
            let pr = vx / 2.0;
            let plen = (pc * pc + (pr * 2.0) * (pr * 2.0)).sqrt().max(1e-6);
            let d = 0.25 * (vx * vx + (vy * 2.0) * (vy * 2.0)).sqrt();
            let sgn = if matches!(part.shape, SlideShape::S) { 1.0 } else { -1.0 };
            let off_c = sgn * pc / plen * d;
            let off_r = sgn * pr / plen * d;
            let p1 = (fc + vx / 3.0 + off_c, fr + vy / 3.0 + off_r);
            let p2 = (fc + 2.0 * vx / 3.0 - off_c, fr + 2.0 * vy / 3.0 - off_r);
            vec![
                Seg::Line(fc, fr, p1.0, p1.1),
                Seg::Line(p1.0, p1.1, p2.0, p2.1),
                Seg::Line(p2.0, p2.1, tc, tr),
            ]
        }

        // Fan / WiFi: a stem from `from` straight to button `to−1`, then a ring
        // arc sweeping `to−1` → `to+1` through `to` (covers the 3-lane fan).
        SlideShape::W => {
            let tm1 = wrap1_8(part.to.wrapping_sub(1));
            let tp1 = wrap1_8(part.to.wrapping_add(1));
            let (mc, mr) = bpt(tm1);
            let ang_m = button_angle(tm1);
            let ang_p = button_angle(tp1);
            // Short arc through `to` (tm1, to, tp1 are consecutive clockwise).
            let cw_dist = (ang_p - ang_m).rem_euclid(two_pi);
            let ccw_dist = (ang_m - ang_p).rem_euclid(two_pi);
            let (a0, a1) = if cw_dist <= ccw_dist {
                (ang_m, ang_m + cw_dist)
            } else {
                (ang_m, ang_m - ccw_dist)
            };
            vec![
                Seg::Line(fc, fr, mc, mr),
                Seg::Arc { cxc: cx, cyr: cy, rc, rr, a0, a1 },
            ]
        }
    }
}

/// Sample one slide leg into a connected list of (col, row, dir_c, dir_r)
/// tuples from `from`→`to`. The direction is the continuous travel direction
/// at that point (in cell units, before aspect correction) so arrows orient
/// smoothly even on diagonals and across segment joins.
fn sample_path(r: &Renderer, part: &crate::chart::SlidePart) -> Vec<(i32, i32, f32, f32)> {
    let segs = slide_segments(r, part);
    if segs.is_empty() {
        return Vec::new();
    }
    let total: f32 = segs.iter().map(seg_len).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    // Overall sample resolution tied to path length so curves stay connected.
    let n_total = (total.ceil() as i32 + 2).max(8) as usize;
    let mut pts: Vec<(i32, i32, f32, f32)> = Vec::with_capacity(n_total + segs.len());
    let mut prev: Option<(i32, i32)> = None;
    for s in &segs {
        let sl = seg_len(s);
        if sl <= 0.0 {
            continue;
        }
        let n = ((n_total as f32) * sl / total).round().max(2.0) as usize;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let (c, ro, dc, dr) = seg_point_dir(s, t);
            let cell = (c.round() as i32, ro.round() as i32);
            if prev != Some(cell) {
                pts.push((cell.0, cell.1, dc, dr));
                prev = Some(cell);
            }
        }
    }
    pts
}

/// Truncate a string to at most `max_chars` visible chars, on a UTF-8 char
/// boundary, so it never exceeds the canvas width and wraps.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

/// Draw a clean ellipse outline: two points per row, so the outline is
/// connected and reads as a circle (with 2:1 char aspect via rx ≈ 2·ry).
fn draw_ellipse(
    cells: &mut [Cell],
    width: i32,
    height: i32,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    glyph: char,
    color: Color,
) {
    let top = (cy - ry).round() as i32;
    let bot = (cy + ry).round() as i32;
    for r in top..=bot {
        let dy = (r as f32 - cy) / ry;
        if dy.abs() > 1.0 {
            continue;
        }
        let dx = rx * (1.0 - dy * dy).sqrt();
        put(cells, width, height, (cx - dx).round() as i32, r, glyph, color);
        put(cells, width, height, (cx + dx).round() as i32, r, glyph, color);
    }
}