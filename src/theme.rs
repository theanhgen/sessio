//! The one place sessio picks a colour. Every other module styles text through the roles here,
//! never through a colour of its own — `tests::no_colour_outside_the_theme` holds them to it.
//!
//! The roles, what each means and what the website calls it are in `docs/DESIGN.md`. The short
//! version: the terminal's own background and foreground are the surface and the body text, and
//! are never painted over; everything coloured is carrying a state, not decorating. Colour is
//! never the only carrier of a state either — each one also has a glyph or a word.

use ratatui::style::{Color, Modifier, Style};

/// Body text: the terminal's own foreground, whatever the user's theme makes it.
pub const TEXT: Color = Color::Reset;
/// Secondary text: hints, metadata, rules, the unselected rows.
pub const DIM: Color = Color::DarkGray;
/// The thing you are looking at: session titles, key names in help, a hint that is switched on.
pub const ACCENT: Color = Color::Cyan;
/// Inline and fenced code in rendered markdown.
pub const CODE: Color = Color::Cyan;
/// Your move: a session waiting on you, an unfinished one, a content-search hit, a warning.
pub const ATTENTION: Color = Color::Yellow;
/// A `claude` process is attached, or the transcript was written in the last five minutes.
pub const RUNNING: Color = Color::Green;
/// Written in the last 24 hours.
///
/// This and the next two are fixed xterm-256 colours, which — unlike the named ANSI ones above —
/// a terminal theme does not remap. So they are picked to hold up on a white background as well as
/// a black one (`fixed_colours_survive_a_light_terminal`); the brighter 208 / 141 / 75 they replace
/// were 2.3–2.7:1 on white.
pub const RECENT: Color = Color::Indexed(202);
/// Claude's own words in the preview: the recap and the last reply.
pub const VOICE: Color = Color::Indexed(98);
/// The tag on sessions another agent wrote (`copilot`).
pub const AGENT_TAG: Color = Color::Indexed(68);
/// Something you asked for happened.
pub const SUCCESS: Color = Color::Green;
/// Something you asked for did not happen.
pub const ERROR: Color = Color::Red;

/// Selection backgrounds. Reversing the terminal's own colours made every selection the same
/// slab of black, so the panel and the tab strip could not be told apart at a glance — and on
/// a light theme it was the heaviest thing on screen.
///
/// One hue, and value does the hierarchy: the session you are reading is plum, the project
/// that contains it is neutral. Two competing colours read as two things of equal weight,
/// which is not what they are — the panel is context, the tab is focus.
pub const PANEL_SEL: Color = Color::Indexed(238);
pub const TAB_SEL: Color = Color::Indexed(54);
/// Text on either of them, bright enough to read on both.
pub const ON_SEL: Color = Color::Indexed(255);

fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

pub fn text() -> Style {
    Style::default()
}
pub fn dim() -> Style {
    fg(DIM)
}
pub fn accent() -> Style {
    fg(ACCENT)
}
pub fn code() -> Style {
    fg(CODE)
}
pub fn attention() -> Style {
    fg(ATTENTION)
}
pub fn running() -> Style {
    fg(RUNNING)
}
pub fn recent() -> Style {
    fg(RECENT)
}
pub fn voice() -> Style {
    fg(VOICE)
}
pub fn agent_tag() -> Style {
    fg(AGENT_TAG)
}

/// The highlight for the project the panel is sitting on.
pub fn panel_selected() -> Style {
    Style::default().bg(PANEL_SEL).fg(ON_SEL)
}

/// The highlight for the session tab in focus. A different hue from the panel's on purpose: two
/// selections are on screen at once and they answer different questions. Bold as well, so the
/// focused tab still stands out where the background cannot be seen.
pub fn tab_selected() -> Style {
    Style::default().bg(TAB_SEL).fg(ON_SEL).add_modifier(Modifier::BOLD)
}

/// `style` sitting on a selection: its own foreground kept, the selection's background and
/// weight taken. A status glyph on the focused tab must stay the colour that says what it is.
pub fn on_selection(style: Style, sel: Style) -> Style {
    style.bg(sel.bg.unwrap_or(TAB_SEL)).add_modifier(Modifier::BOLD)
}

/// What kind of thing a flash is saying. The colour tells you at a glance, the mark without it:
/// an error always leads with `✗`, and success messages carry their own `↗` / `↩` / `✓`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// Neutral acknowledgement or progress: "searching…", "reply discarded".
    #[default]
    Info,
    /// It worked.
    Success,
    /// It did not happen, and here is why or what to press next — nothing failed.
    Warning,
    /// It was tried and failed.
    Error,
    /// Started and not finished yet: a reply on its way, a process being ended. Said once, and
    /// the thing it is about carries its own marker until the result replaces it.
    Pending,
}

