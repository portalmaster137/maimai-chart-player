//! Parse the maidata.txt container: `&key=value` header and `&inote_N` blocks.

use std::collections::HashMap;
use std::path::Path;

use crate::chart::{DifficultyInfo, DIFFICULTY_NAMES};

/// Parsed maidata.txt: metadata + per-difficulty chart measure lines.
#[derive(Debug, Default)]
pub struct Maidata {
    pub meta: HashMap<String, String>,
    /// Raw measure lines for each difficulty 1..=6 that is present.
    pub charts: HashMap<u8, Vec<String>>,
}

impl Maidata {
    pub fn title(&self) -> String {
        self.meta.get("title").cloned().unwrap_or_default()
    }

    pub fn whole_bpm(&self) -> Option<f32> {
        self.meta.get("wholebpm").and_then(|v| v.parse::<f32>().ok())
    }

    /// Difficulty info for all six slots, in order, marking which are present.
    pub fn difficulties(&self) -> Vec<DifficultyInfo> {
        (1..=6)
            .map(|i| {
                let present = self.charts.contains_key(&i);
                DifficultyInfo {
                    index: i,
                    name: DIFFICULTY_NAMES[i as usize],
                    level: self.meta.get(&format!("lv_{i}")).cloned().unwrap_or_default(),
                    charter: self
                        .meta.get(&format!("des_{i}"))
                        .cloned()
                        .unwrap_or_default(),
                    present,
                }
            })
            .collect()
    }

    pub fn chart(&self, difficulty: u8) -> Option<&Vec<String>> {
        self.charts.get(&difficulty)
    }
}

/// Read and parse a maidata.txt file.
pub fn load(path: &Path) -> std::io::Result<Maidata> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse(&text))
}

/// Parse maidata text. Lines use `&key=value`; chart bodies (`&inote_N=...`)
/// span multiple lines until a line containing `E`.
pub fn parse(text: &str) -> Maidata {
    let mut md = Maidata::default();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A key line begins with `&`. The value may be empty (chart body follows).
        let Some(rest) = trimmed.strip_prefix('&') else {
            continue;
        };
        let (key, value) = match rest.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => continue,
        };

        if let Some(diff_str) = key.strip_prefix("inote_") {
            if let Ok(idx) = diff_str.parse::<u8>() {
                // Collect measure lines until a line that is just `E` (ignoring
                // whitespace). The value after `=` on the key line is usually empty.
                let mut measures: Vec<String> = Vec::new();
                if !value.is_empty() {
                    measures.push(value.to_string());
                }
                while let Some(next) = lines.peek() {
                    let n = next.trim();
                    if n == "E" || n == "E," || n.eq_ignore_ascii_case("e") {
                        lines.next();
                        break;
                    }
                    if n.starts_with('&') {
                        // Next key begins without an explicit `E`; stop here.
                        break;
                    }
                    let l = lines.next().unwrap();
                    if !l.trim().is_empty() {
                        measures.push(l.to_string());
                    }
                }
                md.charts.insert(idx, measures);
            }
            continue;
        }

        md.meta.insert(key, value);
    }

    md
}