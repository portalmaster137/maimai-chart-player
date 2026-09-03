# STATE.md — MaiMai Terminal Chart Player

Living reference for development. Update the progress checklist as work proceeds.

## Goal
A terminal (console) maimai chart player in Rust. Given a directory with
`maidata.txt` (simai chart) + `track.mp3`, it parses the simai notation, plays the
audio, and renders an ASCII circle with the maimai sensor grid whose notes move in
sync with the song. Notes spawn near the center and travel **outward** to their
button on the outer ring, flashing at hit time. Speed factor `--speed 1..10`.

## Test case
`/home/porta/Code/MaimaiData/maimai DX PRiSM PLUS/11820/`
- `maidata.txt` (1040 lines) + `track.mp3` (~10MB, 320kbps/44.1kHz) + `bg.png`
- Song: *Xaleid◆scopiX[DX]* by xi. BPM ramp 120→240 (wholebpm=180).
- Difficulties present: 2=STD(7.9), 3=HRD(11.0), 4=MAST(13.7), 5=REIM(14.9), 6=UPR(15.0). No EZ.

## Architecture
```
src/
  main.rs     # entry: CLI -> loader -> player
  cli.rs      # clap args + interactive difficulty menu
  maidata.rs  # parse &key=value header + &inote_N blocks -> raw measures
  simai.rs    # tokenize + parse simai note grammar -> NoteEvent list
  chart.rs    # NoteEvent/Kind/Position model; timing resolution walk
  render.rs   # ASCII circle canvas + per-frame note drawing
  bg.rs       # background art (bg.png/jpg/mp4) -> dim ASCII backdrop layer
  player.rs   # master clock, audio start, sync/render loop, key handling
```
Dependencies: `rodio` (symphonia-mp3), `crossterm`, `clap` (derive).

## Parsing reference (simai)
- Header `&key=value`. Relevant: `&title`, `&wholebpm`, `&first` (offset secs —
  **absent here** → default 0, use `--offset`), `&lv_N`, `&des_N`, `&inote_N`.
- Chart = measures, one per line: `(BPM){divisor}slot1,slot2,...,`. `(BPM)` and
  `{N}` optional, may appear **inline between commas** (e.g. `(230){2},(240),`).
- **Timing:** `seconds_per_step = 240 / (cur_bpm * cur_divisor)`. Walk slots:
  apply leading `(BPM)`/`{N}`, emit note at `cur_time`, advance by step. Empty=rest.
- **Tokens** (per slot, `/`-separated = simultaneous EACH):
  - Tap: `1..8` + `b`(break)/`x`(EX)/`$`/`$$`(star). Adjacent `12,` = simultaneous.
  - Hold: `Nh[A:B]` (+`b`/`x`). dur = `B * 240/(bpm*A)`. `Nh[#s]` = absolute secs.
  - Slide: `<src><shape><dst>[A:B]`, shapes `- > < ^ p q w v V z s` (+`pp`/`qq`),
    `V` carries a turning-point digit (`aVbc` = via `b` to `c`). `*` chains,
    multi-point `a-b-c`. Star tap at `time`; **motion starts one quarter note
    later** = `time + 60/bpm`; lasts bracket duration.
  - Touch: `A1..A8 B1..B8 D1..D8 E1..E8 C`. `f`=firework. `Ch[A:B]`=center hold.
- End marker: line containing `E`.

## Rendering model
- Canvas autoscales to the terminal: `canvas_dims(tw,th) = (tw-1, th-2)` (one
  safety column so the last char never auto-wraps, two rows reserved for the
  HUD). The circle is centered in this canvas and its radius fits BOTH dims at
  ~2:1 aspect: `r_rows = min(h/2-2, w/4-1).max(4)`, `r_cols = 2*r_rows`. So it
  grows on big terminals, shrinks on small ones, and never overflows either axis.
