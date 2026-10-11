//! Native image mapping for the user and tool content carried by the neutral chat shape.
use super::ProviderError;
use serde_json::{json, Value as Json};

pub fn parts(content: &Json, protocol: &str) -> Result<Vec<Json>, ProviderError> {
    let text = |value: &str| {
        if protocol == "responses" {
            json!({"type":"input_text","text":value})
        } else {
            json!({"type":"text","text":value})
        }
    };
    if let Some(value) = content.as_str() {
        return Ok(vec![text(value)]);
    }
    if content.is_null() {
        return Ok(vec![text("")]);
    }
    let items =
        content.as_array().ok_or_else(|| ProviderError::permanent("content must be text or a content-parts array"))?;
    items
        .iter()
        .map(|item| match item["type"].as_str() {
            Some("text") => {
                item["text"].as_str().map(text).ok_or_else(|| ProviderError::permanent("text content part has no text"))
            }
            Some("image_url") => {
                let url = item["image_url"]["url"]
                    .as_str()
                    .ok_or_else(|| ProviderError::permanent("image part has no URL"))?;
                let (header, data) = url
                    .split_once(";base64,")
                    .ok_or_else(|| ProviderError::permanent("only inline image snapshots are supported"))?;
                let media_type = header
                    .strip_prefix("data:")
                    .ok_or_else(|| ProviderError::permanent("image snapshot must use data encoding"))?;
                if !matches!(media_type, "image/png" | "image/jpeg" | "image/gif" | "image/webp") || data.is_empty() {
                    return Err(ProviderError::permanent("unsupported image snapshot type or empty bytes"));
                }
                Ok(if protocol == "responses" {
                    json!({"type":"input_image","image_url":url})
                } else {
                    json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}})
                })
            }
            _ => Err(ProviderError::permanent("unsupported content part; refusing to discard it")),
        })
        .collect()
}
