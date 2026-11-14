//! Shared bounds and validation for the local daemon protocol.

use serde_json::Value as Json;
use std::io::BufRead;

pub const VERSION: u64 = 2;
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
pub const EVENT_PAGE_BYTES: usize = 4 * 1024 * 1024;
pub const EVENT_PAGE_ROWS: i64 = 256;
pub const MAX_PAGE_ROWS: i64 = 1000;

/// Bound allocation before parsing an untrusted socket frame.
pub fn read_frame(reader: &mut impl BufRead) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|e| format!("daemon read: {e}"))?;
        if available.is_empty() {
            return Err("daemon closed before a complete frame".into());
        }
        let end = available.iter().position(|b| *b == b'\n');
        let count = end.map_or(available.len(), |i| i + 1);
        if bytes.len().saturating_add(count) > MAX_FRAME_BYTES {
            return Err("daemon frame exceeds 32 MiB".into());
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            return String::from_utf8(bytes).map_err(|e| format!("daemon frame UTF-8: {e}"));
        }
    }
}

/// Stop serialization at the wire bound instead of allocating an arbitrary reply string.
pub fn encode_frame(value: &Json) -> Result<Vec<u8>, String> {
    struct Frame(Vec<u8>);
    impl std::io::Write for Frame {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_FRAME_BYTES.saturating_sub(self.0.len() + 1) {
                return Err(std::io::Error::other("daemon frame exceeds 32 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut frame = Frame(Vec::new());
    serde_json::to_writer(&mut frame, value).map_err(|e| e.to_string())?;
    frame.0.push(b'\n');
    Ok(frame.0)
}

/// Validate the whole page before acknowledging any event. The head is not a delivery cursor.
pub fn event_cursor(page: &Json, since: i64) -> Result<(i64, bool), String> {
    if page["resync_required"].as_bool().ok_or("events.resync_required missing")? {
        if page["resync_reason"].as_str() == Some("oversized_event") {
            return Err("an event exceeds the 4 MiB page budget; take a fresh checkpoint or export the session (run usage may be incomplete)".into());
        }
        return Err("event history was reclaimed; take a fresh checkpoint (run usage may be incomplete)".into());
    }
    let events = page["events"].as_array().ok_or("events.events missing")?;
    let mut cursor = since;
    for event in events {
        let sequence = event["sequence"].as_i64().ok_or("event.sequence missing")?;
        if cursor.checked_add(1) != Some(sequence) {
            return Err("event page is not contiguous; cursor unchanged".into());
        }
        cursor = sequence;
    }
    let watermark = page["watermark"].as_i64().ok_or("events.watermark missing")?;
    let head = page["head_watermark"].as_i64().ok_or("events.head_watermark missing")?;
    let more = page["has_more"].as_bool().ok_or("events.has_more missing")?;
    if watermark != cursor || head < cursor || (more && events.is_empty()) {
        return Err("invalid event page cursor; cursor unchanged".into());
    }
    Ok((cursor, more))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_and_event_pages_fail_without_acknowledging_partial_data() {
        assert!(read_frame(&mut &b"{}"[..]).is_err());
        assert_eq!(read_frame(&mut &b"{}\n"[..]).unwrap(), "{}\n");
        assert_eq!(encode_frame(&json!({})).unwrap(), b"{}\n");
        assert!(encode_frame(&json!("x".repeat(MAX_FRAME_BYTES))).is_err());
        assert!(read_frame(&mut vec![b'x'; MAX_FRAME_BYTES + 1].as_slice()).is_err());
        let mut page = json!({"events":[{"sequence":4}], "watermark":4,
            "head_watermark":10, "has_more":true, "resync_required":false});
        assert_eq!(event_cursor(&page, 3).unwrap(), (4, true));
        page["watermark"] = json!(10);
        assert!(event_cursor(&page, 3).is_err());
        page["watermark"] = json!(4);
        assert!(event_cursor(&page, 2).is_err());
        page["resync_required"] = json!(true);
        assert!(event_cursor(&page, 3).is_err());
    }
}
