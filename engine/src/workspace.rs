//! Workspace policies: shared / isolated / git worktree (plan §12.3).
//!
//! A worktree isolates working files; it is not a security sandbox. Uncommitted
//! task inputs in the original directory are never ignored silently: the policy
//! falls back to shared mode and says why. Directories with unmerged results,
//! unresolved conflicts or local modifications are never auto-deleted.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use teamagents_core::models::{AgentSpec, WorkspacePolicy};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub path: PathBuf,
    pub policy: WorkspacePolicy,
    pub note: Option<String>,
    pub branch: Option<String>,
    pub base_commit: Option<String>,
}

fn git<I, S>(cwd: &Path, args: I) -> (i32, String, String)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "core.fsmonitor=false"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output();
    match output {
        Ok(out) => (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Err(e) => (-1, String::new(), e.to_string()),
    }
}

pub fn is_git_repo(cwd: &Path) -> bool {
    git(cwd, ["rev-parse", "--git-dir"]).0 == 0
}

pub fn is_dirty(cwd: &Path) -> bool {
    dirty_status(cwd, false).unwrap_or(true)
}

fn dirty_status(cwd: &Path, include_ignored: bool) -> Result<bool, String> {
    let (code, out, err) = git(
        cwd,
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            if include_ignored { "--ignored=matching" } else { "--ignored=no" },
        ],
    );
    if code != 0 {
        return Err(format!("无法检查工作目录 {}：{}", cwd.display(), err.trim()));
    }
    Ok(!out.is_empty())
}

pub fn head_commit(cwd: &Path) -> Option<String> {
    let (code, out, _) = git(cwd, ["rev-parse", "--verify", "HEAD^{commit}"]);
    if code == 0 {
        Some(out.trim().to_string())
    } else {
        None
    }
}

/// Stored outside the worktree: a checked-out branch name alone is not proof
/// that TeamAgents owns it. Older sessions without this file keep their refs.
#[derive(Serialize, Deserialize)]
struct WorktreeOrigin {
    branch: String,
    base_commit: String,
}

fn origin_path(work: &Path) -> PathBuf {
    work.parent().unwrap_or(work).join("worktree.json")
}

fn read_origin(work: &Path) -> Option<WorktreeOrigin> {
    serde_json::from_slice(&std::fs::read(origin_path(work)).ok()?).ok()
}

fn git_path(cwd: &Path, option: &str) -> Result<PathBuf, String> {
    let (code, out, err) = git(cwd, ["rev-parse", "--path-format=absolute", option]);
    if code != 0 {
        return Err(format!("无法核对 Git 工作目录 {}：{}", cwd.display(), err.trim()));
    }
    std::fs::canonicalize(out.trim()).map_err(|e| format!("无法核对 Git 路径 {}：{e}", cwd.display()))
}

