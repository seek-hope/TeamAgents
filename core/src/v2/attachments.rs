//! D-451: immutable user attachment descriptors. Filesystem work belongs to the engine.
use serde::{Deserialize, Serialize};

pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputAttachment {
    pub artifact_id: String,
    pub label: String,
    pub media_type: String,
    pub bytes: usize,
    pub digest: String,
}

impl InputAttachment {
    pub fn validate(&self) -> Result<(), String> {
        if self.artifact_id.is_empty() || !self.artifact_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("invalid attachment artifact identity".into());
        }
        if self.label.is_empty() || self.label.len() > 4096 || self.label.chars().any(char::is_control) {
            return Err(
                "attachment label must be a nonempty path without control characters (at most 4096 bytes)".into()
            );
        }
        let limit = match self.media_type.as_str() {
            "text/plain" => MAX_TEXT_BYTES,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" => MAX_IMAGE_BYTES,
            _ => return Err("unsupported attachment media type".into()),
        };
        if self.bytes > limit {
            return Err(format!("attachment {} exceeds {limit} bytes", self.label));
        }
        if self.digest.len() != 64 || !self.digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("attachment digest must be SHA-256 hex".into());
        }
        Ok(())
    }
}
