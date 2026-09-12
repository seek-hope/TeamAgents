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
import warnings
from pathlib import Path
from typing import Any

import yaml

from .models import PermissionMode, TeamSpec, UserConfig

APP = "teamagents"

# User-level opt-in that lets repository-local tool bindings (MCP commands,
# web tools) take effect. Default off: opening a cloned repository must never
# be enough to run its configured commands (plan sections 12.2 and 14).
TRUST_PROJECT_TOOLS = "trust_project_tools"


class ProjectConfigWarning(UserWarning):
    """A repository-local .teamagents/config.toml entry was ignored."""


def _warn_ignored(message: str) -> None:
    # stacklevel: _warn_ignored -> _merge_* -> load_user_config -> caller
    warnings.warn(f"teamagents: {message}", ProjectConfigWarning, stacklevel=4)


def _trust_project_tools(user_data: dict[str, Any]) -> bool:
    """Read the user-config opt-in for project-defined tool bindings."""
    permissions = user_data.get("permissions", {})
    if not isinstance(permissions, dict):
        raise ValueError("[permissions] must be a table in user config")
    value = permissions.get(TRUST_PROJECT_TOOLS, False)
    if not isinstance(value, bool):
        raise ValueError(
            f"permissions.{TRUST_PROJECT_TOOLS} must be true/false, got {value!r}")
    return value


def _merge_project_models(user_models: dict[str, Any],
                          project_models: dict[str, Any]) -> dict[str, Any]:
    """Project may add profiles; a user-defined name always wins."""
    merged = dict(user_models)
    for name, profile in project_models.items():
        if name in user_models:
            _warn_ignored(f"项目配置试图覆盖用户模型 profile {name!r}，已忽略项目定义")
            continue
        merged[name] = profile
    return merged


def _merge_project_tools(user_tools: dict[str, Any], project_tools: dict[str, Any],
                         *, trusted: bool) -> dict[str, Any]:
    """Project tools load only after the user opts in; user-defined names win."""
    merged = dict(user_tools)
    for name, binding in project_tools.items():
        if name in user_tools:
            _warn_ignored(f"项目配置试图覆盖用户工具 {name!r}，已忽略项目定义")
            continue
        if not trusted:
            _warn_ignored(
                f"项目配置定义了工具 {name!r}，默认不信任项目工具，已忽略；"
                f"确认安全后可在用户配置设置 [permissions] {TRUST_PROJECT_TOOLS} = true")
            continue
        merged[name] = binding
    return merged


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
    """User config plus repository-local project config (user-defined names win).

    A project file may add model profiles, but it can never replace a
    user-defined name and its tool bindings only load after the user opts in
    with ``[permissions] trust_project_tools = true`` (plan sections 12.2/14).
    """
    data = _read_toml(user_config_path())
    project = _read_toml(project_config_path(cwd or Path.cwd()))
    merged: dict[str, Any] = {
        "models": _merge_project_models(data.get("models", {}), project.get("models", {})),
        "tools": _merge_project_tools(data.get("tools", {}), project.get("tools", {}),
                                      trusted=_trust_project_tools(data)),
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
