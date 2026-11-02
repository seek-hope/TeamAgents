//! Bounded byte ingestion shared by model adapters. Decode complete SSE lines,
//! never HTTP chunks: a transport chunk may end inside any UTF-8 character.
use super::{Cancel, ProviderError, SseEnd, StreamTiming};
use futures_util::{Stream, StreamExt};
use std::ops::ControlFlow;
use std::time::Duration;

pub(super) const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub(super) async fn read_body(response: reqwest::Response, cancel: &Cancel) -> Result<Vec<u8>, ProviderError> {
    if response.content_length().is_some_and(|size| size > MAX_RESPONSE as u64) {
        return Err(ProviderError::permanent("model response too large"));
    }
    collect_body(response.bytes_stream(), cancel, MAX_RESPONSE).await
}

async fn collect_body<S, B>(mut stream: S, cancel: &Cancel, limit: usize) -> Result<Vec<u8>, ProviderError>
where
    S: Stream<Item = Result<B, reqwest::Error>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut body = Vec::new();
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
            next = stream.next() => next,
        };
        let Some(next) = next else { return Ok(body) };
        let chunk = next.map_err(|error| ProviderError::transient(format!("model stream: {error}")))?;
        if chunk.as_ref().len() > limit.saturating_sub(body.len()) {
            return Err(ProviderError::permanent("model response too large"));
        }
        body.extend_from_slice(chunk.as_ref());
    }
}

#[derive(Default)]
struct Frames {
    line: Vec<u8>,
    data: String,
    after_cr: bool,
    bytes: usize,
}

impl Frames {
    fn push(
        &mut self,
        bytes: &[u8],
        limit: usize,
        on_frame: &mut impl FnMut(&str) -> Result<ControlFlow<()>, ProviderError>,
        timing: &mut StreamTiming,
    ) -> Result<(ControlFlow<()>, bool), ProviderError> {
        if bytes.len() > limit.saturating_sub(self.bytes) {
            return Err(ProviderError::permanent("model stream response too large"));
        }
        self.bytes += bytes.len();
        let mut delivered = false;
        for &byte in bytes {
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                continue;
            }
            self.after_cr = byte == b'\r';
            if !matches!(byte, b'\r' | b'\n') {
                self.line.push(byte);
                continue;
            }
            let line =
                std::str::from_utf8(&self.line).map_err(|_| ProviderError::permanent("model stream: invalid UTF-8"))?;
            if line.is_empty() && !self.data.is_empty() {
                // SSE joins all data fields with LF and removes the final LF.
                self.data.pop();
                timing.frame();
                delivered = true;
                if let ControlFlow::Break(()) = on_frame(&self.data)? {
                    return Ok((ControlFlow::Break(()), true));
                }
                self.data.clear();
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
                self.data.push('\n');
            } else if line == "data" {
                self.data.push('\n');
            }
            self.line.clear();
        }
        Ok((ControlFlow::Continue(()), delivered))
    }
}

pub(super) async fn pump_sse(
    response: reqwest::Response,
    cancel: &Cancel,
    stall: Duration,
    mut on_frame: impl FnMut(&str) -> Result<ControlFlow<()>, ProviderError> + Send,
    timing: &mut StreamTiming,
) -> Result<SseEnd, ProviderError> {
    let mut stream = response.bytes_stream();
    let mut frames = Frames::default();
    // Only a complete data event renews the stall deadline. Comments and
    // partial frames cannot keep an otherwise stalled model alive.
    let mut deadline = tokio::time::Instant::now() + stall;
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(SseEnd::Cancelled),
            _ = tokio::time::sleep_until(deadline) => return Ok(SseEnd::Transport(format!(
                "model stream stalled: no event for {}s", stall.as_secs()
            ))),
            next = stream.next() => next,
        };
        let Some(next) = next else { return Ok(SseEnd::Closed) };
        let chunk = match next {
            Ok(chunk) => chunk,
            Err(error) => return Ok(SseEnd::Transport(format!("model stream: {error}"))),
        };
        let (flow, delivered) = frames.push(&chunk, MAX_RESPONSE, &mut on_frame, timing)?;
        if let ControlFlow::Break(()) = flow {
            return Ok(SseEnd::Closed);
        }
        if delivered {
            deadline = tokio::time::Instant::now() + stall;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_split_preserves_unicode_multiline_frames_and_delimiters() {
        let payload = "{\"text\":\"caf\u{e9} \u{4e2d} \u{1f600}\",\n\"args\":\"\u{4e2d}.txt\"}";
        for separator in ["\n", "\r\n", "\r"] {
            let wire = format!(": keepalive{separator}data: {}{separator}data: {}{separator}{separator}data: [DONE]{separator}{separator}",
                payload.lines().next().unwrap(), payload.lines().nth(1).unwrap());
            for split in 0..=wire.len() {
                let mut frames = Frames::default();
                let mut timing = StreamTiming::new(std::time::Instant::now());
                let mut seen = Vec::new();
                let mut receive = |data: &str| {
                    seen.push(data.to_owned());
                    Ok(ControlFlow::Continue(()))
                };
                for chunk in [&wire.as_bytes()[..split], &wire.as_bytes()[split..]] {
                    let _ = frames.push(chunk, wire.len(), &mut receive, &mut timing).unwrap();
                }
                assert_eq!(seen, [payload, "[DONE]"], "split {split}, separator {separator:?}");
                assert_eq!(timing.report().frames, 2);
            }
        }
    }

    #[test]
    fn incomplete_invalid_and_oversized_frames_never_become_events() {
        let mut frames = Frames::default();
        let mut timing = StreamTiming::new(std::time::Instant::now());
        let mut receive = |_: &str| panic!("no complete event");
        let _ = frames.push(b"data: incomplete", 32, &mut receive, &mut timing).unwrap();
        assert!(frames.push(&[b'x'; 33], 32, &mut receive, &mut timing).is_err());
        let mut frames = Frames::default();
        assert!(frames.push(b"data: \xff\n\n", 32, &mut receive, &mut timing).is_err());
        assert_eq!(timing.report().frames, 0);
    }

    #[tokio::test]
    async fn bounded_body_rejects_before_eof_and_honors_cancellation() {
        let stream = futures_util::stream::iter([Ok(vec![1u8; 9])]).chain(futures_util::stream::pending());
        let error = tokio::time::timeout(Duration::from_millis(100), collect_body(stream, &Cancel::new(), 8))
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.message.contains("too large"));
        let cancel = Cancel::new();
        cancel.cancel();
        let error = collect_body(futures_util::stream::pending::<Result<Vec<u8>, reqwest::Error>>(), &cancel, 8)
            .await
            .unwrap_err();
        assert_eq!(error.class, super::super::ErrorClass::Interrupted);
        let body = collect_body(futures_util::stream::iter([Ok(vec![1u8; 4]), Ok(vec![2u8; 4])]), &Cancel::new(), 8)
            .await
            .unwrap();
        assert_eq!(body, [1, 1, 1, 1, 2, 2, 2, 2]);
    }
}