- Resize handling: the render loop re-checks `terminal::size()` every frame and
  rebuilds the `Renderer` when it changes — resizing the window mid-song works.
- Frame emission uses `\r\n` between rows (raw mode disables OPOST, so bare `\n`
  does NOT carriage-return → staircase/wrap). The last HUD line has no trailing
  newline so the cursor never advances past the bottom row (no scroll). HUD and
  keys lines are char-boundary truncated to the canvas width (long titles with
  multibyte glyphs like `◆` won't wrap).
- Outer ring radius R; inner spawn radius r0 (~1.5 cells, "near center but not
  exactly"). Buttons 1..8 are 45° apart but **rotated half a step (22.5°) off the
  cardinals** so no button sits at top/right/bottom/left — clockwise with 1-4 on
  the right half and 5-8 on the left half, mirrored across both axes:
  `  8 1  / 7   2 / 6  C  3 / 5 4  ` (each pair straddles the cardinal, none on it).
- **Touch sensor zones** emulate maimai DX's 5 concentric zone groups (34
  sensors): A (outer, aligned with buttons), B (inner, aligned with buttons),
  D (outer, *between* buttons), E (middle, *between* buttons), C (center). The
  static layer draws 4 concentric reference rings — outer button ring `.`
  (**white** so it reads against the bg backdrop) / A·D ring `·` (0.80·R) /
  E ring `∘` (0.60·R) / B ring `,` (0.45·R) — plus the
  `C` marker. Each touch note renders at its zone's radius × angle:
  - A_i, B_i at `button_angle(i)` (the 8 button angles).
  - D_i, E_i at `button_angle(i) − π/8` (the midpoints between buttons → the
    cardinal/inter-cardinal directions in our 22.5°-offset layout).
  - C at the center (radius 0).
  Radii (0.80 / 0.60 / 0.45 of R) are terminal approximations of maimai's true
  sensor radii; A and D share the 0.80·R ring at alternating angles.
- Outward travel: window `W = BASE/speed` (BASE tuned so speed 5 ≈ 1.2s).
  progress `p=(now-(t-W))/W`; pos = lerp(inner_for_button, button_pos, p).
  Flash at p≈1 for FLASH secs, then fade.
- **Notes are filled circle discs** (`put_disc`, 2:1 aspect), sized up:
  tap/ex `●` r=1.2, break `●` r=1.5, star `★` r=1.3, touch `●` r=1.0, flash `◉` r=1.7.
  Dim trail disc behind while approaching.
- **Holds grow during approach then retract.** The hold bar appears the moment
  the note spawns and grows with the approaching head: during `[time-window,
  time]` the bar grows from the (near-center) spawn point toward the button
  (`frac = p_a`, `p_a = (now-(time-window))/window`), with the head disc at its
  tip (drawn by `draw_tap` at `lerp(spawn, button, p_a)`). At hit the bar is full
  (spawn→button) under the flash. During `[time, end]` the bar retracts toward
  the button (`frac = 1 - p`, `p = (now-time)/(end-time)`) — the inner end pulls
  from spawn toward the button — reaching zero at `end`. After the flash a head
  disc is sustained at the button through `end`. Thickness ≈ 0.7× the note
  radius (break bars are thicker/orange). Ring holds only (`Kind::Hold`); touch
  holds use the rainbow-body arm below.
- **Color scheme** (`note_color`, priority order):
  1. break → **orange** (256-color `\x1b[38;5;208m`) — always, even if simultaneous.
  2. simultaneous (≥2 notes at the same `time`) → **yellow** (`\x1b[93m`).
  3. star / touch → **blue** (`\x1b[94m`).
  4. EX → **magenta**.
  5. default tap → **pink** (256-color `\x1b[38;5;213m`).
  Firework touches are yellow. `simultaneous_flags` groups sorted events by equal
  `time` and marks groups of ≥2.
- **Star slides fade in then trace:** each leg builds a list of geometric
  **segments** (`slide_segments` → `Seg::Line` / `Seg::Arc`), samples them into a
  connected cell path (`sample_path` → (col,row,dir_c,dir_r)) proportional to each
  segment's length, and is drawn as **little arrows** (`arrow_for`) oriented along
  the continuous travel direction (8 octants, aspect-corrected): `> < ^ v ↘↙↖↗`.
  During `[motion_start-FADE_IN, motion_start]` the full path fades in as dim
  arrows (gray→dim-blue). During `[motion_start, motion_end]` the star head
  (`★` disc, blue/orange/yellow) travels start→end; arrows behind it light up
  bright (blue, or orange if break) and stay lit, arrows ahead stay dim.
- **Slide shape geometry** (per simai spec; `B(b)`=button cell, `I(θ)`=inner-ring
  point, `R`=outer ring radius):
  - `-` straight chord `B(from)→B(to)`.
  - `>`/`<` ring **arc**, directed by the **start-lane flip rule**: start in the
    upper half (`button_angle(from).sin() < 0`) → `>` clockwise / `<` ccw; lower
    half → flipped. Arc length = the directed distance (can exceed 180° — e.g.
    `1<4` sweeps ccw 225° through 8/7/6/5, the long way).
  - `^` auto ring arc — **shortest** direction.
  - `v` V-shape: polyline `B(from)→center→B(to)`.
  - `V` L-shape: polyline `B(from)→B(turn)→B(to)` via the turning-point button
    (`turn` field; falls back to `v` if absent).
  - `p`/`q` U-loop: `B(from)→I(from)`, inner-ring arc (radius `0.30·R`, ccw for
    `p` / cw for `q`, directed distance), `I(to)→B(to)`.
  - `pp`/`qq` CUP: same as `p`/`q` but inner-ring radius `0.60·R` (bigger loop).
  - `s`/`z` thunder zigzag: 3-segment polyline `B(from)→P1→P2→B(to)` with
    perpendicular offset `0.25·|chord|`; `s` bulges one way, `z` mirrored.
  - `w` fan/WiFi: **three rays that expand simultaneously** from `B(from)` to
    the three consecutive sensors `B(to−1)`, `B(to)`, `B(to+1)` — the star splits
    at the source and the fan opens like a hand fan. Straight chord rays (not
    the old stem+arc). See "Fan slides" below.
  Arc tangent for arrow orientation: `(−rc·sin a·da, rr·cos a·da)` with
  `da = a1−a0`. These are terminal approximations of maimai's true curves
  (especially `p`/`q`/`pp`/`qq`/`s`/`z`), but each now renders its
  distinctive shape instead of collapsing to a straight chord.
- **Fan slides (`w`) split into three expanding rays** — they do *not* use the
  single-path trace. `draw_fan` (called from the `Kind::Slide` arm for
  `SlideShape::W` instead of `draw_slide_leg`) draws three straight chord rays
  from `B(from)` to `B(to−1)`, `B(to)`, `B(to+1)` (`wrap1_8` for the 1..=8
  wrap). Three phases mirror the slide fade-in/trace model:
  1. **Fade-in** `[motion_start−FADE_IN, motion_start]`: all three full rays
     appear dimly (gray→dim color) plus a dim `★` hub at the source.
  2. **Expand** `[motion_start, motion_end]`: three heads travel outward
     simultaneously. Each ray is **bright** (`head_color`) from source→tip and
     **dim** (`dim(head_color)`) from tip→destination; a `★` head rides each
     tip. `p = (now−motion_start)/(motion_end−motion_start)` → all three tips at
     `lerp(source, dest_i, p)`. The fan visibly opens.
  3. **Fully open** `[motion_end, motion_end+FLASH]`: hold the full bright fan
     and flash a big `✦` disc (`FLASH_R`) at each of the three destination
     sensors — the "landed on three sensors" climax. After that, nothing.
  Rays are thick filled `●` bars (`draw_bar`, `RAY_R=0.9`) so the fan reads as
  a bold solid shape; a bright `★` hub marks the pivot. Color is `head_color`
  (blue star / orange break / yellow simultaneous), so break fans are orange
  and simultaneous fans are yellow with no extra wiring.
- **Touch notes are shutters, not center travelers.** A touch appears at its own
  zone position as a small square shutter: four pyramids (▲▼◀▶) around the hit
  point that close in as `p→1` (open at the square's edges, tips meet at hit),
  then briefly hold closed and go away. The appear→close window is `BASE/speed`,
  so faster speed = shorter shutter (speed 1 ≈ 6s, speed 5 ≈ 1.2s, speed 10 ≈
  0.6s). During approach the shutter is colored (blue, or yellow if
  simultaneous/firework). **At the hit moment** (`now ∈ [t, t+FLASH]`) the closed
  shutter flashes **white** (`\x1b[97m`) as a "hit now" border, replacing the
  approach color. Touch holds close the shutter at hit, then sustain an `h` disc
  through `[t, end]`.
- **Touch holds are rainbow with a clockwise timer ring.** They approach like a
  touch tap (closing shutter), then during `[time, end]` a big rainbow body sits
  at the touch point and a ring sweeps clockwise from the top (12 o'clock) as a
  hold-duration indicator. **Center touch holds (`Ch` / Zone::C) render exactly
  at C** (radius 0, not on the inner ring); non-center touch holds use their
  zone's inner-ring spot ("usually center, not always"). Body radius
  `TOUCH_HOLD_R=1.7` (nice and big), timer ring radius `HOLD_RING_R=2.6` around
  it. Sweep progress `p = (now-time)/(end-time)` → the arc covers `p` of a full
  turn (full circle at `end`). The body hue cycles slowly with time; the arc is
  rainbow-colored along its length (24-step 256-color wheel:
  red→orange→yellow→green→cyan→blue→magenta).

## CLI
`maimai-player <DIR> [-d <N>] [--offset <secs>] [--speed <1-10>] [--bg <0-10>] [--bg-style <ramp|cells>]`
- No `-d` → interactive numbered menu (level + charter). Defaults: speed 5, offset 0, bg 5, bg-style ramp.
- Keys: `q`/Esc quit, `+`/`-` offset nudge, `↑`/`↓` speed, `p` pause, `m` mute, `b` cycle bg level (0→1→…→10→0).

## Background art (bg.rs)
- **Detection** (`Bg::detect`, in `main.rs` *before* the alternate screen so warnings
  are visible): candidates in order bg.mp4/.webm/.mov/.avi (video preferred), then
  bg.png/.jpg/.jpeg. `--bg 0` skips detection entirely. Never fails — one stderr
  warning, then a disabled `Bg`.
- **Composition**: `Renderer::set_bg_layer` installs a `bg_layer` of `Cell`s
  (width×height) with the static ring/label glyphs stamped on top; `frame()` clones
  `bg_layer` instead of `static_layer` when non-empty. Painter order stays
  **bg → notes**: `put`/`put_disc` overwrite only `ch`+fg (preserving any cell
  background), so notes and rings paint on top of the backdrop.
- **Char-ramp style** (`--bg-style ramp`): luminance
  (Rec.709) → `RAMP = [' ','.',',',':',';','-','=','+','*','#','%','@']` (12 levels
  matching `GRAYS`, codes 232..=243 — all darker than `Color::Dim` #7f7f7f, so
  rings/labels and dim trails stay legible). `max_index(level) = 11*level/10` (min
  1): level 5 caps at code 237, level 10 reaches 243. Mapping:
  `lum^0.9 * 0.95 + 0.02` scaled — black stays blank (ramp index 0 = space).
- **Truecolor-cells style** (`--bg-style cells`, needs a 24-bit-color terminal):
  space chars with dimmed truecolor painted into `Cell.bg` (`Option<[u8; 3]>`).
  `--bg` scales brightness linearly (level 10 ≈ 45% of original), then each channel
  is quantized to 5 bits (`& 0xF8 | 0x04`) so neighbors merge into longer SGR runs
  (a per-cell 24-bit escape every frame is expensive to emit). Frame emission
  tracks fg and bg separately: default fg mid-row is `\x1b[39m` (never `\x1b[0m`,
  which would wipe the bg), bg changes emit `\x1b[48;2;r;g;bm` / `\x1b[49m`, and
  `\x1b[49m` precedes each row's `\x1b[K` so the 1-col safety margin erases with
  terminal-default background. Output is ~3× the ramp style (~30KB/frame).
- **2:1 char aspect**: the pixel buffer is `w × 2h` (each cell = 2 vertical
  pixels); `cells_from_pixels(px, pw, ph, cw, ch, bpp, level, style)` averages each
  cell's source-space pixel block (`bpp`: 4 = RGBA with alpha composited over
  black, 3 = rgb24 from the video pipe) and dispatches on `BgStyle`.
- **Images**: `image` crate (png+jpeg features), `DynamicImage::resize_to_fill`
  cover-crop, cached per canvas size → zero per-frame cost.
- **Video** (ffmpeg CLI, no video crates): spawned per canvas size as
  `ffmpeg -hide_banner -loglevel error -nostdin -re -an -sn -dn -i bg.mp4 -vf
  fps=12,scale=W:2H:force_original_aspect_ratio=increase,crop=W:2H -f rawvideo
  -pix_fmt rgb24 pipe:1` with **stdin/stderr nulled** (inheriting the raw-mode TTY
  would steal keystrokes). A reader thread `read_exact`s frame-sized chunks into
  `Arc<Mutex<Slot{frame, seq, ended}>>`; the 60fps loop re-converts only when
  `seq` changes (~12 Hz). `-re` streams at native rate (without it ffmpeg dumps
  the whole file instantly and the "animation" is over in a blink). Resize kills +
  respawns the pipe; `Drop` kills + waits (no zombie). Missing ffmpeg or pipe
  failure → one warning, play without bg. Pause freezes the video for free (the
  loop stops draining the pipe, ffmpeg blocks on write).
- **Style switch**: `BgStyle::CharRamp | TruecolorBg` (`--bg-style ramp|cells`).
- The player pushes `bg.layer(cw, ch)` into the renderer every frame when `Some`
  (level change, resize, or new video frame); `Bg` tracks `last_dims` so a resize
  always re-pushes to the freshly rebuilt `Renderer`.

## Progress checklist
- [x] Scaffold + Cargo.toml + STATE.md (cargo build clean)
- [x] maidata header parser + &inote_N block extraction
- [x] simai tokenizer (taps/breaks/ex/star/hold/slide/touch)
- [x] timing resolution walk (inline BPM/{}, 240/(bpm*div))
- [x] NoteEvent model with resolved hold/slide end + slide motion start
- [x] CLI args + interactive difficulty menu (--dump/--demo/--no-audio helpers)
- [x] render: canvas + 8-button circle + center + inner touch ring
- [x] render: tap outward-travel + flash
- [x] render: break/EX/star colors+glyphs
- [x] render: hold sustained marker
- [x] render: slide moving star + path (segment-based geometry: `-` line,
      `>`/`<`/`^` ring arcs with flip-rule/shortest direction, `v` V through
      center, `V` L through turning button, `p`/`q`/`pp`/`qq` U/CUP loops on
      inner ring, `s`/`z` thunder zigzag, `w` fan/WiFi stem+arc sweep)
- [x] render: touch zones (inner ring, green + / yellow F firework)
- [x] render: 5 touch sensor zones emulated at distinct radii/angles
      (A outer 0.80·R + B inner 0.45·R at button angles; D outer 0.80·R + E middle
      0.60·R at the midpoint/cardinal angles; C center) with 4 concentric
      reference rings drawn on the static layer
- [x] player: rodio audio playback + master clock (audio optional via --no-audio)
- [x] player: 60fps sync loop + key handling + terminal restore (TermGuard)
- [x] integrate main.rs, tune BASE/FLASH/speed (BASE=6 → speed5≈1.2s)
- [x] verify -d 4 and -d 5 (0 parser warnings), pty run confirms notes travel + flash
- [x] autoscale/resize: canvas fills terminal, circle fits both dims at 2:1,
      `\r\n` row separation (no raw-mode staircase/wrap), live resize rebuild,
      HUD truncated to width
- [x] notes as filled circle discs (sized up); break = orange; star slides
      fade-in full path then trace out with bright trail + traveling head
- [x] touch notes: square shutter (4 pyramids ▲▼◀▶ closing in to hit) instead
      of center-travel; window scales with speed (faster = shorter)
- [x] color scheme: default tap pink, star+touch+slide-line blue, simultaneous
      ≥2 yellow, break orange; slide path drawn as little arrows (`>↘v↙<↖^↗`)
      oriented along travel direction
- [x] button layout rotated 22.5° off cardinals: 1-4 right half, 5-8 left half,
      clockwise, equal 45° spacing, symmetric (no button at N/E/S/W)
- [x] holds extend as a retracting bar (button → near-center) instead of an `H`
      marker; length = remaining hold fraction, thick/orange for break
- [x] holds grow during approach (bar appears the moment the note spawns and
      grows with the head from spawn→button), full at hit, retract during hold
- [x] touch holds: rainbow body at the touch point (center C usually) + a small
      ring sweeping clockwise from top as a hold-duration indicator (24-step
      256-color rainbow wheel)
- [x] touch taps: white border at the hit moment — the closed shutter flashes
      bright white (`\x1b[97m`) during `[t, t+FLASH]` to mark when to hit;
      approach stays blue/yellow
- [x] fan (`w`) slides: split into three rays that expand simultaneously from
      the source button to sensors `to−1`/`to`/`to+1` (a big opening fan), with
      fade-in → expand → fully-open `✦` flashes at the three destinations;
      thick `●` rays + `★` hub/heads; color via `head_color` (orange break /
      yellow simultaneous / blue star)
- [x] background art (bg.rs): bg.png/jpg cover-cropped to a dim grayscale
      char-ramp backdrop (codes 232..=243) behind the notes; bg.mp4/webm/mov/avi
      decoded via an ffmpeg rawvideo pipe (12fps, `-re` real-time, stdin/stderr
      nulled, reader thread + respawn on resize, Drop kills the child);
      `--bg 0-10` dim level + `b` key cycling + HUD label
- [x] bg style switch: `--bg-style ramp` (grayscale chars) and `--bg-style cells`
      (dimmed 5-bit-quantized truecolor painted into cell backgrounds; needs a
      24-bit-color terminal) — fg emission uses `\x1b[39m` mid-row and `\x1b[49m`
      precedes `\x1b[K` so cell backgrounds survive; `put` preserves cell bg

## Verification status
- Parser: all 5 difficulties parse with **0 warnings** (STD 588 / HRD 980 / MAST 1265
  / REIM 2001 / UPR 2093 events). Timing verified against the MAST intro
  (`{8}4,4,6,6,...` → 0.25s gaps at 120bpm; `1bh[1:2]`@t=4 → end=8.0).
- Renderer: `--demo` and live pty runs show the sensor grid, outward-traveling taps,
  break-hold `H` sustaining, touch `+` on the inner ring, slide stars.
- Live loop: confirmed under a pty (`script`); `q` quits and restores the terminal.
- Audio: rodio path compiles and is exercised; this sandbox has no audio device so
  playback is tested structurally only. `--no-audio` runs the visualizer with a
  virtual clock for headless/no-device machines.
- `cargo build` clean; `cargo clippy` has no errors (a few cosmetic style notes remain).
- Colors/arrows verified via `--demo` ANSI-code counts: single tap=pink[213],
  break=orange[208], single touch=blue[94], simultaneous 4-touch=yellow[93] (no
  pink/orange/blue leak); slide arrows oriented correctly (horizontal 2→6=`<`,
  diagonal 1→5=`↙`, arc 1→4 curves `↙→v→↘`).
- **Slide shapes verified via `--demo` at mid-trace times** (temp `DEMO_NOW` hook,
  since removed): `1<4` sweeps ccw 225° through 8/7/6/5 (flip-rule long way, not
  the old short arc); `2V46` L-bends at button 4 (down the right side then across
  the bottom, `turn=4` carried from parser); `3v2` bends through the center;
  `6pp4` traces an inner-ring U-dip (radius 0.60·R); `1w5` renders the fan —
  `v` stem down the right (1→4) curving into `↖` sweep across 4→5→6, star at the
  junction. `--dump -d 6` confirms the `V` turning digit parses (`2V46` →
  `turn: 4, to: 6`) and `^`/`AutoCircle` parses. No `s`/`z`/`^`/`p`/`q`/`qq` in
  the test chart, so those shapes are verified by geometry/code review only.
- **Touch zones + holds verified via `--demo`** (temp `DEMO_NOW` hook, since
  removed): UPR at t=132.46 (simultaneous A2/A3/B2/B3/D3/E3) shows shutters at
  the correct per-zone positions — A outer near buttons 2/3, B inner, D outer at
  cardinal-east, E middle at cardinal-east — and the 4 concentric rings
  (outer `.` / A·D `·` / E `∘` / B `,`) render. MAST break hold `1bh[1:2]`
  (t=4, end=8): bar half-grown toward button 1 at `now=3.4` (mid-approach), full
  + flash at `now=4.0`, half-retracted at `now=6.0`, gone at `now=8.0`.
- **Touch hit white border verified via `--demo`** (temp `DEMO_NOW` hook, since
  removed): UPR simultaneous touches at t=132.46024 — approach frame `now=132.40`
  emits yellow `\x1b[93m` shutters (simultaneous) with no white; hit frame
  `now=132.46024` emits white `\x1b[97m` shutters (9 codes) with no yellow. The
  16-touch group at t=153.54 hits with 17 white codes. Single touch at t=26.7604
  hits with 3 white codes. White appears only in `[t, t+FLASH]`.
- **Fan (`w`) slides verified via `--demo`** (temp `DEMO_NOW` hook, since
  removed): REIM `-d 5` long `1w5` (t=155.952, motion [156.327, 158.684],
  from=1, dests 4/5/6) — `now=156.0` shows the fade-in (three faint full rays +
  dim `★` hub at button 1); `now=157.5` (p≈0.5) shows three bright half-rays
  from button 1 reaching midway to buttons 4/5/6 with dim remainder ahead and
  `★` heads at the tips; `now=158.7` shows the fully-open fan — three bright
  full rays 1→4/5/6 with big `✦` flashes at the destination sensors; `now=158.9`
  (past `motion_end+FLASH`) the fan is gone. The simultaneous `1w5`+`8w4` pair
  at t=155.952 renders yellow (both simultaneous); UPR break fan `3bw7`
  (t=208.504) renders orange (`\x1b[38;5;208m`) — color flows from `head_color`,
  no fan-specific wiring.

- **Background art (bg.rs) verified via `--demo` + pty runs**: image path emits
  ~530 gray SGR runs per frame (run-length compressed, not per-cell); `--bg 0`
  emits zero gray codes and skips detection (no ffmpeg probe even with bg.mp4
  present); `--bg 1` uses only codes 232/233; `--bg 10` reaches 242 (243 only
  for near-white pixels; the test jacket's brightest cell lands at index 10).
  Note-glyph positions are byte-identical with and without bg (only the HUD
  `bg N` label differs). Video path: an ffmpeg-generated fixture (`ffmpeg -loop
  1 -i bg.png -t 8 -r 24 -pix_fmt yuv420p /tmp/bg.mp4`) decodes ~594 gray runs
  into the demo frame; live pty run shows continuous bg streaming, `b` cycling
  the HUD label 5→6→7, and clean exit with no leftover ffmpeg processes.
  `PATH=/usr/bin/nonexistent` under a pty: one warning, playback continues with
  no bg. Unknown `--bg-style` values are a clap error.
- **Truecolor-cells mode verified via `--demo`**: `--bg-style cells` emits only
  `\x1b[48;2;r;g;bm` bg codes (zero gray fg codes) while `ramp` emits only gray
  fg codes (zero bg codes) — the styles never mix. Bg runs scale with level
  (bg1: 32 runs / 4 distinct colors ≈ 4.5 avg brightness → bg5: 324/19 → bg10:
  498/45 ≈ 15.5), every canvas row ends `\x1b[49m\x1b[K`, note-glyph positions
  are byte-identical with and without the backdrop, and `--bg 0` emits nothing.
  Live pty run in cells mode with the video fixture: ~30KB/frame streamed
  continuously, `b` cycles the HUD label, clean exit, no ffmpeg zombies.

## Known simplifications
- Touch zones mapped to inner ring at button angles (not exact maimai geometry).
  Now superseded: zones use distinct radii (A/D 0.80·R, E 0.60·R, B 0.45·R, C 0)
  and two angle sets (button angles for A/B, midpoint/cardinal angles for D/E),
  but the radii are terminal approximations of maimai's true sensor radii and
  D/E's π/8 offset is the spec's "22.5° CCW of the same-numbered button".
- Slide path shapes are terminal approximations of maimai's true curves:
  `>`/`<`/`^`/`-` are exact (ring arc / chord); `v`/`V` are exact polylines;
  `p`/`q`/`pp`/`qq` model the loop as an inner-ring arc at the directed distance
  (radius 0.30·R / 0.60·R) rather than the true maimai U/CUP spline; `s`/`z` are
  a 3-segment zigzag with a fixed 0.25·|chord| offset. (`w` fans now render as
  three straight expanding rays — the accurate fan shape, no longer the old
  stem+90°-sweep approximation.) Each renders its distinctive shape instead of
  collapsing to a straight chord.
- Seek/scrub not supported in v1 (mp3 decode seek is non-trivial).
- Background video is not seek-synced to the song clock (frames stream from
  playback start; after the video ends the last frame freezes as a still).
  The char-ramp backdrop is a plain luminance quantization, not dithered.
- `--bg-style cells` assumes a 24-bit-color terminal (no capability detection,
  same as the existing 256-color assumption) and quantizes to 5 bits/channel
  rather than dithering.

## Gotchas
- Inline `(BPM)` mid-measure changes step duration for subsequent slots.
- `{384}` / `{96}` long-hold measures with embedded events in later commas.
- `*`-chained and multi-point slides; free-form `[s##s]` timings — warn + approximate.
- No `&first` → chart t=0 = audio start; `--offset` shifts (live `+`/`-` nudge).
- ffmpeg bg pipe: stdin MUST be nulled (inheriting the raw-mode TTY steals
  keystrokes) and `-re` is required for real-time streaming. `read_exact` on an
  empty buffer is a no-op — refill the scratch buffer after each `mem::swap` or
  the reader spins on zero-byte reads.
- Video decode ends silently (`ended` flag); `new_frame_ready` must NOT gate on
  it, or the final frames (which arrived before EOF) never display.

## Future
Seek/scrub, judgements, chart scrubbing, record/replay, SFX, bg capability
detection, dithered/colored bg styles.