/// Inspect actual Git state, never a cached branch hint or the caller's cwd.
pub(crate) fn inspect_worktree(project_cwd: &Path, path: &Path) -> Result<Workspace, String> {
    let directory = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let marker = std::fs::symlink_metadata(path.join(".git")).map_err(|e| e.to_string())?;
    if !directory.is_dir() || !marker.is_file() {
        return Err(format!("{} 不是有效的成员 Git worktree", path.display()));
    }
    let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if git_path(path, "--show-toplevel")? != canonical
        || git_path(path, "--git-common-dir")? != git_path(project_cwd, "--git-common-dir")?
    {
        return Err(format!("成员工作目录 {} 不属于当前项目，拒绝切换或清理", path.display()));
    }
    let head = head_commit(path).ok_or_else(|| format!("无法读取成员工作目录 {} 的 HEAD", path.display()))?;
    let (code, out, err) = git(path, ["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let branch = match code {
        0 => Some(out.trim().to_string()),
        1 => None, // Detached HEAD is valid, but its commits still need protection.
        _ => return Err(format!("无法读取成员分支：{}", err.trim())),
    };
    let base_commit = read_origin(path).map(|origin| origin.base_commit).or_else(|| {
        let (code, out, _) = git(project_cwd, ["merge-base", "HEAD", &head]);
        (code == 0).then(|| out.trim().to_string())
    });
    Ok(Workspace { path: path.to_path_buf(), policy: WorkspacePolicy::GitWorktree, note: None, branch, base_commit })
}

/// A moved session must not leave a prunable backlink in the project's .git.
pub(crate) fn repair_worktree(project_cwd: &Path, path: &Path) -> Result<(), String> {
    inspect_worktree(project_cwd, path)?;
    let (code, _, err) = git(project_cwd, [OsStr::new("worktree"), OsStr::new("repair"), path.as_os_str()]);
    if code != 0 {
        return Err(format!("无法修复 worktree 注册 {}：{}", path.display(), err.trim()));
    }
    Ok(())
}

/// Resolve where this member works, applying its configured policy.
pub fn prepare(agent: &AgentSpec, project_cwd: &Path, member_dir: &Path) -> Result<Workspace, String> {
    match agent.workspace_policy {
        WorkspacePolicy::Shared => Ok(Workspace {
            path: project_cwd.to_path_buf(),
            policy: WorkspacePolicy::Shared,
            note: None,
            branch: None,
            base_commit: None,
        }),
        WorkspacePolicy::Isolated => {
            let path = member_dir.join("work");
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            // touch, not truncate: reopening a session must keep member notes
            // (`INPUTS.md` is created if missing)
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(path.join("INPUTS.md"));
            Ok(Workspace {
                path,
                policy: WorkspacePolicy::Isolated,
                note: Some("isolated directory: copy inputs explicitly, deliver results through artifact refs".into()),
                branch: None,
                base_commit: None,
            })
        }
        WorkspacePolicy::GitWorktree => {
            let path = member_dir.join("work");
            // Existing member identity wins over the project's current dirtiness.
            // Otherwise a resumed runner silently loses its root and edits the
            // user's shared checkout instead of continuing its own work.
            if path.try_exists().map_err(|e| e.to_string())?
                || origin_path(&path).try_exists().map_err(|e| e.to_string())?
            {
                let workspace = inspect_worktree(project_cwd, &path)?;
                repair_worktree(project_cwd, &path)?;
                return Ok(workspace);
            }
            if !is_git_repo(project_cwd) {
                return Ok(fallback(
                    project_cwd,
                    "git_worktree requested but the directory is not a git repository; using shared mode",
                ));
            }
            if dirty_status(project_cwd, false)? {
                return Ok(fallback(project_cwd, "git_worktree requested but the project has uncommitted changes; using shared mode so those inputs are not ignored"));
            }
            let base = head_commit(project_cwd).ok_or("项目没有可用于建立 worktree 的 HEAD 提交")?;
            let branch = format!("teamagents/{}-{}", agent.id, uuid::Uuid::new_v4().simple());
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let (code, _, err) = git(
                project_cwd,
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("-b"),
                    branch.as_ref(),
                    path.as_os_str(),
                    base.as_ref(),
                ],
            );
            if code != 0 {
                return Err(format!("git worktree add failed: {}", err.trim()));
            }
            let origin = WorktreeOrigin { branch: branch.clone(), base_commit: base.clone() };
            let metadata = origin_path(&path);
            let staged = metadata.with_extension("json.tmp");
            std::fs::write(&staged, serde_json::to_vec(&origin).map_err(|e| e.to_string())?)
                .and_then(|()| std::fs::rename(&staged, &metadata))
                .map_err(|e| format!("worktree 已保留在 {}，但来源记录写入失败：{e}", path.display()))?;
            Ok(Workspace {
                path,
                policy: WorkspacePolicy::GitWorktree,
                note: None,
                branch: Some(branch),
                base_commit: Some(base),
            })
        }
    }
}