impl Tone {
    pub fn style(self) -> Style {
        match self {
            Tone::Info => text(),
            Tone::Success => fg(SUCCESS),
            Tone::Warning => attention(),
            Tone::Error => fg(ERROR),
            Tone::Pending => text(),
        }
    }

    /// The glyph the renderer puts in front of the message, so a failure never depends on red.
    pub fn mark(self) -> &'static str {
        match self {
            Tone::Error => "✗ ",
            Tone::Pending => "⏳ ",
            _ => "",
        }
    }
}

/// A role colour as the xterm-256 palette draws it, for the website demo and the contrast tests.
/// `None` for the terminal's own default, which only the terminal knows.
pub fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0), (128, 0, 0), (0, 128, 0), (128, 128, 0),
        (0, 0, 128), (128, 0, 128), (0, 128, 128), (192, 192, 192),
        (128, 128, 128), (255, 0, 0), (0, 255, 0), (255, 255, 0),
        (0, 0, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255),
    ];
    const STEP: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let indexed = |i: u8| -> (u8, u8, u8) {
        match i {
            0..=15 => BASE[i as usize],
            16..=231 => {
                let n = i - 16;
                (STEP[(n / 36) as usize], STEP[((n % 36) / 6) as usize], STEP[(n % 6) as usize])
            }
            _ => {
                let v = 8 + 10 * (i - 232);
                (v, v, v)
            }
        }
    };
    Some(match c {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Black => indexed(0),
        Color::Red | Color::LightRed => indexed(9),
        Color::Green | Color::LightGreen => indexed(10),
        Color::Yellow | Color::LightYellow => indexed(11),
        Color::Blue | Color::LightBlue => indexed(12),
        Color::Magenta | Color::LightMagenta => indexed(13),
        Color::Cyan | Color::LightCyan => indexed(14),
        Color::White => indexed(15),
        Color::Gray => indexed(7),
        Color::DarkGray => indexed(8),
    })
}

/// WCAG 2.x contrast ratio between two sRGB colours.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    fn lum((r, g, b): (u8, u8, u8)) -> f64 {
        let ch = |v: u8| {
            let c = f64::from(v) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selections are the only place sessio paints a background, so they are the only pairs
    /// whose contrast it fully controls. Both must clear WCAG AA for body text.
    #[test]
    fn selection_text_clears_aa_on_both_selections() {
        let on = rgb(ON_SEL).unwrap();
        for (name, bg) in [("panel", PANEL_SEL), ("tab", TAB_SEL)] {
            let r = contrast(on, rgb(bg).unwrap());
            assert!(r >= 4.5, "{name} selection is {r:.2}:1");
        }
    }

    /// The status glyphs keep their own colour on the focused tab, so they sit on its plum too.
    #[test]
    fn status_colours_stay_legible_on_the_focused_tab() {
        let bg = rgb(TAB_SEL).unwrap();
        let marks = [
            ("attention", ATTENTION),
            ("running", RUNNING),
            ("recent", RECENT),
            ("agent tag", AGENT_TAG),
        ];
        for (name, c) in marks {
            let r = contrast(rgb(c).unwrap(), bg);
            assert!(r >= 3.0, "{name} on the tab selection is {r:.2}:1");
        }
    }

    /// Dark mode is not a prerequisite. The named ANSI roles are the terminal theme's to keep
    /// legible; the fixed ones are ours, and must clear 3:1 (WCAG's floor for glyphs and bold
    /// labels) on a black terminal and a white one alike.
    #[test]
    fn fixed_colours_survive_a_light_terminal() {
        for (name, c) in [("recent", RECENT), ("voice", VOICE), ("agent tag", AGENT_TAG)] {
            let c = rgb(c).unwrap();
            for (bg_name, bg) in [("black", (0, 0, 0)), ("white", (255, 255, 255))] {
                let r = contrast(c, bg);
                assert!(r >= 3.0, "{name} on {bg_name} is {r:.2}:1");
            }
        }
    }

    #[test]
    fn an_error_is_marked_without_colour() {
        assert!(!Tone::Error.mark().is_empty());
        assert_ne!(Tone::Error.style(), Tone::Success.style());
        assert_ne!(Tone::Warning.style(), Tone::Success.style());
        // Only an outcome that happened may be green: in progress is not done.
        assert_ne!(Tone::Pending.style(), Tone::Success.style());
        assert!(!Tone::Pending.mark().is_empty(), "pending reads apart without colour");
    }

    /// Colour lives here and nowhere else. Checked over the source, since a stray `Color::` or a
    /// hand-rolled `.fg(` compiles perfectly well.
    #[test]
    fn no_colour_outside_the_theme() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("theme.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            for (n, line) in src.lines().enumerate() {
                if ["Color::", ".fg(", ".bg(", "Color as"].iter().any(|p| line.contains(p)) {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(offenders.is_empty(), "colour outside src/theme.rs:\n{}", offenders.join("\n"));
    }
}
