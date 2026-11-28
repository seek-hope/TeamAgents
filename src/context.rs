//! Workspace context discovery: hierarchical AGENTS.md, local skills, git diff/status, and @file index.
use crate::{sandbox::SandboxMode, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};

const INSTRUCTIONS_LIMIT: usize = 64 * 1024;
const SKILL_LIMIT: usize = 128 * 1024;
const DIFF_LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String,
}

fn read_bounded_regular(path: &Path, limit: usize) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.nlink() != 1 || meta.len() > limit as u64 {
        return None;
    }
    let f = File::open(path).ok()?;
    let mut raw = Vec::new();
    f.take(limit as u64 + 1).read_to_end(&mut raw).ok()?;
    if raw.len() > limit {
        return None;
    }
    let text = String::from_utf8(raw).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn xdg_config_root() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Some(xdg);
    }
    env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()).map(|h| h.join(".config"))
}

fn git_root_ancestor(workspace: &Path) -> Option<PathBuf> {
    workspace.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// Discover global (`teamagents/AGENTS.md`) and repo (`AGENTS.override.md` / `AGENTS.md`) instructions.
pub(crate) fn discover_instructions(workspace: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut total = 0usize;

    let global_path = if let Some(explicit) = env::var_os("TEAMAGENTS_GLOBAL_INSTRUCTIONS_FILE") {
        let p = PathBuf::from(explicit);
        (!p.as_os_str().is_empty()).then_some(p)
    } else {
        xdg_config_root().map(|cfg| cfg.join("teamagents/AGENTS.md"))
    };
    if let Some(path) = global_path {
        if let Some(text) = read_bounded_regular(&path, INSTRUCTIONS_LIMIT) {
            total += text.len();
            out.push((path.display().to_string(), text));
        }
    }

    let Ok(canon_ws) = workspace.canonicalize() else {
        return out;
    };
    let stop_at = git_root_ancestor(&canon_ws);
    let mut dirs: Vec<&Path> = Vec::new();
    for anc in canon_ws.ancestors() {
        dirs.push(anc);
        if stop_at.as_deref() == Some(anc) || anc.parent().is_none() {
            break;
        }
        if stop_at.is_none() {
            break;
        }
    }
    dirs.reverse();

    for dir in dirs {
        let picked = ["AGENTS.override.md", "AGENTS.md"].into_iter().find_map(|name| {
            let p = dir.join(name);
            read_bounded_regular(&p, INSTRUCTIONS_LIMIT).map(|txt| (p, txt))
        });
        let Some((path, text)) = picked else {
            continue;
        };
        if total + text.len() > INSTRUCTIONS_LIMIT {
            let avail = INSTRUCTIONS_LIMIT.saturating_sub(total);
            if avail > 64 {
                let end = text.floor_char_boundary(avail);
                out.push((path.display().to_string(), format!("{}\n[... truncated ...]", &text[..end])));
            }
            break;
        }
        total += text.len();
        let label = path
            .strip_prefix(&canon_ws)
            .ok()
            .filter(|r| !r.as_os_str().is_empty())
            .map(|r| r.display().to_string())
            .unwrap_or_else(|| path.display().to_string());
        out.push((label, text));
    }
    out
}

fn parse_skill_frontmatter(raw: &str, fallback_name: &str) -> (String, String) {
    let mut name = fallback_name.to_owned();
    let mut desc = String::new();
    let mut lines = raw.lines();
    if lines.next().is_some_and(|l| l.trim() == "---") {
        for line in lines.by_ref() {
            let trimmed = line.trim();
            if trimmed == "---" {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                let val = v.trim().trim_matches(|c| c == '"' || c == '\'');
                match k.trim() {
                    "name" if !val.is_empty() => name = val.to_owned(),
                    "description" if !val.is_empty() => desc = val.to_owned(),
                    _ => {}
                }
            }
        }
    }
    if desc.is_empty() {
        desc = raw
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && *l != "---" && !l.starts_with('#'))
            .unwrap_or("Local skill instructions")
            .to_owned();
    }
    (name, desc)
}

fn scan_skill_root(root: &Path, scope: &str, out: &mut BTreeMap<String, SkillEntry>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir_path = entry.path();
        if fs::symlink_metadata(&dir_path).ok().is_none_or(|m| m.file_type().is_symlink() || !m.is_dir()) {
            continue;
        }
        let skill_md = dir_path.join("SKILL.md");
        let Some(text) = read_bounded_regular(&skill_md, SKILL_LIMIT) else {
            continue;
        };
        let folder = entry.file_name().to_string_lossy().into_owned();
        let (name, description) = parse_skill_frontmatter(&text, &folder);
        out.insert(
            name.clone(),
            SkillEntry { name, description, path: skill_md.display().to_string(), scope: scope.to_owned() },
        );
    }
}