/// Fail closed before deleting anything, including a detached or switched HEAD.
pub(crate) fn check_worktree_cleanup(work: &Path, project_cwd: &Path) -> Result<(), String> {
    // ponytail: cooperative session locks cannot exclude external Git/editors
    // between these probes and removal; a durable staged deletion journal would
    // be needed for a crash-atomic lifecycle across the repository and session.
    inspect_worktree(project_cwd, work)?;
    if git_path(work, "--absolute-git-dir")?.join("locked").try_exists().map_err(|e| e.to_string())? {
        return Err(format!("worktree {} 已被 Git 锁定，请先核对并解锁", work.display()));
    }
    // Git worktree remove deletes ignored files even without --force; .gitignore
    // is not permission to discard outputs when deleting a whole session.
    if dirty_status(work, true)? {
        return Err("worktree 存在未提交、被忽略的文件或冲突；请先保留成果再清理".into());
    }
    let head = head_commit(work).ok_or("无法读取成员 HEAD，保留工作目录")?;
    let project_head = head_commit(project_cwd).ok_or("无法读取项目 HEAD，保留工作目录")?;
    let (code, _, err) = git(project_cwd, ["merge-base", "--is-ancestor", &head, &project_head]);
    match code {
        0 => Ok(()),
        1 => Err("worktree results are unmerged (committed but not merged); merge them before cleanup".into()),
        _ => Err(format!("无法验证 worktree 提交是否已合并，保留工作目录：{}", err.trim())),
    }
}

fn fallback(project_cwd: &Path, note: &str) -> Workspace {
    Workspace {
        path: project_cwd.to_path_buf(),
        policy: WorkspacePolicy::Shared,
        note: Some(note.to_string()),
        branch: None,
        base_commit: None,
    }
}

/// Remove an isolated/worktree directory only when nothing would be lost.
pub fn cleanup(workspace: &Workspace, project_cwd: &Path, force: bool) -> (bool, String) {
    match workspace.policy {
        WorkspacePolicy::Shared => (false, "shared workspace is never removed".into()),
        WorkspacePolicy::GitWorktree => {
            if !force {
                if let Err(error) = check_worktree_cleanup(&workspace.path, project_cwd) {
                    return (false, error);
                }
            }
            if let Err(error) = repair_worktree(project_cwd, &workspace.path) {
                return (false, error);
            }
            let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("remove")];
            if force {
                args.push(OsStr::new("--force"));
            }
            args.push(workspace.path.as_os_str());
            let (code, _, err) = git(project_cwd, &args);
            if code != 0 {
                return (false, err.trim().to_string());
            }
            if let Some(origin) = read_origin(&workspace.path) {
                // No -D: even a once-owned branch may have advanced elsewhere.
                // Old sessions without ownership evidence retain their branch.
                let reference = format!("refs/heads/{}", origin.branch);
                if git(project_cwd, ["merge-base", "--is-ancestor", &reference, "HEAD"]).0 == 0 {
                    git(project_cwd, ["branch", "-d", "--", &origin.branch]);
                }
            }
            match std::fs::remove_file(origin_path(&workspace.path)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return (false, format!("worktree 已移除，但来源记录清理失败：{error}；请核对成员目录后重试"));
                }
            }
            (true, "worktree removed".into())
        }
        WorkspacePolicy::Isolated => {
            let entries: Vec<PathBuf> = std::fs::read_dir(&workspace.path)
                .map(|it| it.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            if !entries.is_empty() && !force {
                return (false, "isolated directory still holds results; archive them first".into());
            }
            match std::fs::remove_dir_all(&workspace.path) {
                Ok(()) => (true, "isolated directory removed".into()),
                Err(e) => (false, e.to_string()),
            }
        }
    }
}

/// Leader-side merge helper: merge a member branch, report conflicts.
pub fn merge_branch(project_cwd: &Path, branch: &str, message: Option<&str>) -> (bool, String) {
    let msg = message.map(str::to_string).unwrap_or_else(|| format!("merge {branch}"));
    let (code, out, err) = git(project_cwd, ["merge", "--no-ff", branch, "-m", &msg]);
    if code == 0 {
        return (true, out.trim().to_string());
    }
    if out.contains("CONFLICT") || err.contains("CONFLICT") {
        return (false, format!("merge conflicts: {}", out.trim()));
    }
    (false, err.trim().to_string())
}

