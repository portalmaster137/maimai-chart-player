//! Tokenize simai notation and resolve note timings into `NoteEvent`s.

use crate::chart::{Kind, NoteEvent, Position, SlidePart, SlideShape, StarKind, Zone};

/// Resolve a chart (raw measure lines) into a flat, time-sorted list of events.
///
/// Timing model: `seconds_per_step = 240 / (bpm * divisor)`. Each comma slot
/// advances time by one step; inline `(BPM)` / `{N}` tokens change state for
/// subsequent slots. `bpm0`/`div0` seed the state before the first measure.
pub fn resolve(measures: &[String], bpm0: f32, div0: u32, warnings: &mut Vec<String>) -> Vec<NoteEvent> {
    let mut events: Vec<NoteEvent> = Vec::new();
    let mut bpm = bpm0;
    let mut divisor = div0;
    let mut time: f32 = 0.0;

    for line in measures {
        // Split into comma slots. A trailing empty piece (from a final comma) is
        // not a step; a non-empty trailing piece (no final comma) still counts.
        let mut pieces: Vec<&str> = line.split(',').collect();
        if pieces.last().map(|p| p.is_empty()).unwrap_or(false) {
            pieces.pop();
        }

        for slot in pieces {
            // Pull leading state tokens `(BPM)` and `{N}` off the slot.
            let mut rest = slot;
            loop {
                let trimmed = rest.trim_start();
                if let Some(after) = trimmed.strip_prefix('(') {
                    if let Some(end) = after.find(')') {
                        let num = &after[..end];
                        if let Ok(b) = num.parse::<f32>() {
                            bpm = b;
                        } else {
                            warnings.push(format!("bad BPM token ({num}) in: {line}"));
                        }
                        rest = &after[end + 1..];
                        continue;
                    }
                }
                if let Some(after) = trimmed.strip_prefix('{') {
                    if let Some(end) = after.find('}') {
                        let num = &after[..end];
                        if let Ok(d) = num.parse::<u32>() {
                            if d > 0 {
                                divisor = d;
                            }
                        } else if let Some(s) = num.strip_prefix('#') {
                            // Custom measure duration in seconds — unsupported,
                            // keep divisor; warn.
                            let _ = s.parse::<f32>();
                            warnings.push(format!("{{#sec}} measure duration unsupported in: {line}"));
                        } else {
                            warnings.push(format!("bad divisor {{{num}}} in: {line}"));
                        }
                        rest = &after[end + 1..];
                        continue;
                    }
                }
                break;
            }

            let expr = rest.trim();
            if !expr.is_empty() {
                parse_slot(expr, time, bpm, divisor, &mut events, warnings);
            }

            // Advance one step using the bpm/divisor now in effect.
            if bpm > 0.0 && divisor > 0 {
                time += 240.0 / (bpm * divisor as f32);
            } else {
                warnings.push(format!("zero bpm/divisor at t={time} in: {line}"));
            }
        }
    }

    events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    events
}

/// Parse one comma slot expression (slash-separated simultaneous notes).
fn parse_slot(
    expr: &str,
    time: f32,
    bpm: f32,
    divisor: u32,
    out: &mut Vec<NoteEvent>,
    warnings: &mut Vec<String>,
) {
    for part in expr.split('/') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if !parse_note(p, time, bpm, divisor, out, warnings) {
            warnings.push(format!("unparsed note token {p:?} at t={time}"));
        }
    }
}

/// Parse a single note token. Returns true if recognized (even partially).
fn parse_note(
    token: &str,
    time: f32,
    bpm: f32,
    divisor: u32,
    out: &mut Vec<NoteEvent>,
    warnings: &mut Vec<String>,
) -> bool {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    // Touch notes start with an uppercase zone letter A-E.
    if bytes[0].is_ascii_uppercase() && (bytes[0] as char).is_ascii_alphabetic() {
        return parse_touch(token, time, bpm, divisor, out, warnings);
    }

    // `0` is the EACH-forcing no-note marker.
    if token == "0" {
        return true;
    }

    // Ring notes start with a digit 1-8. Handle adjacent bare taps (e.g. "12").
    let mut idx = 0;
    let mut emitted = false;
    while idx < bytes.len() {
        let c = bytes[idx];
        if !(b'1'..=b'8').contains(&c) {
            break;
        }
        let btn = c - b'0';
        // Look ahead: modifiers, hold, or slide?
        let consumed = parse_ring_note(&token[idx..], btn, time, bpm, divisor, out, warnings);
        if consumed == 0 {
            return emitted;
        }
        idx += consumed;
        emitted = true;
    }
    emitted
}

