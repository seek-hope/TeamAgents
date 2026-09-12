"""Codex-inspired TUI theme.

Palette taken from the local Codex CLI binary (the only truecolor constants it
embeds): white text, `#5d5d5d` secondary/borders, `#0d0d0d` background,
`#3B82F6` accent, `#00ff00` success; errors and warnings keep the terminal's
own ANSI red/yellow, exactly like Codex does. Everything else stays
monochrome so status colors mean something.
"""

from __future__ import annotations

from textual.theme import Theme

WHITE = "#ffffff"
GREY = "#5d5d5d"          # secondary text, borders, dim labels
BACKGROUND = "#0d0d0d"    # codex dark background
PANEL = "#181818"         # status bar / headers / hover
ACCENT = "#3B82F6"        # the single bright accent codex uses
SUCCESS = "#00ff00"       # codex's diff-add / success green
HOVER = "#1f1f1f"

CODEX_THEME = Theme(
    name="codex",
    dark=True,
    primary=ACCENT,
    secondary=GREY,
    accent=ACCENT,
    foreground=WHITE,
    background=BACKGROUND,
    surface=BACKGROUND,
    panel=PANEL,
    boost=PANEL,
    success=SUCCESS,
    warning="ansi_yellow",
    error="ansi_red",
)

#: Rich text styles used inside the conversation log
LABEL_STYLE = "bold #5d5d5d"
BODY_STYLE = "#ffffff"
NOTICE_STYLE = "#afafaf"
ERROR_STYLE = "ansi_red"
SUCCESS_STYLE = "#00ff00"
