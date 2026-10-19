//! D-458: automatic operation-approval review.
//!
//! The reviewer is a separate, read-only model call. It never executes a
//! tool and never decides a capability request; it can only allow or deny the
//! exact operation the control plane has already parked. A failure, timeout or
//! malformed answer leaves the approval pending for the user (a headless run
//! then exits 3 without an effect), which is the fail-closed choice.

use crate::providers::{Cancel, Provider};
use serde_json::{json, Value as Json};
use std::time::Duration;
use teamagents_core::kernel::{estimated_tokens, ModelRequest};

pub const REVIEW_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONTEXT_CHARS: usize = 8_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub risk_level: String,
    pub user_authorization: String,
    pub outcome: Verdict,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewError {
    /// A deterministic policy refusal, decided without a model call.
    HardDenied(String),
    Timeout,
    Transport(String),
    Parse(String),
}

pub fn hard_deny(action: &Json) -> Option<String> {
    if action["name"].as_str() == Some("request_self_update") {
        return Some("auto-review refuses a self-update request; it is user-only".into());
    }
    let text = serde_json::to_string(action).unwrap_or_default().to_ascii_lowercase();
    let checks = [
        ("git push", "publishing a branch"),
        ("gh release", "publishing a release"),
        ("cargo publish", "publishing a crate"),
        ("git tag", "publishing a tag"),
        ("rm -rf /", "recursive deletion outside the workspace"),
        ("sudo ", "privilege escalation"),
        ("/etc/shadow", "credential material"),
        (".ssh/", "credential material"),
        ("id_rsa", "credential material"),
        ("api_key", "credential material"),
        ("token=", "credential material"),
    ];
    checks.iter().find_map(|(needle, why)| text.contains(needle).then(|| format!("auto-review refuses {why}")))
}

pub fn build_request(model: &str, options: &Json, action: &Json, context: &str, scope: &Json) -> ModelRequest {
    let context: String = context.chars().take(MAX_CONTEXT_CHARS).collect();
    let action = serde_json::to_string_pretty(action).unwrap_or_else(|_| "{}".into());
    let messages = vec![
        json!({
            "role": "system",
            "content": "You are an approval reviewer for a coding agent. You do not execute tools and you cannot widen authority. Decide only whether the exact parked operation may run under the user's explicit request and the existing permission scope. Allow only when the action is necessary for that request and within the workspace; deny destructive actions, credential access, publishing/release operations, privilege escalation, network access the user did not request, and anything whose scope you cannot establish. If uncertain, deny. Return strict JSON only: {\"risk_level\":\"low|medium|high|critical\",\"user_authorization\":\"unknown|low|medium|high\",\"outcome\":\"allow|deny\",\"rationale\":\"one sentence\"}. A low-risk action may be {\"outcome\":\"allow\"}."
        }),
        json!({
            "role": "user",
            "content": format!("Recent user/instructions context:\n{context}\n\nPermission scope:\n{scope}\n\nExact parked operation:\n{action}\n\nDecide.")
        }),
    ];
    ModelRequest {
        request_id: format!("review-{}", uuid::Uuid::new_v4()),
        model: model.to_string(),
        est_prompt_tokens: estimated_tokens(&json!({"messages": messages, "tools": []})),
        messages,
        tools: vec![],
        options: options.clone(),
    }
}

pub fn parse(text: &str) -> Result<Assessment, String> {
    let payload = serde_json::from_str::<Json>(text.trim())
        .or_else(|_| {
            let (start, end) = (text.find('{'), text.rfind('}'));
            match (start, end) {
                (Some(start), Some(end)) if start < end => serde_json::from_str(&text[start..=end]),
                _ => Err(serde_json::Error::io(std::io::Error::other("not JSON"))),
            }
        })
        .map_err(|e| format!("reviewer answer was not JSON: {e}"))?;
    let outcome = match payload["outcome"].as_str() {
        Some("allow") => Verdict::Allow,
        Some("deny") => Verdict::Deny,
        other => return Err(format!("reviewer outcome must be allow or deny, got {other:?}")),
    };
    let rationale = payload["rationale"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(match outcome {
            Verdict::Allow => "reviewer allowed a low-risk action",
            Verdict::Deny => "reviewer denied without a rationale",
        })
        .to_string();
    Ok(Assessment {
        risk_level: payload["risk_level"]
            .as_str()
            .unwrap_or(match outcome {
                Verdict::Allow => "low",
                Verdict::Deny => "high",
            })
            .to_string(),
        user_authorization: payload["user_authorization"].as_str().unwrap_or("unknown").to_string(),
        outcome,
        rationale,
    })
}

pub async fn review<P: Provider>(
    provider: &P,
    request: &ModelRequest,
    cancel: &Cancel,
) -> Result<Assessment, ReviewError> {
    let mut on_event = |_| {};
    let future = provider.complete(request, cancel, &mut on_event);
    let outcome = match tokio::time::timeout(REVIEW_TIMEOUT, future).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => return Err(ReviewError::Transport(error.message)),
        Err(_) => {
            cancel.cancel();
            return Err(ReviewError::Timeout);
        }
    };
    let text = outcome.response.message["content"].as_str().unwrap_or("");
    parse(text).map_err(ReviewError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_allow_and_deny_and_defaults() {
        assert_eq!(parse(r#"{"outcome":"allow"}"#).unwrap().outcome, Verdict::Allow);
        assert_eq!(parse(r#"{"outcome":"deny","rationale":"outside scope"}"#).unwrap().outcome, Verdict::Deny);
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"outcome":"maybe"}"#).is_err());
        assert_eq!(parse("prefix {\"outcome\":\"allow\"} suffix").unwrap().outcome, Verdict::Allow);
    }

    #[test]
    fn hard_denies_publication_and_credentials_without_a_model() {
        assert!(hard_deny(&json!({"name": "shell", "args": {"command": "git push origin main"}})).is_some());
        assert!(hard_deny(&json!({"name": "shell", "args": {"command": "cat ~/.ssh/id_rsa"}})).is_some());
        assert!(hard_deny(&json!({"name": "shell", "args": {"command": "cargo test"}})).is_none());
    }

    #[test]
    fn request_has_no_tools_and_carries_the_exact_action() {
        let request = build_request(
            "m",
            &json!({}),
            &json!({"name": "shell", "args": {"command": "ls"}}),
            "do it",
            &json!({"workspace": "/tmp"}),
        );
        assert!(request.tools.is_empty());
        let text = serde_json::to_string(&request.messages).unwrap();
        assert!(text.contains("shell"));
        assert!(text.contains("ls"));
    }
}
