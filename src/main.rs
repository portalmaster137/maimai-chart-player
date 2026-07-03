//! MaiMai terminal chart player entry point.

mod chart;
mod cli;
mod maidata;
mod player;
mod render;
mod simai;

use clap::Parser;
use cli::{choose_difficulty, Cli};
use maidata::load;
use player::{run, PlayerConfig};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let maidata_path = cli.dir.join("maidata.txt");
    let track_path = cli.dir.join("track.mp3");

    let md = load(&maidata_path).map_err(|e| format!("reading maidata.txt: {e}"))?;
    if !track_path.exists() {
        return Err(format!("track.mp3 not found in {}", cli.dir.display()).into());
    }

    let diff = match choose_difficulty(&md, cli.difficulty) {
        Some(d) => d,
        None => return Ok(()),
    };
    let diff_name = chart::DIFFICULTY_NAMES[diff as usize].to_string();

    let measures = md
        .chart(diff)
        .ok_or_else(|| format!("difficulty {diff} has no chart"))?;

    let mut warnings: Vec<String> = Vec::new();
    let bpm0 = md.whole_bpm().unwrap_or(120.0);
    let events = simai::resolve(measures, bpm0, 4, &mut warnings);

    if !warnings.is_empty() {
        eprintln!("parser warnings ({}):", warnings.len());
        for w in warnings.iter().take(40) {
            eprintln!("  - {w}");
        }
        if warnings.len() > 40 {
            eprintln!("  ... ({} more)", warnings.len() - 40);
        }
    }
    eprintln!("parsed {} events for difficulty {diff} ({diff_name}).", events.len());

    if let Some(n) = cli.dump {
        for ev in events.iter().take(n) {
            eprintln!("  t={:>8.3}  {:?}", ev.time, ev);
        }
        return Ok(());
    }

    if cli.demo {
        return render_demo(&events);
    }
    eprintln!("press q/Esc to quit. starting in 1s...");
    std::thread::sleep(std::time::Duration::from_secs(1));

    run(
        &md,
        &events,
        &track_path,
        PlayerConfig {
            offset: cli.offset,
            speed: cli.speed,
            difficulty_name: diff_name,
            no_audio: cli.no_audio,
        },
    )?;
    Ok(())
}

/// Render a single demo frame to stdout without audio or raw-mode terminal.
/// Picks events near a chosen `now` so the circle shows approaching + hit notes.
fn render_demo(events: &[chart::NoteEvent]) -> Result<(), Box<dyn std::error::Error>> {
    use chart::{Kind, Position};
    use render::{Hud, Renderer};
    use std::time::Instant;

    let renderer = Renderer::new(74, 26);
    // Find a `now` where there's a good mix of activity (first slide + taps).
    let now = 4.6; // during the MAST intro: holds + E-touches + approaching taps
    let _ = Position::Button(1);
    let _ = Kind::Tap { brk: false, ex: false, star: chart::StarKind::None };

    let hud = Hud {
        title: "DEMO".into(),
        difficulty: "MAST".into(),
        bpm: 120.0,
        time: now,
        offset: 0.0,
        speed: 5,
        paused: false,
        muted: false,
    };
    let frame = renderer.frame(events, now, 5, &hud);
    print!("{frame}");
    let _ = Instant::now();
    Ok(())
}