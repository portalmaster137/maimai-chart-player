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
    inner_cols: f32,
    inner_rows: f32,
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
        let inner_cols = r_cols * 0.45;
        let inner_rows = r_rows * 0.45;
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
        // Clean outer + inner ellipse outlines (2 points per row → connected circle).
        draw_ellipse(&mut static_layer, width, height, cx, cy, r_cols, r_rows, '.', Color::Dim);
        draw_ellipse(&mut static_layer, width, height, cx, cy, inner_cols, inner_rows, ',', Color::Dim);
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
            inner_cols,
            inner_rows,
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
                self.draw_tap(cells, ev, now, window, *brk, *ex, StarKind::None, sim, false);
                // During [time, end] the hold extends as a bar from the button
                // inward toward the center, retracting as time elapses (the bar's
                // length = remaining fraction of the hold). At hit the bar is full;
                // at end it has retracted to just the head disc at the button.
                if now >= ev.time && now <= *end && matches!(ev.pos, Position::Button(_)) {
                    let (tc, tr, sc, sr, _, _) = self.spawn_target(ev.pos);
                    let color = note_color(*brk, *ex, StarKind::None, sim, false);
                    let rdisc = if *brk { BREAK_R } else { NOTE_R };
                    let dur = (*end - ev.time).max(0.001);
                    let p = ((now - ev.time) / dur).clamp(0.0, 1.0);
                    let frac = 1.0 - p; // remaining hold → bar length
                    self.draw_hold_tail(cells, tc, tr, sc, sr, frac, rdisc * 0.7, color);
                    // Head disc at the button (skip during the brief flash window
                    // so draw_tap's flash glyph stays visible at the hit moment).
                    if now > ev.time + FLASH {
                        put_disc(cells, self.width, self.height, tc, tr, rdisc, '●', color);
                    }
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
                    self.draw_slide_leg(cells, part, now, *brk, head_color);
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
            // Hit: hold the shutter closed briefly, then it goes away.
            self.draw_shutter(cells, target_c, target_r, 1.0, color);
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

    /// Draw a hold's tail: a bar of filled discs from the button (tc,tr) inward
    /// toward the spawn point (sc,sr), covering `frac` of that distance (the
    /// remaining hold fraction). The button cell itself is left untouched so the
    /// head/flash glyph there is preserved. `rrows` sets the bar thickness.
    fn draw_hold_tail(
        &self,
        cells: &mut [Cell],
        tc: i32,
        tr: i32,
        sc: f32,
        sr: f32,
        frac: f32,
        rrows: f32,
        color: Color,
    ) {
        if frac <= 0.0 {
            return;
        }
        let dc = sc - tc as f32;
        let dr = sr - tr as f32;
        // Cell-distance estimate (cols dominate at 2:1 aspect); step ~0.5 cell.
        let dist = dc.abs().max(dr.abs() * 2.0).max(1.0);
        let n = ((dist * frac) * 2.0).ceil() as i32;
        for i in 1..=n {
            let t = (i as f32 / n as f32) * frac; // (0, frac]
            let c = tc as f32 + dc * t;
            let r = tr as f32 + dr * t;
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
                let ang = touch_angle(zone, idx);
                let tc = (self.cx + self.inner_cols * ang.cos()).round() as i32;
                let tr = (self.cy + self.inner_rows * ang.sin()).round() as i32;
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
            let ang = touch_angle(zone, idx);
            let c = (self.cx + self.inner_cols * ang.cos()).round() as i32;
            let r = (self.cy + self.inner_rows * ang.sin()).round() as i32;
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

fn touch_angle(zone: Zone, idx: u8) -> f32 {
    match zone {
        Zone::C => 0.0, // center (unused via radius)
        _ => {
            // Map touch index 1..8 to the same 8 angles as buttons.
            let i = idx.clamp(1, 8).max(1);
            button_angle(i)
        }
    }
}

/// Pick the shorter arc direction for `<` (ccw) and `>` (cw).
fn arc_direction(ang_from: f32, ang_to: f32, shape: SlideShape) -> (f32, f32) {
    // Normalize delta to (-2π, 2π).
    let mut delta = ang_to - ang_from;
    while delta > std::f32::consts::PI {
        delta -= 2.0 * std::f32::consts::PI;
    }
    while delta < -std::f32::consts::PI {
        delta += 2.0 * std::f32::consts::PI;
    }
    match shape {
        SlideShape::ArcRight => (ang_from, ang_from + delta.abs()), // clockwise
        SlideShape::ArcLeft => (ang_from, ang_from - delta.abs()),  // counter-clockwise
        _ => (ang_from, ang_from + delta),
    }
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

/// Sample one slide leg into a connected list of (col, row, dir_c, dir_r)
/// tuples from `from`→`to`. The direction is the continuous travel direction
/// at that point (in cell units, before aspect correction) so arrows orient
/// smoothly even on diagonals.
fn sample_path(r: &Renderer, part: &crate::chart::SlidePart) -> Vec<(i32, i32, f32, f32)> {
    let (from_c, from_r) = r.button_pos[part.from as usize];
    let (to_c, to_r) = r.button_pos[part.to as usize];
    // Estimate path length in cells to pick a sample count that stays connected.
    let len = match part.shape {
        SlideShape::ArcRight | SlideShape::ArcLeft => {
            let ang_from = button_angle(part.from);
            let ang_to = button_angle(part.to);
            let (_, a1) = arc_direction(ang_from, ang_to, part.shape);
            let ang = (a1 - ang_from).abs();
            (r.r_cols * ang).max(r.r_rows * ang)
        }
        _ => {
            let dc = to_c - from_c;
            let dr = (to_r - from_r) * 2; // aspect
            ((dc * dc + dr * dr) as f32).sqrt()
        }
    };
    let n = (len.ceil() as i32 + 2).max(8) as usize;
    let mut pts = Vec::with_capacity(n + 1);
    let mut prev: Option<(i32, i32)> = None;
    for k in 0..=n {
        let p = k as f32 / n as f32;
        let (cc, rr, dc, dr) = point_and_dir(r, part, p, from_c, from_r, to_c, to_r);
        if prev != Some((cc, rr)) {
            pts.push((cc, rr, dc, dr));
            prev = Some((cc, rr));
        }
    }
    pts
}

/// Position and continuous travel direction along a leg at progress p ∈ [0,1].
fn point_and_dir(
    r: &Renderer,
    part: &crate::chart::SlidePart,
    p: f32,
    from_c: i32,
    from_r: i32,
    to_c: i32,
    to_r: i32,
) -> (i32, i32, f32, f32) {
    match part.shape {
        SlideShape::ArcRight | SlideShape::ArcLeft => {
            let ang_from = button_angle(part.from);
            let ang_to = button_angle(part.to);
            let (a0, a1) = arc_direction(ang_from, ang_to, part.shape);
            let a = a0 + (a1 - a0) * p;
            let c = r.cx + r.r_cols * a.cos();
            let row = r.cy + r.r_rows * a.sin();
            // Tangent d(pos)/dp along the arc (direction of increasing p).
            let da = a1 - a0;
            let dc = -r.r_cols * a.sin() * da;
            let dr = r.r_rows * a.cos() * da;
            (c.round() as i32, row.round() as i32, dc, dr)
        }
        _ => {
            let c = from_c as f32 + (to_c as f32 - from_c as f32) * p;
            let row = from_r as f32 + (to_r as f32 - from_r as f32) * p;
            // Straight chord: constant direction.
            let dc = (to_c - from_c) as f32;
            let dr = (to_r - from_r) as f32;
            (c.round() as i32, row.round() as i32, dc, dr)
        }
    }
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