/// Parse one ring note starting at a known button. Returns bytes consumed.
fn parse_ring_note(
    s: &str,
    btn: u8,
    time: f32,
    bpm: f32,
    divisor: u32,
    out: &mut Vec<NoteEvent>,
    warnings: &mut Vec<String>,
) -> usize {
    let b = s.as_bytes();
    // s[0] is the button digit itself; scan modifiers/hold/slide after it.
    let mut i = 1;
    let mut brk = false;
    let mut ex = false;
    let mut star = StarKind::None;
    let mut tapless = false;

    // Tap modifiers (b, x, $, $$, ?, !, @) in any order.
    while i < b.len() {
        match b[i] {
            b'b' => brk = true,
            b'x' => ex = true,
            b'$' => {
                star = if star == StarKind::None {
                    StarKind::Star
                } else {
                    StarKind::Spin
                };
            }
            b'?' | b'!' => tapless = true,
            b'@' => star = StarKind::None, // regular tap, not star
            _ => break,
        }
        i += 1;
    }

    // Hold?
    if i < b.len() && b[i] == b'h' {
        i += 1;
        let (dur, consumed) = parse_bracket(&s[i..], bpm, divisor, warnings);
        i += consumed;
        let end = time + dur.max(0.0);
        out.push(NoteEvent {
            time,
            pos: Position::Button(btn),
            kind: Kind::Hold { end, brk, ex },
            firework: false,
        });
        return i;
    }

    // Slide? A shape char begins a slide.
    if i < b.len() && is_shape_start(b[i]) {
        let star_kind = if tapless { StarKind::None } else if star == StarKind::None { StarKind::Star } else { star };
        let (parts, last_end, consumed) =
            parse_slide_chain(&s[i..], btn, time, bpm, divisor, warnings);
        out.push(NoteEvent {
            time,
            pos: Position::Button(btn),
            kind: Kind::Slide { star: star_kind, brk, ex, parts, last_end },
            firework: false,
        });
        return i + consumed;
    }

    // Plain tap (possibly adjacent to more taps).
    if !tapless {
        out.push(NoteEvent {
            time,
            pos: Position::Button(btn),
            kind: Kind::Tap { brk, ex, star },
            firework: false,
        });
    }
    i
}

fn is_shape_start(c: u8) -> bool {
    matches!(c, b'-' | b'>' | b'<' | b'p' | b'q' | b'w' | b'v' | b'V' | b'z' | b's')
}

/// Parse a slide chain: legs separated by `*`, each group ends with `[dur]`.
/// Returns the parts, the absolute last-end time, and bytes consumed.
fn parse_slide_chain(
    s: &str,
    start_btn: u8,
    time: f32,
    bpm: f32,
    divisor: u32,
    warnings: &mut Vec<String>,
) -> (Vec<SlidePart>, f32, usize) {
    let b = s.as_bytes();
    let mut i = 0;
    let mut from = start_btn;
    let mut parts: Vec<SlidePart> = Vec::new();

    // Motion begins one quarter note after the star tap.
    let mut cursor = time + if bpm > 0.0 { 60.0 / bpm } else { 0.5 };

    loop {
        // Parse one group: one or more legs (shape + dst) then a bracket.
        let mut legs: Vec<(SlideShape, u8)> = Vec::new();
        loop {
            if i >= b.len() {
                warnings.push(format!("slide ended before bracket: {s}"));
                break;
            }
            let shape = match b[i] {
                b'-' => SlideShape::Line,
                b'>' => SlideShape::ArcRight,
                b'<' => SlideShape::ArcLeft,
                b'p' => {
                    if b.get(i + 1) == Some(&b'p') {
                        i += 1;
                        SlideShape::PP
                    } else {
                        SlideShape::P
                    }
                }
                b'q' => {
                    if b.get(i + 1) == Some(&b'q') {
                        i += 1;
                        SlideShape::QQ
                    } else {
                        SlideShape::Q
                    }
                }
                b'w' => SlideShape::W,
                b'v' => {
                    if b.get(i + 1) == Some(&b'V') {
                        i += 1;
                        SlideShape::VBig
                    } else {
                        SlideShape::V
                    }
                }
                b'V' => SlideShape::VBig,
                b'z' => SlideShape::Z,
                b's' => SlideShape::S,
                _ => break,
            };
            i += 1;
            // `V` (big) shape carries a turning-point digit before the dest
            // (e.g. `1V36` = V via turning point 3 to dest 6). Consume & ignore it.
            if shape == SlideShape::VBig && i < b.len() && (b'1'..=b'8').contains(&b[i]) {
                i += 1;
            }
            // Destination button digit.
            if i < b.len() && (b'1'..=b'8').contains(&b[i]) {
                let dst = b[i] - b'0';
                i += 1;
                legs.push((shape, dst));
            } else {
                warnings.push(format!("slide missing dst in: {s}"));
                break;
            }
            // Optional break-slide marker `b` after the destination, before bracket.
            if i < b.len() && b[i] == b'b' {
                i += 1;
            }
            // If next is a bracket or `*` or end, stop collecting legs.
            if i >= b.len() || b[i] == b'[' || b[i] == b'*' {
                break;
            }
        }

        // Bracket duration for this group.
        let mut dur = 0.5_f32;
        if i < b.len() && b[i] == b'[' {
            let (d, consumed) = parse_bracket(&s[i..], bpm, divisor, warnings);
            dur = d.max(0.0);
            i += consumed;
            // Optional trailing `b` (break slide) — ignore for rendering.
            if i < b.len() && b[i] == b'b' {
                i += 1;
            }
        } else {
            warnings.push(format!("slide group without bracket in: {s}"));
        }

        // Distribute duration evenly across the group's legs.
        let n = legs.len().max(1);
        let leg_dur = dur / n as f32;
        for (shape, dst) in legs {
            let motion_start = cursor;
            let motion_end = cursor + leg_dur;
            parts.push(SlidePart { shape, from, to: dst, motion_start, motion_end });
            cursor = motion_end;
            from = dst;
        }

        // `*` chains another group; continue. Otherwise done.
        if i < b.len() && b[i] == b'*' {
            i += 1;
            continue;
        }
        break;
    }

    (parts, cursor, i)
}