/// Discover `SKILL.md` skills from global directories and repo `.agents/skills` / `.teamagents/skills`.
pub(crate) fn discover_skills(workspace: &Path) -> Vec<SkillEntry> {
    let mut by_name = BTreeMap::new();
    if let Some(raw) = env::var_os("TEAMAGENTS_SKILLS_DIRS") {
        let s = raw.to_string_lossy();
        for dir in s.split(':').filter(|d| !d.trim().is_empty()) {
            scan_skill_root(Path::new(dir), "global", &mut by_name);
        }
    } else {
        if let Some(cfg) = xdg_config_root() {
            scan_skill_root(&cfg.join("teamagents/skills"), "global", &mut by_name);
        }
        if let Some(home) = env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
            scan_skill_root(&home.join(".agents/skills"), "global", &mut by_name);
            scan_skill_root(&home.join(".codex/skills"), "global", &mut by_name);
        }
    }
    scan_skill_root(&workspace.join(".agents/skills"), "repo", &mut by_name);
    scan_skill_root(&workspace.join(".teamagents/skills"), "repo", &mut by_name);
    by_name.into_values().collect()
}

pub(crate) fn read_skill_doc(workspace: &Path, name: &str) -> Result<String> {
    let trimmed = name.trim();
    let skills = discover_skills(workspace);
    let Some(entry) = skills.into_iter().find(|s| s.name == trimmed) else {
        return Err(format!("unknown skill {trimmed:?}"));
    };
    read_bounded_regular(Path::new(&entry.path), SKILL_LIMIT)
        .ok_or_else(|| format!("unable to read skill {trimmed:?} (missing or exceeds 128 KiB)"))
}

fn valid_git_ref(raw: &str) -> Result<&str> {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('-') || t.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
        return Err(format!("invalid git revision {raw:?}"));
    }
    Ok(t)
}