/// Candidate Git member roots, including broken markers. Inspection must fail
/// closed rather than treating unreadable or damaged worktrees as absent.
pub fn member_worktrees(session_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let members = session_dir.join("members");
    let entries = match std::fs::read_dir(&members) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("无法检查成员工作目录 {}：{error}", members.display())),
    };
    let mut found = vec![];
    for entry in entries {
        let work = entry.map_err(|e| e.to_string())?.path().join("work");
        match std::fs::symlink_metadata(work.join(".git")) {
            Ok(_) => found.push(work),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if origin_path(&work).try_exists().map_err(|e| e.to_string())? {
                    found.push(work);
                }
            }
            Err(error) => return Err(format!("无法检查工作目录 {}：{error}", work.display())),
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ta-ws-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn agent(id: &str, policy: WorkspacePolicy) -> AgentSpec {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "role": "worker", "runtime_kind": "deepagents",
            "model_profile": "m", "workspace_policy": policy,
        }))
        .unwrap()
    }

    fn init_repo(dir: &Path) {
        git(dir, ["init", "-q", "-b", "main"]);
        git(dir, ["config", "user.email", "t@example.com"]);
        git(dir, ["config", "user.name", "T"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git(dir, ["add", "."]);
        git(dir, ["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn shared_and_isolated_roots() {
        let root = temp("policy");
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let member = root.join("sessions/s1/members/iso");

        let shared = prepare(&agent("a", WorkspacePolicy::Shared), &project, &member).unwrap();
        assert_eq!(shared.path, project);
        assert!(shared.note.is_none());

        let isolated = prepare(&agent("a", WorkspacePolicy::Isolated), &project, &member).unwrap();
        assert_eq!(isolated.path, member.join("work"));
        assert!(isolated.path.join("INPUTS.md").exists());
        assert!(isolated.note.unwrap().contains("isolated directory"));
    }

    #[test]
    fn worktree_lifecycle_and_guards() {
        let root = temp("worktree");
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_repo(&project);
        let member = root.join("sessions/s1/members/dev");

        let ws = prepare(&agent("dev", WorkspacePolicy::GitWorktree), &project, &member).unwrap();
        assert_eq!(ws.policy, WorkspacePolicy::GitWorktree);
        assert!(ws.path.join(".git").is_file(), "a real worktree was created");
        let branch = ws.branch.clone().unwrap();
        assert!(branch.starts_with("teamagents/dev-"));

        // reopening reuses the same worktree instead of failing
        let again = prepare(&agent("dev", WorkspacePolicy::GitWorktree), &project, &member).unwrap();
        assert_eq!(again.path, ws.path);
        assert_eq!(again.branch.as_deref(), Some(branch.as_str()));

        // uncommitted work is never deleted silently
        std::fs::write(ws.path.join("draft.txt"), "wip").unwrap();
        let (ok, reason) = cleanup(&ws, &project, false);
        assert!(!ok, "{reason}");
        assert!(ws.path.join("draft.txt").exists());

        // committed-but-unmerged work is refused too, then a merge unblocks cleanup
        git(&ws.path, ["add", "."]);
        git(&ws.path, ["commit", "-q", "-m", "member work"]);
        let (ok, reason) = cleanup(&ws, &project, false);
        assert!(!ok && reason.contains("unmerged"), "{reason}");
        let (merged, out) = merge_branch(&project, &branch, Some("merge dev"));
        assert!(merged, "{out}");
        let (ok, reason) = cleanup(&ws, &project, false);
        assert!(ok, "{reason}");
        assert!(!ws.path.exists());
    }

    #[test]
    fn dirty_or_unversioned_projects_fall_back_to_shared() {
        let root = temp("fallback");
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let member = root.join("sessions/s1/members/dev");

        let ws = prepare(&agent("dev", WorkspacePolicy::GitWorktree), &project, &member).unwrap();
        assert_eq!(ws.policy, WorkspacePolicy::Shared);
        assert!(ws.note.unwrap().contains("not a git repository"));

        init_repo(&project);
        std::fs::write(project.join("README.md"), "changed").unwrap();
        let ws = prepare(&agent("dev", WorkspacePolicy::GitWorktree), &project, &member).unwrap();
        assert_eq!(ws.policy, WorkspacePolicy::Shared);
        assert!(ws.note.unwrap().contains("uncommitted changes"));
    }
}
