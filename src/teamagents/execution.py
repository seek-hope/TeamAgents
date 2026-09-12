"""Linux execution backend: bubblewrap-isolated shell + guarded file backend.

Mounts and network flags are constructed here and unit-tested (T23); the
isolation facility being unavailable must never silently degrade to full
access (plan section 12.2).
"""

from __future__ import annotations

import dataclasses
import os
import signal
import subprocess
import time
from pathlib import Path
from typing import Any, Sequence

from deepagents.backends import FilesystemBackend
from deepagents.backends.protocol import (
    DeleteResult,
    EditResult,
    ExecuteResponse,
    FileUploadResponse,
    WriteResult,
)

#: read-only system paths mounted into every isolated command when present
SYSTEM_RO = ["/usr", "/etc", "/opt"]
SYSTEM_LINKS = [("usr/lib", "/lib"), ("usr/lib64", "/lib64"),
                ("usr/bin", "/bin"), ("usr/bin", "/sbin")]


class IsolationUnavailable(RuntimeError):
    """Raised when bwrap cannot be used; callers must ask for explicit approval."""


@dataclasses.dataclass
class ExecResult:
    output: str
    exit_code: int | None
    truncated: bool = False
    artifact: str | None = None
    timed_out: bool = False


def bwrap_available() -> bool:
    return _which("bwrap") is not None


def _which(name: str) -> str | None:
    for p in os.environ.get("PATH", "/usr/bin:/bin").split(":"):
        cand = Path(p) / name
        if cand.exists() and os.access(cand, os.X_OK):
            return str(cand)
    return None


def _sanitized_env(workdir: Path, extra: dict[str, str] | None) -> dict[str, str]:
    """Whitelist environment: no model keys, no credentials (plan section 12.2)."""
    env = {
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "HOME": str(workdir),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
        "LC_ALL": os.environ.get("LC_ALL", ""),
        "TERM": "dumb",
        "TMPDIR": "/tmp",
        "PYTHONIOENCODING": "utf-8",
    }
    for key, value in (extra or {}).items():
        env[key] = value
    return {k: v for k, v in env.items() if v != ""}


def bwrap_argv(workdir: Path, *, network: bool = False,
               extra_rw: Sequence[Path] = (), extra_ro: Sequence[Path] = (),
               command: str = "true") -> list[str]:
    """Construct the isolation command line (system files ro, workdir rw, no net)."""
    argv = ["bwrap"]
    for path in SYSTEM_RO:
        if Path(path).exists():
            argv += ["--ro-bind", path, path]
    for target, link in SYSTEM_LINKS:
        # always create the link inside the new root (the host's /bin is itself a
        # symlink, so host existence checks are meaningless here)
        if Path("/" + target).exists():
            argv += ["--symlink", target, link]
    argv += ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]
    argv += ["--bind", str(workdir), str(workdir)]
    for path in extra_rw:
        argv += ["--bind", str(path), str(path)]
    for path in extra_ro:
        argv += ["--ro-bind", str(path), str(path)]
    argv += ["--chdir", str(workdir)]
    argv += ["--unshare-pid", "--unshare-ipc", "--unshare-uts", "--die-with-parent",
             "--new-session"]
    if not network:
        argv += ["--unshare-net"]
    argv += ["--", "/bin/bash", "-lc", command]
    return argv


def run_isolated(command: str, *, workdir: Path, timeout: int = 120,
                 network: bool = False, extra_rw: Sequence[Path] = (),
                 extra_ro: Sequence[Path] = (), env: dict[str, str] | None = None,
                 max_output_bytes: int = 200 * 1024,
                 artifact_dir: Path | None = None) -> ExecResult:
    """Run one command inside the sandbox. Process-group kill on timeout."""
    if not bwrap_available():
        raise IsolationUnavailable(
            "bwrap is not available: refusing to run commands without isolation")
    workdir = Path(workdir).resolve()
    argv = bwrap_argv(workdir, network=network, extra_rw=extra_rw, extra_ro=extra_ro,
                      command=command)
    proc = subprocess.Popen(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=_sanitized_env(workdir, env),
        start_new_session=True,
        text=False,
    )
    timed_out = False
    try:
        out_bytes, _ = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        _kill_group(proc)
        out_bytes, _ = proc.communicate()
    output = out_bytes.decode("utf-8", errors="replace")
    truncated = len(out_bytes) > max_output_bytes
    artifact = None
    if truncated:
        artifact = _save_artifact(output, artifact_dir)
        output = output[:max_output_bytes] + (
            f"\n[output truncated at {max_output_bytes} bytes; full output: {artifact}]")
    if timed_out:
        output += f"\n[command killed after {timeout}s timeout]"
    return ExecResult(output=output, exit_code=proc.returncode, truncated=truncated,
                      artifact=artifact, timed_out=timed_out)


def _kill_group(proc: subprocess.Popen) -> None:
    """Kill the whole process group so no children survive (plan section 9.3)."""
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass


