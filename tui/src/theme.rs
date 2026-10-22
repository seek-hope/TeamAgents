//! ANSI palette matching oh-my-zsh's default `robbyrussell` theme: bold green
//! success, bold red failure, cyan directory, bold blue git and yellow dirt.
//! Only the terminal's 16 colours are used, so the TUI follows the same palette
//! as the user's shell instead of imposing an RGB one.

use ratatui::style::Color;

/// Default terminal text (`%f` in the shell prompt).
pub const FG: Color = Color::White;
pub const GREY: Color = Color::DarkGray;
pub const MUTED: Color = Color::DarkGray;
pub const BG: Color = Color::Black;
pub const PANEL_BG: Color = Color::Black;
pub const CHIP_BG: Color = Color::DarkGray;
pub const BORDER: Color = Color::DarkGray;
/// `%{$fg_bold[blue]%}`: the git segment and the single accent.
pub const ACCENT: Color = Color::LightBlue;
pub const USER_BG: Color = Color::DarkGray;
/// `%{$fg[cyan]%}`: the directory segment.
pub const CODE: Color = Color::Cyan;
/// `%{$fg_bold[green]%}`.
pub const ADDED: Color = Color::LightGreen;
pub const NOTICE: Color = Color::White;
/// `%{$fg_bold[red]%}`.
pub const ERROR: Color = Color::LightRed;
/// `%{$fg[yellow]%}`.
pub const WARNING: Color = Color::Yellow;
pub const SUCCESS: Color = Color::LightGreen;
