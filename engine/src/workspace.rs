//! Workspace policies: shared / isolated / git worktree (plan §12.3, workspace.py).
//!
//! A worktree isolates working files; it is not a security sandbox. Uncommitted
//! task inputs in the original directory are never ignored silently: the policy
//! falls back to shared mode and says why. Directories with unmerged results,
//! unresolved conflicts or local modifications are never auto-deleted.

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

fn git(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
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
    git(cwd, &["rev-parse", "--git-dir"]).0 == 0
}

pub fn is_dirty(cwd: &Path) -> bool {
    !git(cwd, &["status", "--porcelain"]).1.trim().is_empty()
}

pub fn head_commit(cwd: &Path) -> Option<String> {
    let (code, out, _) = git(cwd, &["rev-parse", "HEAD"]);
    if code == 0 {
        Some(out.trim().to_string())
    } else {
        None
    }
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
            let _ = std::fs::File::create(path.join("INPUTS.md"));
            Ok(Workspace {
                path,
                policy: WorkspacePolicy::Isolated,
                note: Some(
                    "isolated directory: copy inputs explicitly, deliver results through artifact refs".into(),
                ),
                branch: None,
                base_commit: None,
            })
        }
        WorkspacePolicy::GitWorktree => {
            if !is_git_repo(project_cwd) {
                return Ok(fallback(project_cwd, "git_worktree requested but the directory is not a git repository; using shared mode"));
            }
            if is_dirty(project_cwd) {
                return Ok(fallback(project_cwd, "git_worktree requested but the project has uncommitted changes; using shared mode so those inputs are not ignored"));
            }
            let path = member_dir.join("work");
            let base = head_commit(project_cwd);
            if path.join(".git").is_file() {
                // Reopening a session must reuse the worktree this member already
                // owns: a second `worktree add` fails and would hide its work.
                let head = git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]).1.trim().to_string();
                let branch = if head.is_empty() || head == "HEAD" { None } else { Some(head) };
                let merged = git(project_cwd, &["merge-base", "HEAD", branch.as_deref().unwrap_or("HEAD")]);
                return Ok(Workspace {
                    path,
                    policy: WorkspacePolicy::GitWorktree,
                    note: None,
                    branch,
                    base_commit: if merged.1.trim().is_empty() { base } else { Some(merged.1.trim().to_string()) },
                });
            }
            if path.exists() {
                return Err(format!(
                    "{} exists but is not a git worktree (no .git file); archive or remove it before starting a new worktree",
                    path.display()
                ));
            }
            let branch = format!("teamagents/{}-{}", agent.id, teamagents_core::models::now() as i64);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let branch_exists = git(project_cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).0 == 0;
            let path_str = path.to_string_lossy().into_owned();
            let (code, _, err) = if branch_exists {
                // branch left behind by a crashed or removed worktree: re-attach it
                git(project_cwd, &["worktree", "add", &path_str, &branch])
            } else {
                git(
                    project_cwd,
                    &["worktree", "add", "-b", &branch, &path_str, base.as_deref().unwrap_or("HEAD")],
                )
            };
            if code != 0 {
                return Err(format!("git worktree add failed: {}", err.trim()));
            }
            Ok(Workspace {
                path,
                policy: WorkspacePolicy::GitWorktree,
                note: None,
                branch: Some(branch),
                base_commit: base,
            })
        }
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
                if is_dirty(&workspace.path) {
                    return (false, "worktree has uncommitted or unmerged changes; review and merge them first".into());
                }
                let porcelain = git(&workspace.path, &["status", "--porcelain"]).1;
                if porcelain
                    .lines()
                    .any(|line| matches!(&line[..line.len().min(2)], "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD"))
                {
                    return (false, "unresolved merge conflicts in the worktree".into());
                }
                if let Some(branch) = &workspace.branch {
                    let unmerged = git(project_cwd, &["branch", "--no-merged", "HEAD", "--list", branch]).1;
                    if unmerged.contains(branch.as_str()) {
                        return (false, "worktree results are unmerged (committed but not merged); merge them before cleanup".into());
                    }
                }
            }
            let path_str = workspace.path.to_string_lossy().into_owned();
            let mut args: Vec<&str> = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(&path_str);
            let (code, _, err) = git(project_cwd, &args);
            if code != 0 {
                return (false, err.trim().to_string());
            }
            if let Some(branch) = &workspace.branch {
                git(project_cwd, &["branch", if force { "-D" } else { "-d" }, branch]);
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
    let (code, out, err) = git(project_cwd, &["merge", "--no-ff", branch, "-m", &msg]);
    if code == 0 {
        return (true, out.trim().to_string());
    }
    if out.contains("CONFLICT") || err.contains("CONFLICT") {
        return (false, format!("merge conflicts: {}", out.trim()));
    }
    (false, err.trim().to_string())
}

/// Member work directories that are git worktrees (`.git` file, not directory).
pub fn member_worktrees(session_dir: &Path) -> Vec<PathBuf> {
    let members = session_dir.join("members");
    let Ok(entries) = std::fs::read_dir(&members) else { return vec![] };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join("work"))
        .filter(|work| work.join(".git").is_file())
        .collect();
    found.sort();
    found
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
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "T"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
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
        git(&ws.path, &["add", "."]);
        git(&ws.path, &["commit", "-q", "-m", "member work"]);
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
