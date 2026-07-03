//! Data model for resolved maimai chart events.

/// Touch sensor zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    A,
    B,
    C,
    D,
    E,
}

/// Where a note lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// Ring button 1..8 (1 = top-right, clockwise).
    Button(u8),
    /// Touch sensor: zone + index (index 0 for center `C`).
    Touch(Zone, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarKind {
    None,
    Star,
    Spin,
}

/// Slide shape, mirroring simai notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideShape {
    Line,       // `-`
    ArcRight,   // `>`
    ArcLeft,    // `<`
    AutoCircle, // `^` (shortest-direction ring arc)
    P,          // `p`
    Q,          // `q`
    PP,         // `pp`
    QQ,         // `qq`
    V,          // `v`
    VBig,       // `V` (L-shape via a turning-point button)
    Z,          // `z`
    S,          // `s`
    W,          // `w` (fan / WiFi)
}

/// One leg of a (possibly chained) slide.
#[derive(Debug, Clone, Copy)]
pub struct SlidePart {
    pub shape: SlideShape,
    pub from: u8,
    pub to: u8,
    /// Turning-point button for the `V` (L-shape) slide; 0 = none.
    pub turn: u8,
    /// Seconds after the star tap when this leg's motion begins.
    pub motion_start: f32,
    /// Seconds after the star tap when this leg's motion ends.
    pub motion_end: f32,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Tap {
        brk: bool,
        ex: bool,
        star: StarKind,
    },
    Hold {
        end: f32,
        brk: bool,
        ex: bool,
    },
    Slide {
        star: StarKind,
        brk: bool,
        ex: bool,
        parts: Vec<SlidePart>,
        /// Latest motion end (absolute seconds); kept for future culling.
        #[allow(dead_code)]
        last_end: f32,
    },
    TouchHold {
        end: f32,
        firework: bool,
    },
}

#[derive(Debug, Clone)]
pub struct NoteEvent {
    /// Absolute hit time in seconds (chart timeline; t=0 = audio start + offset).
    pub time: f32,
    pub pos: Position,
    pub kind: Kind,
    pub firework: bool,
}

/// Difficulty slot metadata.
#[derive(Debug, Clone, Default)]
pub struct DifficultyInfo {
    pub index: u8,      // 1..6
    pub name: &'static str,
    pub level: String,  // e.g. "13.7"
    pub charter: String,
    pub present: bool,  // has an &inote_N block
}

pub const DIFFICULTY_NAMES: [&str; 7] = ["", "EZ", "STD", "HRD", "MAST", "REIM", "UPR"];