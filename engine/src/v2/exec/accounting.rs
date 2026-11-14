//! Attribute a headless report to immutable request positions and billing-work membership.
//! Batched inputs and overlapping follow-ups can share requests; this is not an exclusive cost allocation.
use super::add_attempt_usage;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};

// ponytail: long runs retain registration/receipt metadata on the client. Beyond these bounds,
// the upgrade path is a paged server-side request ledger, not silent loss of accounting evidence.
const MAX_ACCOUNTING_BYTES: usize = 16 * 1024 * 1024;
const MAX_ACCOUNTING_REQUESTS: usize = 10_000;

struct Request {
    instance: String,
    position: Option<(i64, i64)>,
    born: i64,
    work: Option<String>,
    parent: Option<String>,
    attempts: BTreeMap<String, Json>,
    closed: bool,
    uncertain: bool,
}

#[derive(Default)]
pub(super) struct Accounting {
    input: Option<(i64, i64)>,
    applied_at: Option<i64>,
    end: Option<(i64, i64, String)>,
    requests: BTreeMap<String, Request>,
    admitted: BTreeSet<String>,
    decisions: BTreeMap<String, i64>,
    settlements: BTreeMap<String, i64>,
    retained_bytes: usize,
}

fn position(payload: &Json, index: &str) -> Result<(i64, i64), String> {
    let epoch = payload["epoch"].as_i64().filter(|n| *n >= 0).ok_or("accounting epoch missing")?;
    let head = payload[index].as_i64().filter(|n| *n >= 0).ok_or("accounting context position missing")?;
    Ok((epoch, head))
}

impl Accounting {
    fn reserve(&mut self, bytes: usize) -> Result<(), String> {
        if bytes > MAX_ACCOUNTING_BYTES.saturating_sub(self.retained_bytes) {
            return Err("request accounting exceeds its 16 MiB buffer; outcome and usage are incomplete (export the session for audit)".into());
        }
        self.retained_bytes += bytes;
        Ok(())
    }

    pub(super) fn input_position(&mut self, entry: &Json) -> Result<(), String> {
        let point = position(entry, "idx")?;
        if self.input.is_some_and(|old| old != point) {
            return Err("input accounting position changed".into());
        }
        self.input = Some(point);
        Ok(())
    }

    pub(super) fn outcome(&mut self, entry: &Json) -> Result<(), String> {
        let (epoch, idx) = position(entry, "idx")?;
        let source = entry["envelope_id"].as_str().unwrap_or("").to_string();
        self.end = Some((epoch, idx, source));
        Ok(())
    }

    pub(super) fn observe(&mut self, event: &Json, leader: &str, envelope: &str) -> Result<(), String> {
        let kind = event["kind"].as_str().unwrap_or("");
        let scope = event["scope"].as_str().unwrap_or("");
        let payload = &event["payload"];
        let sequence = event["sequence"].as_i64().ok_or("accounting event sequence missing")?;
        if kind == "input" && scope == leader && payload["envelope_id"] == envelope && payload["applied"] == true {
            let (epoch, head) = position(payload, "context_head")?;
            self.input_position(&json!({"epoch":epoch,"idx":head}))?;
            self.applied_at = Some(sequence);
        }
        if kind == "work_input_admitted" && scope == leader && payload["envelope_id"] == envelope {
            if let Some(work) = payload["work_id"].as_str() {
                if !self.admitted.contains(work) {
                    self.reserve(work.len() + 128)?;
                }
                self.admitted.insert(work.into());
            }
        }
        if kind == "response_imported" && scope == leader {
            if let Some(decision) = payload["decision_id"].as_str() {
                if !self.decisions.contains_key(decision) {
                    self.reserve(decision.len() + 128)?;
                }
                self.decisions.insert(decision.into(), sequence);
            }
        }
        if kind == "work_completed" || kind == "work_cancelled" {
            if let Some(work) = payload["work_id"].as_str() {
                if !self.settlements.contains_key(work) {
                    self.reserve(work.len() + 128)?;
                }
                self.settlements.insert(work.into(), sequence);
            }
        }
        if matches!(kind, "request_began" | "compression_began" | "review_began") {
            let id = payload["request_id"].as_str().ok_or("accounting request id missing")?;
            let parent = if kind == "review_began" {
                Some(payload["parent_request_id"].as_str().ok_or("review accounting parent missing")?.to_string())
            } else {
                None
            };
            let point = if parent.is_none() { Some(position(payload, "context_head")?) } else { None };
            let request = Request {
                instance: scope.into(),
                position: point,
                born: sequence,
                work: payload["work_id"].as_str().map(str::to_string),
                parent,
                attempts: BTreeMap::new(),
                closed: false,
                uncertain: false,
            };
            if self.requests.contains_key(id) {
                return Err("request registered twice in event stream".into());
            }
            if self.requests.len() >= MAX_ACCOUNTING_REQUESTS {
                return Err("request accounting exceeds 10000 registrations; outcome and usage are incomplete (export the session for audit)".into());
            }
            self.reserve(
                scope.len()
                    + id.len()
                    + request.work.as_ref().map_or(0, String::len)
                    + request.parent.as_ref().map_or(0, String::len)
                    + 512,
            )?;
            self.requests.insert(id.into(), request);
        } else if kind == "attempt_recorded" {
            // No registration in this drain means the request predates this run.
            if let Some(request) = self.requests.get(scope) {
                let id = payload["attempt_id"].as_str().ok_or("accounting attempt id missing")?;
                let receipt = json!({"usage":payload["usage"],"cost":payload["cost"]});
                if let Some(old) = request.attempts.get(id) {
                    if old != &receipt {
                        return Err("attempt accounting receipt changed".into());
                    }
                } else {
                    self.reserve(receipt.to_string().len() + id.len() + 128)?;
                    self.requests.get_mut(scope).expect("registered request").attempts.insert(id.into(), receipt);
                }
            }
        } else if matches!(
            kind,
            "response_imported"
                | "request_failed"
                | "request_cancelled"
                | "context_compressed"
                | "compression_failed"
                | "review_attempt_closed"
                | "review_interrupted"
        ) {
            if let Some(request) = payload["request_id"].as_str().and_then(|id| self.requests.get_mut(id)) {
                request.closed = true;
                request.uncertain |=
                    kind == "request_cancelled" || kind == "review_interrupted" || payload["unknown_usage"] == true;
            }
        }
        Ok(())
    }

