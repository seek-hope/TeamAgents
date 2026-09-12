"""User configuration (TOML, stdlib), TeamSpec import/export (JSON/YAML),
and XDG paths.

Config parsing never executes Python, imports arbitrary modules, or resolves
YAML tags (plan section 14). Project config may add model profiles, tools and
skills, but cannot open full-auto mode or widen pre-authorized scopes.
"""

from __future__ import annotations

import json
import os
import tomllib
from pathlib import Path
from typing import Any

import yaml

from .models import PermissionMode, TeamSpec, UserConfig

APP = "teamagents"


def xdg_config_home() -> Path:
    return Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))


def xdg_state_home() -> Path:
    return Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state"))


def user_config_path() -> Path:
    return xdg_config_home() / APP / "config.toml"


def project_config_path(cwd: Path) -> Path:
    return cwd / ".teamagents" / "config.toml"


def state_dir() -> Path:
    return xdg_state_home() / APP


def sessions_dir() -> Path:
    return state_dir() / "sessions"


def default_session_id(cwd: Path | None = None) -> str:
    """One session per project directory unless the user picks one."""
    base = str((cwd or Path.cwd()).resolve())
    import hashlib
    return "proj_" + hashlib.sha256(base.encode()).hexdigest()[:12]


def _read_toml(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    with path.open("rb") as fh:
        return tomllib.load(fh)


def load_user_config(cwd: Path | None = None) -> UserConfig:
    """User config plus repository-local project config (project wins on names)."""
    data = _read_toml(user_config_path())
    project = _read_toml(project_config_path(cwd or Path.cwd()))
    merged: dict[str, Any] = {
        "models": {**data.get("models", {}), **project.get("models", {})},
        "tools": {**data.get("tools", {}), **project.get("tools", {})},
        "skills_paths": list(data.get("skills_paths", [])) + list(project.get("skills_paths", [])),
        "instruction_files": (list(data.get("instruction_files", []))
                              + list(project.get("instruction_files", []))),
    }
    catalog = UserConfig.model_validate(merged)
    _validate_skill_paths(catalog)
    return catalog


def _validate_skill_paths(catalog: UserConfig) -> None:
    for p in catalog.skills_paths + catalog.instruction_files:
        path = Path(p).expanduser()
        if not path.exists():
            raise ValueError(f"configured skills/instruction path does not exist: {p}")


def permission_mode_from_config(cwd: Path | None = None) -> PermissionMode:
    """Full-auto must be user-chosen in config or CLI; never from a project file."""
    data = _read_toml(user_config_path())
    mode = data.get("permissions", {}).get("mode", "approved_scope")
    if mode not in ("approved_scope", "full_auto"):
        raise ValueError(f"invalid permission mode {mode!r} in user config")
    return PermissionMode(mode)


def load_team_spec(path: str | Path) -> TeamSpec:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    data = json.loads(text) if p.suffix.lower() == ".json" else yaml.safe_load(text)
    if not isinstance(data, dict):
        raise ValueError(f"{p}: team spec must be a mapping")
    return TeamSpec.model_validate(data)


def dump_team_spec(spec: TeamSpec, path: str | Path) -> None:
    p = Path(path)
    data = spec.model_dump(mode="json")
    if p.suffix.lower() == ".json":
        p.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    else:
        p.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")