def _save_artifact(output: str, artifact_dir: Path | None) -> str | None:
    if artifact_dir is None:
        return None
    artifact_dir.mkdir(parents=True, exist_ok=True)
    name = f"exec-{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}.log"
    path = artifact_dir / name
    path.write_text(output, encoding="utf-8")
    return f"/artifacts/{name}"


# ---------------------------------------------------------------------------
# Deep Agents backends
# ---------------------------------------------------------------------------


class GuardedFilesystemBackend(FilesystemBackend):
    """FilesystemBackend with symlink-aware containment under its root.

    String prefix checks are not acceptable (plan section 12.2): every write
    resolves the real path (including symlinked parents) and refuses escapes.
    Reads of symlinks pointing outside the root are refused too.
    """

    def __init__(self, root_dir: str | Path, max_file_size_mb: int = 10):
        super().__init__(root_dir=root_dir, virtual_mode=True,
                         max_file_size_mb=max_file_size_mb)
        self._root_real = Path(root_dir).resolve()

    def _resolve_path(self, key: str) -> Path:
        """One guard for every file operation: real path (symlinks included) must
        stay under the authorized root."""
        try:
            resolved = super()._resolve_path(key)
        except ValueError as e:
            # normalize the base class' traversal rejection into a guard error so
            # tool-level callers get a result instead of an exception
            raise PermissionError(
                f"path {key!r} rejected: {e}") from e
        root = self._root_real
        probe, tail = resolved, []
        while not probe.exists() and probe != probe.parent:
            tail.append(probe.name)
            probe = probe.parent
        real = probe.resolve()
        if tail:
            real = real.joinpath(*reversed(tail))
        try:
            real.relative_to(root)
        except ValueError as e:
            raise PermissionError(
                f"path {key!r} resolves outside the authorized directory "
                f"({real} is not under {root})") from e
        return resolved


class ReadOnlyFilesystemBackend(GuardedFilesystemBackend):
    """Guarded backend that refuses every mutation (memory/skills routes).

    The model may read these locations but must never write them: the error is
    returned as a failed operation result (never raised), so the tool call
    fails cleanly and the turn continues (plan section 12.2).
    """

    def __init__(self, root_dir: str | Path, *, virtual_prefix: str = "",
                 max_file_size_mb: int = 10):
        super().__init__(root_dir, max_file_size_mb=max_file_size_mb)
        # route prefix stripped by CompositeBackend, echoed so the model sees
        # the path it actually asked for
        self._virtual_prefix = virtual_prefix.rstrip("/")

    def _reject(self, path: str) -> str:
        shown = f"{self._virtual_prefix}/{path.lstrip('/')}"
        return (f"Error: {shown} is read-only for members; this location is owned "
                f"by the user and cannot be modified")

    def write(self, file_path: str, content: str) -> WriteResult:
        return WriteResult(error=self._reject(file_path))

    def edit(self, file_path: str, old_string: str, new_string: str,
             replace_all: bool = False) -> EditResult:
        return EditResult(error=self._reject(file_path))

    def delete(self, file_path: str) -> DeleteResult:
        return DeleteResult(error=self._reject(file_path))

    def upload_files(self, files: list[tuple[str, bytes]]) -> list[FileUploadResponse]:
        return [FileUploadResponse(path=path, error="permission_denied")
                for path, _content in files]


class IsolatedShellBackend(GuardedFilesystemBackend):
    """Filesystem backend whose `execute` runs inside bubblewrap (plan §12.2).

    Network is withheld unless the caller explicitly allowed it; commands that
    cannot be isolated raise instead of being silently run unsandboxed.
    """

    def __init__(self, root_dir: str | Path, *, artifacts_dir: str | Path | None = None,
                 extra_rw: Sequence[Path] = (), extra_ro: Sequence[Path] = (),
                 network: bool = False, timeout: int = 120,
                 max_output_bytes: int = 200 * 1024,
                 env: dict[str, str] | None = None):
        super().__init__(root_dir)
        self.id = f"bwrap:{Path(root_dir).name}"
        self.artifacts_dir = Path(artifacts_dir) if artifacts_dir else None
        self.extra_rw = list(extra_rw)
        self.extra_ro = [Path(p) for p in extra_ro]
        self.network = network
        self.timeout = timeout
        self.max_output_bytes = max_output_bytes
        self.env = env or {}

    # -- SandboxBackendProtocol ---------------------------------------------

    def execute(self, command: str, *, timeout: int | None = None) -> ExecuteResponse:
        result = run_isolated(
            command,
            workdir=Path(self.cwd),
            timeout=timeout or self.timeout,
            network=self.network,
            extra_rw=self.extra_rw,
            extra_ro=self.extra_ro,
            env=self.env,
            max_output_bytes=self.max_output_bytes,
            artifact_dir=self.artifacts_dir,
        )
        return ExecuteResponse(output=result.output, exit_code=result.exit_code,
                               truncated=result.truncated)

    async def aexecute(self, command: str, *, timeout: int | None = None) -> ExecuteResponse:
        import asyncio
        return await asyncio.to_thread(self.execute, command, timeout=timeout)
