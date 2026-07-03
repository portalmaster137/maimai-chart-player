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
  - Slide: `<src><shape><dst>[A:B]`, shapes `- > < p q w v V z s` (+`pp`/`qq`),
    `*` chains, multi-point `a-b-c`. Star tap at `time`; **motion starts one
    quarter note later** = `time + 60/bpm`; lasts bracket duration.
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
  Touch zones on a smaller inner ring at the same 8 angles (simplification). C at center.
- Outward travel: window `W = BASE/speed` (BASE tuned so speed 5 ≈ 1.2s).
  progress `p=(now-(t-W))/W`; pos = lerp(inner_for_button, button_pos, p).
  Flash at p≈1 for FLASH secs, then fade.
- **Notes are filled circle discs** (`put_disc`, 2:1 aspect), sized up:
  tap/ex `●` r=1.2, break `●` r=1.5, star `★` r=1.3, touch `●` r=1.0, flash `◉` r=1.7.
  Dim trail disc behind while approaching.
- **Holds extend as a retracting bar.** During `[time, end]` a bar of filled discs
  runs from the button inward toward the (near-center) spawn point. Its length =
  remaining hold fraction (`frac = 1 - p`, `p = (now-time)/(end-time)`): full at
  the hit, retracts to a stub at the button as the hold elapses, gone at `end`.
  Thickness ≈ 0.7× the note radius (break bars are thicker/orange). The button cell
  itself is left to the head disc (or the flash glyph during the brief hit flash),
  so the bar reads as a tail extending the note, not a separate marker.
- **Color scheme** (`note_color`, priority order):
  1. break → **orange** (256-color `\x1b[38;5;208m`) — always, even if simultaneous.
  2. simultaneous (≥2 notes at the same `time`) → **yellow** (`\x1b[93m`).
  3. star / touch → **blue** (`\x1b[94m`).
  4. EX → **magenta**.
  5. default tap → **pink** (256-color `\x1b[38;5;213m`).
  Firework touches are yellow. `simultaneous_flags` groups sorted events by equal
  `time` and marks groups of ≥2.
- **Star slides fade in then trace:** each leg samples a connected cell path
  (`sample_path` → (col,row,dir_c,dir_r)). The path is drawn as **little arrows**
  (`arrow_for`) oriented along the continuous travel direction (8 octants, aspect-
  corrected): `> < ^ v ↘↙↖↗`. During `[motion_start-FADE_IN, motion_start]` the
  full path fades in as dim arrows (gray→dim-blue). During `[motion_start,
  motion_end]` the star head (`★` disc, blue/orange/yellow) travels start→end;
  arrows behind it light up bright (blue, or orange if break) and stay lit,
  arrows ahead stay dim.
- **Touch notes are shutters, not center travelers.** A touch appears at its own
  zone position as a small square shutter: four pyramids (▲▼◀▶) around the hit
  point that close in as `p→1` (open at the square's edges, tips meet at hit),
  then briefly hold closed and go away. The appear→close window is `BASE/speed`,
  so faster speed = shorter shutter (speed 1 ≈ 6s, speed 5 ≈ 1.2s, speed 10 ≈
  0.6s). Plain touches are blue (or yellow if simultaneous/firework). Touch holds
  close the shutter at hit, then sustain an `h` disc through `[t, end]`.
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
`maimai-player <DIR> [-d <N>] [--offset <secs>] [--speed <1-10>]`
- No `-d` → interactive numbered menu (level + charter). Defaults: speed 5, offset 0.
- Keys: `q`/Esc quit, `+`/`-` offset nudge, `↑`/`↓` speed, `p` pause, `m` mute.

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
- [x] render: slide moving star + path (straight chord + ring arc for >/<)
- [x] render: touch zones (inner ring, green + / yellow F firework)
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
- [x] touch holds: rainbow body at the touch point (center C usually) + a small
      ring sweeping clockwise from top as a hold-duration indicator (24-step
      256-color rainbow wheel)

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

## Known simplifications
- Touch zones mapped to inner ring at button angles (not exact maimai geometry).
- Slide path shapes approximated (straight line + ring-arc for `>`/`<`); full
  p/q/w/v/V/z geometry approximated as straight or arc.
- Seek/scrub not supported in v1 (mp3 decode seek is non-trivial).

## Gotchas
- Inline `(BPM)` mid-measure changes step duration for subsequent slots.
- `{384}` / `{96}` long-hold measures with embedded events in later commas.
- `*`-chained and multi-point slides; free-form `[s##s]` timings — warn + approximate.
- No `&first` → chart t=0 = audio start; `--offset` shifts (live `+`/`-` nudge).

## Future
Seek/scrub, judgements, chart scrubbing, record/replay, SFX, jacket image (bg.png).