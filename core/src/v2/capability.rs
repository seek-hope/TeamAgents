//! The capability vocabulary (§5.1): the `action` names and `resource_scope`
//! shapes a grant can carry, and which of them the runtime actually asks about.
//!
//! `authorized`/`scope_covers`/`active_grant` are generic — they answer any
//! question. The runtime is not: every check that consults a grant asks about
//! one of a handful of concrete `(action, resource)` pairs (`capability_gap`,
//! `complete_task`, `spawn_instance`, `reset_instance`, `set_lifecycle`,
//! `issue_grant`). A grant that covers none of those pairs authorizes nothing:
//! no code path will ever consult it. That is not a security hole (a dead grant
//! grants nothing) but it is a product trap — a user who grants `shell` over
//! `instance:i-worker` believes the worker may run shell commands and it may
//! not. `authorizes_something` is the question a client asks before writing
//! such a row, and the reason `teamagents authority` refuses to write one
//! silently (D-61).

/// The action vocabulary the runtime checks: `shell` (the shared workspace),
/// `manage` (the session or one instance), `message`/`delegate` (one instance)
/// and `task_result` (one task). The control plane stores any string, so a
/// client that accepts input from a human should validate against this list
/// instead of writing rows nothing will ever read.
pub const ACTIONS: &[&str] = &["shell", "message", "delegate", "manage", "task_result"];

/// The shape of a `resource_scope`: the code builds every scope it asks about
/// from one of these (`"session"`, `"workspace"`, `format!("instance:{id}")`,
/// `format!("task:{id}")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeShape {
    /// The whole session; it covers every narrower resource (`scope_covers`).
    Session,
    /// The shared project directory an instance was created in.
    Workspace,
    /// One instance, `instance:<id>`.
    Instance,
    /// One task, `task:<id>`.
    Task,
}

impl ScopeShape {
    /// The shape a scope string has, if it is one of the code's own shapes.
    pub fn of(scope: &str) -> Option<ScopeShape> {
        match scope {
            "session" => Some(ScopeShape::Session),
            "workspace" => Some(ScopeShape::Workspace),
            _ if scope.strip_prefix("instance:").is_some_and(|id| !id.is_empty()) => Some(ScopeShape::Instance),
            _ if scope.strip_prefix("task:").is_some_and(|id| !id.is_empty()) => Some(ScopeShape::Task),
            _ => None,
        }
    }

    /// A scope of this shape, for error messages (`<id>` stands for the
    /// instance or task the user means).
    pub fn example(self) -> &'static str {
        match self {
            ScopeShape::Session => "session",
            ScopeShape::Workspace => "workspace",
            ScopeShape::Instance => "instance:<id>",
            ScopeShape::Task => "task:<id>",
        }
    }
}

/// The resource shape the checks of `action` ask about, or `None` for an action
/// no check consults. `manage` is asked both over the session (spawning) and
/// over one instance (reset/park), so its answer is the narrower shape: a
/// session grant is still meaningful because it covers it.
pub fn asks_about(action: &str) -> Option<ScopeShape> {
    match action {
        "shell" => Some(ScopeShape::Workspace),
        "manage" | "message" | "delegate" => Some(ScopeShape::Instance),
        "task_result" => Some(ScopeShape::Task),
        _ => None,
    }
}

/// Whether a grant of `action` over `scope` authorizes anything at all: it
/// covers at least one resource some check asks about. The session covers every
/// resource, so any known action over `session` qualifies; otherwise the scope
/// must have the shape that action's checks build.
pub fn authorizes_something(action: &str, scope: &str) -> bool {
    let Some(asked) = asks_about(action) else {
        return false;
    };
    match ScopeShape::of(scope) {
        Some(ScopeShape::Session) => true,
        Some(shape) => shape == asked,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{asks_about, authorizes_something, ScopeShape, ACTIONS};

    /// The pairs a user may grant and the pairs the runtime asks about are the
    /// same set (V2Authority's `NoDeadGrantPair`, pinned here): anything the
    /// authority surface accepts, some check consults; anything it refuses, none
    /// does.
    #[test]
    fn the_grantable_pairs_are_exactly_the_asked_ones() {
        let accepted = [
            ("shell", "workspace"),
            ("shell", "session"),
            ("manage", "session"),
            ("manage", "instance:i-worker"),
            ("message", "session"),
            ("message", "instance:i-worker"),
            ("delegate", "session"),
            ("delegate", "instance:i-worker"),
            ("task_result", "session"),
            ("task_result", "task:t-1"),
        ];
        for (action, scope) in accepted {
            assert!(authorizes_something(action, scope), "{action}@{scope} must be grantable");
        }
        let refused = [
            // a shell grant the shell check never asks about (§5.1: shell is a
            // workspace capability)
            ("shell", "instance:i-worker"),
            ("shell", "task:t-1"),
            // a shape nothing builds for these actions
            ("message", "workspace"),
            ("delegate", "task:t-1"),
            ("manage", "workspace"),
            ("task_result", "instance:i-worker"),
            // not in the vocabulary at all
            ("shel", "workspace"),
            ("", "session"),
            // not one of the code's scope shapes
            ("shell", "the-project"),
        ];
        for (action, scope) in refused {
            assert!(!authorizes_something(action, scope), "{action}@{scope} must be refused");
        }
    }

    /// The vocabulary is what the code checks, so the scope shapes are the
    /// code's own constructions.
    #[test]
    fn the_scope_shapes_are_the_codes_own() {
        assert_eq!(ScopeShape::of("session"), Some(ScopeShape::Session));
        assert_eq!(ScopeShape::of("workspace"), Some(ScopeShape::Workspace));
        assert_eq!(ScopeShape::of("instance:i-1"), Some(ScopeShape::Instance));
        assert_eq!(ScopeShape::of("task:t-1"), Some(ScopeShape::Task));
        for bad in ["", "instance:", "task:", "instance", "i-1", "Session"] {
            assert_eq!(ScopeShape::of(bad), None, "{bad:?}");
        }
        assert_eq!(asks_about("shell"), Some(ScopeShape::Workspace));
        assert_eq!(asks_about("nonsense"), None);
        assert_eq!(ACTIONS.len(), 5);
    }
}
