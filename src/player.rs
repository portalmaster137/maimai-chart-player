//! Audio playback, master clock, sync/render loop, and key handling.

use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use rodio::{Decoder, OutputStream, Sink};

use crate::bg::Bg;
use crate::chart::NoteEvent;
use crate::maidata::Maidata;
use crate::render::{Hud, Renderer};

pub struct PlayerConfig {
    pub offset: f32,
    pub speed: u8,
    pub difficulty_name: String,
    pub no_audio: bool,
}

/// Run the player. Returns Ok(()) on clean exit.
pub fn run(
    md: &Maidata,
    events: &[NoteEvent],
    track: &Path,
    bg: &mut Bg,
    cfg: PlayerConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let (term_w, term_h) = terminal::size().unwrap_or((80, 24));
    // The visualizer needs a real interactive terminal.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin())
        || !std::io::IsTerminal::is_terminal(&std::io::stdout())
    {
        return Err("maimai-player must be run in an interactive terminal (TTY).".into());
    }

    // --- Terminal setup ---
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;

    // Guard to restore the terminal no matter how we exit.
    struct TermGuard;
    impl Drop for TermGuard {
        fn drop(&mut self) {
            let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
    let _guard = TermGuard;

    // --- Audio (optional) ---
    let volume: f32 = 0.8;
    let mut audio_sink: Option<Sink> = None;
    // The output stream must outlive the sink, so keep it bound here.
    let _stream: Option<OutputStream> = if cfg.no_audio {
        None
    } else {
        match open_audio(track, volume) {
            Ok((stream, sink)) => {
                audio_sink = Some(sink);
                Some(stream)
            }
            Err(e) => {
                eprintln!("warning: audio unavailable ({e}); continuing without sound. Use --no-audio to silence this.");
                None
            }
        }
    };

    // Canvas fills the terminal (minus margins for the HUD and a right-edge
    // safety column so the last printed char never auto-wraps the line). The
    // circle itself is centered within this canvas and autoscaled to fit both
    // dimensions at ~2:1 aspect inside the renderer.
    let (cw, ch) = canvas_dims(term_w, term_h);
    let mut renderer = Renderer::new(cw, ch);
    let mut last_size = (term_w, term_h);
    let mut offset = cfg.offset;
    let mut speed = cfg.speed.clamp(1, 10);
    let mut paused = false;
    let mut muted = false;
    let diff_name = cfg.difficulty_name;

    let start = Instant::now();
    let title = md.title();
    let bpm = md.whole_bpm().unwrap_or(0.0);

    // Pausing freezes both audio and the chart clock: we accumulate time spent
    // paused and subtract it from the elapsed master clock.
    let mut pause_accum: Duration = Duration::ZERO;
    let mut pause_start: Option<Instant> = None;

    let frame_dur = Duration::from_millis(16);
    let mut running = true;

    while running {
        let mut elapsed = start.elapsed();
        if let Some(ps) = pause_start {
            elapsed = elapsed.saturating_sub(ps.elapsed());
        }
        elapsed = elapsed.saturating_sub(pause_accum);
        let now = elapsed.as_secs_f32() - offset;

        // Adapt to terminal resize: if the size changed since the last frame,
        // rebuild the renderer at the new dimensions.
        let (tw, th) = terminal::size().unwrap_or(last_size);
        if (tw, th) != last_size {
            let (cw, ch) = canvas_dims(tw, th);
            renderer = Renderer::new(cw, ch);
            last_size = (tw, th);
        }
        let (cw, ch) = canvas_dims(tw, th);
        // Push a fresh backdrop into the renderer when one is due (level
        // change, resize, or a new video frame).
        if let Some(layer) = bg.layer(cw, ch) {
            renderer.set_bg_layer(layer);
        }

        let hud = Hud {
            title: title.clone(),
            difficulty: diff_name.clone(),
            bpm,
            time: now,
            offset,
            speed,
            paused,
            muted,
            bg: bg.level(),
        };

        let frame = renderer.frame(events, now, speed, &hud);
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()?;

        // Key handling (non-blocking).
        while event::poll(Duration::ZERO)? {
            let ev = event::read()?;
            if let Event::Key(k) = ev {
                if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
                    continue;
                }
                match (k.code, k.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => running = false,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => running = false,
                    (KeyCode::Char('+') | KeyCode::Char('='), _) => offset += 0.05,
                    (KeyCode::Char('-'), _) => offset -= 0.05,
                    (KeyCode::Up, _) => speed = (speed + 1).min(10),
                    (KeyCode::Down, _) => speed = (speed.saturating_sub(1)).max(1),
                    (KeyCode::Char('p'), _) => {
                        paused = !paused;
                        if paused {
                            pause_start = Some(Instant::now());
                            if let Some(s) = &audio_sink { s.pause(); }
                        } else {
                            if let Some(ps) = pause_start.take() {
                                pause_accum += ps.elapsed();
                            }
                            if let Some(s) = &audio_sink { s.play(); }
                        }
                    }
                    (KeyCode::Char('m'), _) => {
                        muted = !muted;
                        if let Some(s) = &audio_sink {
                            s.set_volume(if muted { 0.0 } else { volume });
                        }
                    }
                    // Cycle the background dim level 0→1→…→10→0 (0 = off).
                    (KeyCode::Char('b'), _) => bg.set_level((bg.level() + 1) % 11),
                    _ => {}
                }
            }
        }

        std::thread::sleep(frame_dur);
    }

    if let Some(s) = &audio_sink {
        s.stop();
    }
    Ok(())
}

/// Open the audio output stream and start playback of `track`.
fn open_audio(track: &Path, volume: f32) -> Result<(OutputStream, Sink), String> {
    let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
    let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
    let file = File::open(track).map_err(|e| e.to_string())?;
    let source = Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    sink.set_volume(volume);
    sink.append(source);
    Ok((stream, sink))
}

/// Compute the canvas (cols × rows) from the terminal size. The canvas fills
/// the terminal: full width minus one safety column (so the last char doesn't
/// auto-wrap), full height minus two rows reserved for the HUD. Both clamped to
/// sane minimums so a tiny terminal still renders a small but valid circle.
fn canvas_dims(term_w: u16, term_h: u16) -> (u16, u16) {
    let width = term_w.saturating_sub(1).max(20);
    let height = term_h.saturating_sub(2).max(12);
    (width, height)
}