/// Parse a `[...]` duration bracket. Returns (seconds, bytes consumed).
fn parse_bracket(s: &str, bpm: f32, _divisor: u32, warnings: &mut Vec<String>) -> (f32, usize) {
    let b = s.as_bytes();
    if b.is_empty() || b[0] != b'[' {
        return (0.5, 0); // default fallback duration
    }
    let Some(end) = b.iter().position(|&c| c == b']') else {
        warnings.push(format!("unterminated bracket {s:?}"));
        return (0.5, 0);
    };
    let inner = &s[1..end];
    let consumed = end + 1;

    // `[BPM#A:B]` — custom BPM for the slide/hold.
    if let Some((bpms, rest)) = inner.split_once('#') {
        if let Ok(custom_bpm) = bpms.parse::<f32>() {
            // rest is "A:B" or a single number (absolute seconds).
            if let Some((a, bb)) = rest.split_once(':') {
                if let (Ok(a), Ok(bb)) = (a.parse::<f32>(), bb.parse::<f32>()) {
                    let d = if custom_bpm > 0.0 && a > 0.0 {
                        bb * 240.0 / (custom_bpm * a)
                    } else {
                        0.5
                    };
                    return (d, consumed);
                }
            }
            if let Ok(sec) = rest.parse::<f32>() {
                return (sec, consumed);
            }
        }
        // `[delay##slide]` free-form (both seconds).
        if let Some((d, sl)) = inner.split_once("##") {
            let delay = d.parse::<f32>().unwrap_or(0.0);
            let slide = sl.parse::<f32>().unwrap_or(0.5);
            return (delay + slide, consumed);
        }
        warnings.push(format!("exotic bracket [{inner}] approximated"));
        return (0.5, consumed);
    }

    // `[#s]` — absolute seconds.
    if let Some(sec) = inner.strip_prefix('#') {
        if let Ok(s) = sec.parse::<f32>() {
            return (s, consumed);
        }
    }

    // `[A:B]` — divisor:length.
    if let Some((a, bb)) = inner.split_once(':') {
        if let (Ok(a), Ok(bb)) = (a.parse::<f32>(), bb.parse::<f32>()) {
            if a > 0.0 && bpm > 0.0 {
                return (bb * 240.0 / (bpm * a), consumed);
            }
        }
    }

    warnings.push(format!("unparsed bracket [{inner}], defaulting"));
    (0.5, consumed)
}

/// Parse a touch note token (zone A-E).
fn parse_touch(
    token: &str,
    time: f32,
    bpm: f32,
    divisor: u32,
    out: &mut Vec<NoteEvent>,
    warnings: &mut Vec<String>,
) -> bool {
    let b = token.as_bytes();
    let zone = match b[0] {
        b'A' => Zone::A,
        b'B' => Zone::B,
        b'C' => Zone::C,
        b'D' => Zone::D,
        b'E' => Zone::E,
        _ => return false,
    };
    let mut i = 1;
    // Optional digit (1-8 for A/B/D/E; C may be `C` or `C1`).
    let mut idx: u8 = 0;
    if i < b.len() && (b'1'..=b'8').contains(&b[i]) {
        idx = b[i] - b'0';
        i += 1;
    } else if !matches!(zone, Zone::C) {
        // A/B/D/E require an index; tolerate absence.
        warnings.push(format!("touch {token} missing index"));
    }

    // Optional firework `f`.
    let mut firework = false;
    if i < b.len() && b[i] == b'f' {
        firework = true;
        i += 1;
    }

    // Optional touch hold (primarily C): `h[...]`.
    if i < b.len() && b[i] == b'h' {
        i += 1;
        let (dur, _) = parse_bracket(&token[i..], bpm, divisor, warnings);
        let end = time + dur.max(0.0);
        out.push(NoteEvent {
            time,
            pos: Position::Touch(zone, idx),
            kind: Kind::TouchHold { end, firework },
            firework,
        });
        return true;
    }

    // Touch tap.
    out.push(NoteEvent {
        time,
        pos: Position::Touch(zone, idx),
        kind: Kind::Tap { brk: false, ex: false, star: StarKind::None },
        firework,
    });
    let _ = i;
    true
}