    pub(super) fn finish(&self, leader: &str) -> Json {
        let mut usage = json!({"prompt_tokens":0,"completion_tokens":0,"total_tokens":0,
            "cache_hit_tokens":0,"cache_miss_tokens":0,"cache_write_tokens":0,"requests":0,"unpriced_attempts":0,
            "cost":{"currency":"USD","known_estimated_usd":0.0,"unpriced_attempts":0,"complete":true}});
        let Some(input) = self.input else { return usage };
        let mut owned = BTreeSet::new();
        let mut works = self.admitted.clone();
        for (id, request) in &self.requests {
            if request.instance == leader
                && request.position.is_some_and(|point| {
                    point.0 == input.0
                        && point.1 >= input.1
                        && self.end.as_ref().is_none_or(|end| point < (end.0, end.1))
                })
            {
                owned.insert(id.as_str());
                if let Some(work) = &request.work {
                    works.insert(work.clone());
                }
            }
        }
        let end_sequence = self.end.as_ref().and_then(|(_, _, source)| {
            if let Some(work) = source.strip_prefix("work-close-").or_else(|| source.strip_prefix("work-block-")) {
                self.settlements.get(work).copied()
            } else {
                let decision = source
                    .strip_prefix("work-question-")
                    .or_else(|| source.strip_prefix("work-blocked-"))
                    .unwrap_or(source);
                self.decisions.get(decision).copied()
            }
        });
        // Members are charged by frozen billing work, bounded by this input's application and outcome.
        // If an unusual closing note lacks an event identity, keep the subtotal conservative and incomplete.
        let bounded = self.applied_at.is_some() && (self.end.is_none() || end_sequence.is_some());
        if !bounded {
            usage["cost"]["complete"] = json!(false);
        }
        for (id, request) in &self.requests {
            if bounded
                && request.parent.is_none()
                && request.instance != leader
                && request.work.as_ref().is_some_and(|work| works.contains(work))
                && request.born >= self.applied_at.unwrap_or(i64::MAX)
                && end_sequence.is_none_or(|end| request.born < end)
            {
                owned.insert(id.as_str());
            }
        }
        for (id, request) in &self.requests {
            if request.parent.as_deref().is_some_and(|parent| owned.contains(parent)) {
                owned.insert(id.as_str());
            }
        }
        for id in owned {
            let request = &self.requests[id];
            for receipt in request.attempts.values() {
                add_attempt_usage(&mut usage, receipt);
            }
            if !request.closed || request.uncertain || request.attempts.is_empty() {
                usage["cost"]["complete"] = json!(false);
            }
        }
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fixture {
        accounting: Accounting,
        sequence: i64,
    }
    impl Fixture {
        fn event(&mut self, kind: &str, scope: &str, payload: Json) {
            self.sequence += 1;
            self.accounting
                .observe(
                    &json!({"sequence":self.sequence,"kind":kind,"scope":scope,"payload":payload}),
                    "leader",
                    "mine",
                )
                .unwrap();
        }
        fn input(&mut self) {
            self.event("input", "leader", json!({"envelope_id":"mine","applied":true,"epoch":2,"context_head":20}));
        }
        fn begin(&mut self, id: &str, instance: &str, work: &str, epoch: i64, head: i64, kind: &str) {
            self.event(kind, instance, json!({"request_id":id,"work_id":work,"epoch":epoch,"context_head":head}));
        }
        fn receipt(&mut self, id: &str, amount: u64) {
            self.event(
                "attempt_recorded",
                id,
                json!({"attempt_id":format!("{id}/1"),"usage":{"total_tokens":amount},"cost":{"estimated_usd":amount}}),
            );
        }
        fn close(&mut self, id: &str, instance: &str, kind: &str) {
            self.event(kind, instance, json!({"request_id":id,"decision_id":format!("decision-{id}")}));
        }
        fn review(&mut self, id: &str, parent: &str) {
            self.event("review_began", "leader", json!({"request_id":id,"parent_request_id":parent,"work_id":"W"}));
        }
    }

    #[test]
    fn request_positions_keep_shared_calls_and_exclude_earlier_and_later_work() {
        let mut f = Fixture::default();
        f.begin("old", "leader", "W", 2, 19, "request_began");
        f.input();
        f.begin("compression", "leader", "W", 2, 20, "compression_began");
        f.receipt("compression", 2);
        f.close("compression", "leader", "context_compressed");
        // The request contains this input plus a later batched input at index 21.
        f.begin("own", "leader", "W", 2, 21, "request_began");
        f.begin("member", "worker", "W", 7, 80, "request_began");
        f.begin("other", "worker", "other-work", 2, 20, "request_began");
        f.begin("old-epoch", "leader", "W", 1, 100, "request_began");
        f.review("review-own", "member");
        f.review("review-old", "old");
        for (id, amount) in [
            ("old", 101),
            ("other", 103),
            ("old-epoch", 107),
            ("review-old", 109),
            ("own", 3),
            ("member", 5),
            ("review-own", 7),
        ] {
            f.receipt(id, amount);
        }
        f.close("member", "worker", "response_imported");
        f.close("review-own", "operation", "review_attempt_closed");
        f.close("own", "leader", "response_imported");
        f.accounting.outcome(&json!({"epoch":2,"idx":22,"envelope_id":"decision-own"})).unwrap();
        // The terminal drain also sees a later follow-up in the same work.
        f.begin("later-leader", "leader", "W", 2, 23, "request_began");
        f.begin("later-worker", "worker", "W", 7, 90, "request_began");
        f.receipt("later-leader", 113);
        f.receipt("later-worker", 127);
        let usage = f.accounting.finish("leader");
        assert_eq!(usage["requests"], 4, "{usage}");
        assert_eq!(usage["total_tokens"], 17, "{usage}");
        assert_eq!(usage["cost"]["known_estimated_usd"], 17.0, "{usage}");
        assert_eq!(usage["cost"]["complete"], true, "{usage}");
    }

    #[test]
    fn unrelated_traffic_cannot_grow_the_accounting_buffer_without_limit() {
        for work in ["unrelated".to_string(), "W".repeat(512 * 1024)] {
            let mut accounting = Accounting::default();
            let failure = (1..=20_000)
                .find_map(|sequence| {
                    accounting
                        .observe(
                            &json!({"kind":"request_began","scope":"worker","sequence":sequence,
                    "payload":{"request_id":format!("other-{sequence}"),"epoch":0,"context_head":1,"work_id":work}}),
                            "leader",
                            "mine",
                        )
                        .err()
                })
                .expect("bounded accounting must refuse oversized accumulated traffic");
            assert!(failure.contains("accounting exceeds") && failure.contains("incomplete"), "{failure}");
        }
    }

    #[test]
    fn open_cancelled_unreceipted_and_review_requests_prevent_complete_cost() {
        for case in ["open", "cancelled", "unreceipted", "review", "member", "priced-failure", "retry"] {
            let mut f = Fixture::default();
            f.input();
            f.begin("own", "leader", "W", 2, 20, "request_began");
            if case != "unreceipted" {
                f.receipt("own", 3);
            }
            if case == "retry" {
                f.event("attempt_recorded", "own", json!({"attempt_id":"own/2","status":"COMPLETE","usage":{"total_tokens":5},"cost":{"estimated_usd":5}}));
            }
            if case != "open" {
                f.close("own", "leader", if case == "cancelled" { "request_cancelled" } else { "request_failed" });
            }
            if case == "review" {
                f.review("pending-review", "own");
            }
            if case == "member" {
                f.begin("pending-member", "worker", "W", 8, 9, "request_began");
            }
            let usage = f.accounting.finish("leader");
            assert_eq!(usage["cost"]["complete"], matches!(case, "priced-failure" | "retry"), "{case}: {usage}");
            assert_eq!(
                usage["total_tokens"],
                if case == "unreceipted" {
                    0
                } else if case == "retry" {
                    8
                } else {
                    3
                },
                "{case}: {usage}"
            );
        }
    }
}
