//! Optional, source-labelled USD estimates. Token admission budgets remain independent.

use crate::kernel::Usage;
use crate::models::PriceRates;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};

impl PriceRates {
    pub fn validate(&self) -> Result<(), String> {
        for rate in [
            Some(self.input_usd_per_million),
            Some(self.output_usd_per_million),
            self.cache_read_usd_per_million,
            self.cache_write_usd_per_million,
        ]
        .into_iter()
        .flatten()
        {
            if !rate.is_finite() || !(0.0..=1_000_000_000.0).contains(&rate) {
                return Err("price rates must be finite, nonnegative and at most 1e9 USD per million tokens".into());
            }
        }
        if self.source.trim().is_empty() || self.source.len() > 512 || self.source.chars().any(char::is_control) {
            return Err("price source must be nonempty, at most 512 bytes and free of control characters".into());
        }
        Ok(())
    }

    fn estimate(&self, usage: Usage) -> Result<f64, &'static str> {
        let uncached = usage
            .prompt
            .checked_sub(usage.cache_hit)
            .and_then(|n| n.checked_sub(usage.cache_write))
            .ok_or("inconsistent_cache_usage")?;
        let reads = if usage.cache_hit == 0 {
            0.0
        } else {
            usage.cache_hit as f64 * self.cache_read_usd_per_million.ok_or("missing_cache_read_rate")?
        };
        let writes = if usage.cache_write == 0 {
            0.0
        } else {
            usage.cache_write as f64 * self.cache_write_usd_per_million.ok_or("missing_cache_write_rate")?
        };
        Ok((uncached as f64 * self.input_usd_per_million
            + usage.completion as f64 * self.output_usd_per_million
            + reads
            + writes)
            / 1_000_000.0)
    }
}

/// Registration and the request share a transaction. Missing historical prices stay unknown.
pub(super) fn freeze(conn: &Connection, request: &str, value: &Json) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let model = value["model"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or("pricing.model missing or too long")?;
    let rates: Option<PriceRates> =
        serde_json::from_value(value["rates"].clone()).map_err(|e| format!("pricing rates: {e}"))?;
    if let Some(rates) = &rates {
        rates.validate()?;
    }
    conn.execute(
        "UPDATE model_requests SET pricing_json=?2 WHERE request_id=?1 AND pricing_json IS NULL",
        rusqlite::params![request, json!({"model":model,"rates":rates}).to_string()],
    )
    .map_err(|e| format!("freeze pricing: {e}"))?;
    Ok(())
}

pub(super) fn attempt_cost(conn: &Connection, request: &str, usage: &Json) -> Result<Json, String> {
    let raw: Option<String> = conn
        .query_row("SELECT pricing_json FROM model_requests WHERE request_id=?1", [request], |row| row.get(0))
        .optional()
        .map_err(|e| format!("request pricing: {e}"))?
        .flatten();
    let frozen: Json = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| format!("stored pricing: {e}"))?
        .unwrap_or(Json::Null);
    let rates: Option<PriceRates> =
        serde_json::from_value(frozen["rates"].clone()).map_err(|e| format!("stored rates: {e}"))?;
    let result = match (Usage::from_json(&json!({"usage":usage})), rates.as_ref()) {
        (None, _) => Err("missing_usage"),
        (_, None) => Err("missing_rates"),
        (Some(usage), Some(rates)) => rates.estimate(usage),
    };
    Ok(json!({"currency":"USD","estimated_usd":result.as_ref().ok(),
        "status":result.err().unwrap_or("estimated"), "model":frozen["model"], "rates":rates}))
}

/// One work may include later-attributed admission calls as well as turn, compression and review calls.
pub fn work_costs(conn: &Connection) -> Result<std::collections::HashMap<String, Json>, String> {
    let mut stmt = conn
        .prepare(
            "WITH receipts AS (
              SELECT COALESCE(w.billed_work_id,r.work_id) AS work_id,
                SUM(json_extract(a.cost_json,'$.estimated_usd')) AS amount,
                SUM(CASE WHEN a.attempt_id IS NOT NULL AND json_extract(a.cost_json,'$.estimated_usd') IS NULL THEN 1 ELSE 0 END) AS unpriced,
                COUNT(a.attempt_id) AS attempts,
                COUNT(DISTINCT CASE WHEN r.status='PENDING' THEN r.request_id END) AS pending,
                SUM(COALESCE(json_extract(a.usage_json,'$.total_tokens'),json_extract(a.usage_json,'$.total'),0)) AS tokens,
                SUM(CASE WHEN a.attempt_id IS NOT NULL AND (a.usage_json IS NULL OR a.usage_json='null') THEN 1 ELSE 0 END) AS unknown_usage,
                SUM(CASE WHEN r.status!='PENDING' AND a.attempt_id IS NULL THEN 1 ELSE 0 END) AS unreceipted
              FROM model_requests r LEFT JOIN attempts a ON r.request_id=a.request_id
              LEFT JOIN work_requests w ON w.request_id=r.request_id
              GROUP BY COALESCE(w.billed_work_id,r.work_id)
            ) SELECT g.id,COALESCE(r.amount,0.0),COALESCE(r.unpriced,0),COALESCE(r.attempts,0),COALESCE(r.pending,0),
                (COALESCE(json_extract(g.known_usage_json,'$.total'),0)>COALESCE(r.tokens,0)
                 OR g.unknown_usage>COALESCE(r.unknown_usage,0) OR COALESCE(r.unreceipted,0)>0)
              FROM work_scopes g LEFT JOIN receipts r ON r.work_id=g.id",
        )
        .map_err(|e| format!("work cost prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let known: f64 = row.get(1)?;
            let unknown: i64 = row.get(2)?;
            let attempts: i64 = row.get(3)?;
            let pending: i64 = row.get(4)?;
            let unrepresented: bool = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                json!({"currency":"USD","known_estimated_usd":known,
            "unpriced_attempts":unknown,"attempts":attempts,"pending_requests":pending,
            "usage_without_receipt":unrepresented,"complete":unknown==0 && pending==0 && !unrepresented}),
            ))
        })
        .map_err(|e| format!("work cost query: {e}"))?;
    rows.collect::<Result<_, _>>().map_err(|e| format!("work cost rows: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn estimates_cache_categories_without_double_counting_or_guessing() {
        let mut rates = PriceRates {
            input_usd_per_million: 2.0,
            output_usd_per_million: 8.0,
            cache_read_usd_per_million: Some(0.2),
            cache_write_usd_per_million: Some(3.0),
            source: "fixture tariff".into(),
        };
        rates.validate().unwrap();
        let usage =
            Usage { prompt: 1000, completion: 100, total: 1100, cache_hit: 400, cache_miss: 400, cache_write: 200 };
        assert!((rates.estimate(usage).unwrap() - 0.00228).abs() < 1e-12);
        rates.cache_write_usd_per_million = None;
        assert_eq!(rates.estimate(usage), Err("missing_cache_write_rate"));
        assert!(rates.estimate(Usage { prompt: 1, ..usage }).is_err());
        rates.input_usd_per_million = f64::NAN;
        assert!(rates.validate().is_err());
        rates.input_usd_per_million = -1.0;
        assert!(rates.validate().is_err());
    }
}
