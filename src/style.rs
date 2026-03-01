use anstyle::{AnsiColor, Color, Effects, Style};
use std::io::IsTerminal;
use std::sync::OnceLock;

fn use_color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal())
}

const GREEN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
const RED_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .effects(Effects::BOLD);
const YELLOW: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const BOLD: Style = Style::new().effects(Effects::BOLD);
const DIM: Style = Style::new().effects(Effects::DIMMED);

pub fn green(s: &str) -> String {
    styled(s, GREEN)
}

pub fn red_bold(s: &str) -> String {
    styled(s, RED_BOLD)
}

pub fn yellow(s: &str) -> String {
    styled(s, YELLOW)
}

pub fn bold(s: &str) -> String {
    styled(s, BOLD)
}

pub fn dim(s: &str) -> String {
    styled(s, DIM)
}

fn styled(s: &str, style: Style) -> String {
    if use_color() {
        format!("{style}{s}{style:#}")
    } else {
        s.to_string()
    }
}