fn run_git(workspace: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let out = Command::new("git").arg("-C").arg(workspace).args(args).output().map_err(|e| format!("run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn bound_diff_text(mut text: String) -> String {
    if text.len() > DIFF_LIMIT {
        let end = text.floor_char_boundary(DIFF_LIMIT);
        text.truncate(end);
        text.push_str("\n[... diff truncated at 256 KiB ...]\n");
    }
    text
}

pub(crate) fn git_diff_summary(workspace: &Path, base: Option<&str>, commit: Option<&str>) -> Result<String> {
    if let Some(c) = commit {
        let rev = valid_git_ref(c)?;
        let text = run_git(workspace, &["show", "--stat", "--patch", rev])?;
        return Ok(bound_diff_text(text));
    }
    if let Some(b) = base {
        let rev = valid_git_ref(b)?;
        let range = format!("{rev}...HEAD");
        let diff = run_git(workspace, &["diff", &range])?;
        return Ok(bound_diff_text(diff));
    }
    let status = run_git(workspace, &["status", "--short"]).unwrap_or_default();
    let diff = run_git(workspace, &["diff", "HEAD"]).or_else(|_| run_git(workspace, &["diff"])).unwrap_or_default();
    if status.trim().is_empty() && diff.trim().is_empty() {
        return Ok("Working tree is clean (no uncommitted changes).".into());
    }
    let mut out = String::new();
    if !status.trim().is_empty() {
        out.push_str("# git status --short\n");
        out.push_str(&status);
        out.push('\n');
    }
    if !diff.trim().is_empty() {
        out.push_str(&diff);
    }
    Ok(bound_diff_text(out))
}

fn git_branch_snapshot(workspace: &Path) -> Option<String> {
    workspace.canonicalize().ok().and_then(|c| git_root_ancestor(&c))?;
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let status = run_git(workspace, &["status", "--porcelain"]).unwrap_or_default();
    let changed = status.lines().filter(|l| !l.trim().is_empty()).count();
    Some(format!("branch={} (dirty files: {changed})", branch.trim()))
}

pub(crate) fn workspace_file_index(workspace: &Path, max_files: usize) -> Vec<String> {
    let mut files = BTreeSet::new();
    let Ok(canon) = workspace.canonicalize() else {
        return Vec::new();
    };
    let mut stack = vec![(canon.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 10 || files.len() >= max_files {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= max_files {
                break;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if matches!(name_str.as_ref(), ".git" | ".teamagents-auth" | "node_modules") {
                continue;
            }
            let p = entry.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if name_str == "target" && p.join("CACHEDIR.TAG").is_file() {
                    continue;
                }
                stack.push((p, depth + 1));
            } else if meta.is_file() {
                if let Ok(rel) = p.strip_prefix(&canon) {
                    files.insert(rel.to_string_lossy().into_owned());
                }
            }
        }
    }
    files.into_iter().collect()
}

/// Assemble an optional Codex-style workspace context prefix injected once at root submission.
/// Empty, unconfigured workspaces need a prefix only when a non-default sandbox mode applies.
pub(crate) fn build_root_context_prefix(
    workspace: &Path,
    sandbox: SandboxMode,
    mcp_servers: &[String],
) -> Option<String> {
    let instructions = discover_instructions(workspace);
    let skills = discover_skills(workspace);
    let git_info = git_branch_snapshot(workspace);

    if instructions.is_empty()
        && skills.is_empty()
        && git_info.is_none()
        && mcp_servers.is_empty()
        && sandbox == SandboxMode::Direct
    {
        return None;
    }

    let mut sections = Vec::new();
    let mut meta_lines = vec![
        format!(
            "Workspace (host): {} (shell cwd inside sandbox is /workspace; native file tools take workspace-relative paths)",
            workspace.display()
        ),
        format!("Sandbox: {}", sandbox.as_str()),
    ];
    if sandbox.is_read_only() {
        meta_lines.push(
            "Read-only mode active: inspect and analyze without modifying workspace files; MCP tools are disabled because MCP servers run outside the sandbox."
                .into(),
        );
    } else if sandbox.is_staged() {
        meta_lines.push(
            "Staged mode active for shell/check commands; native edit_file/apply_patch write the host workspace directly, and MCP tools are disabled."
                .into(),
        );
    }
    if let Some(git) = git_info {
        meta_lines.push(format!("Git: {git}"));
    }
    if sandbox == SandboxMode::Direct && !mcp_servers.is_empty() {
        meta_lines.push(format!("Configured MCP servers (use mcp_list / mcp_call): {}", mcp_servers.join(", ")));
    }
    sections.push(format!("<environment_context>\n{}\n</environment_context>", meta_lines.join("\n")));

    if !skills.is_empty() {
        let mut list =
            String::from("<available_skills>\nCall `read_skill` with the exact name before using a skill:\n");
        for s in &skills {
            list.push_str(&format!("- {} ({}): {}\n", s.name, s.scope, s.description));
        }
        list.push_str("</available_skills>");
        sections.push(list);
    }

    for (source, body) in instructions {
        sections.push(format!("<instructions source=\"{source}\">\n{body}\n</instructions>"));
    }

    Some(sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = env::temp_dir().join(format!("teamagents-ctx-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_hierarchical_agents_md_and_local_skills() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = Temp::new();
        fs::create_dir(root.0.join(".git")).unwrap();
        fs::create_dir_all(root.0.join("apps/web/.agents/skills/rust-review")).unwrap();
        fs::write(root.0.join("AGENTS.md"), "root rule\n").unwrap();
        fs::write(root.0.join("apps/AGENTS.md"), "apps default\n").unwrap();
        fs::write(root.0.join("apps/AGENTS.override.md"), "apps override\n").unwrap();
        fs::write(
            root.0.join("apps/web/.agents/skills/rust-review/SKILL.md"),
            "---\nname: rust-review\ndescription: Audit unsafe blocks\n---\n# Body\nCheck SAFETY comments.\n",
        )
        .unwrap();

        let prev_global = env::var_os("TEAMAGENTS_GLOBAL_INSTRUCTIONS_FILE");
        let prev_skills = env::var_os("TEAMAGENTS_SKILLS_DIRS");
        env::set_var("TEAMAGENTS_GLOBAL_INSTRUCTIONS_FILE", "");
        env::set_var("TEAMAGENTS_SKILLS_DIRS", "");

        let ws = root.0.join("apps/web");
        let instr = discover_instructions(&ws);
        let texts: Vec<&str> = instr.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, vec!["root rule", "apps override"]);

        let skills = discover_skills(&ws);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "rust-review");
        assert_eq!(skills[0].description, "Audit unsafe blocks");
        let doc = read_skill_doc(&ws, "rust-review").unwrap();
        assert!(doc.contains("Check SAFETY comments."));
        assert!(read_skill_doc(&ws, "../escape").is_err());

        let plain = Temp::new();
        let servers = [String::from("example-mcp")];
        let staged = build_root_context_prefix(&plain.0, SandboxMode::Staged, &[]).unwrap();
        assert!(staged.contains("Staged mode active"));
        for mode in [SandboxMode::ReadOnly, SandboxMode::Staged] {
            let prefix = build_root_context_prefix(&plain.0, mode, &servers).unwrap();
            assert!(prefix.contains("MCP tools are disabled"));
            assert!(!prefix.contains("example-mcp"));
        }
        assert!(build_root_context_prefix(&plain.0, SandboxMode::Direct, &servers).unwrap().contains("example-mcp"));

        match prev_global {
            Some(v) => env::set_var("TEAMAGENTS_GLOBAL_INSTRUCTIONS_FILE", v),
            None => env::remove_var("TEAMAGENTS_GLOBAL_INSTRUCTIONS_FILE"),
        }
        match prev_skills {
            Some(v) => env::set_var("TEAMAGENTS_SKILLS_DIRS", v),
            None => env::remove_var("TEAMAGENTS_SKILLS_DIRS"),
        }
    }
}
