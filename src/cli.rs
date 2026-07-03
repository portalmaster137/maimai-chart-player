//! CLI parsing and interactive difficulty selection.

use std::io::{self, Write};
use std::path::PathBuf;

use clap::Parser;
use crate::maidata::Maidata;
use crate::chart::DifficultyInfo;

#[derive(Parser, Debug)]
#[command(name = "maimai-player", version, about = "Terminal maimai chart player (simai + mp3).")]
pub struct Cli {
    /// Directory containing maidata.txt and track.mp3.
    pub dir: PathBuf,

    /// Difficulty index 1..=6 (EZ/STD/HRD/MAST/REIM/UPR). Omit for interactive menu.
    #[arg(short, long)]
    pub difficulty: Option<u8>,

    /// Chart-to-audio offset in seconds (chart t=0 = audio start + offset).
    #[arg(long, default_value_t = 0.0)]
    pub offset: f32,

    /// Note travel speed factor 1..=10 (higher = faster/shorter approach).
    #[arg(long, default_value_t = 5)]
    pub speed: u8,

    /// Dump the first N resolved events to stderr and exit (no playback).
    #[arg(long, num_args = 0..=1, default_missing_value = "20")]
    pub dump: Option<usize>,

    /// Render a single synthetic demo frame to stdout (no audio/terminal) and exit.
    #[arg(long)]
    pub demo: bool,

    /// Run the visualizer without audio output (virtual clock). For headless/no-device use.
    #[arg(long)]
    pub no_audio: bool,
}

/// Decide which difficulty to play, prompting interactively if needed.
/// Returns the chosen index, or None if the user cancelled.
pub fn choose_difficulty(md: &Maidata, cli_diff: Option<u8>) -> Option<u8> {
    let diffs: Vec<DifficultyInfo> = md.difficulties().into_iter().filter(|d| d.present).collect();

    if let Some(d) = cli_diff {
        if md.chart(d).is_some() {
            return Some(d);
        }
        eprintln!("Difficulty {d} not present. Available:");
        for d in &diffs {
            eprintln!("  {}: {} (lv {})", d.index, d.name, d.level);
        }
        // Fall through to interactive.
    }

    if diffs.is_empty() {
        eprintln!("No charts found in maidata.txt.");
        return None;
    }
    if diffs.len() == 1 {
        return Some(diffs[0].index);
    }

    // Interactive menu.
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "\nAvailable difficulties:");
    for d in &diffs {
        let charter = if d.charter.is_empty() { String::new() } else { format!(" — {}", d.charter) };
        let _ = writeln!(out, "  [{}] {}  lv {}{}", d.index, d.name, d.level, charter);
    }
    let _ = write!(out, "Pick a number: ");
    let _ = out.flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok() {
        if let Ok(n) = input.trim().parse::<u8>() {
            if md.chart(n).is_some() {
                return Some(n);
            }
        }
        eprintln!("Invalid selection.");
        return None;
    }
    None
}