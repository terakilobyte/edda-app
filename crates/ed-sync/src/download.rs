//! Resumable download policy, independent of any HTTP library.
//!
//! [`DownloadState`] is the pure state machine: which byte to ask for next,
//! whether a response may be appended to what is already on disk, how much
//! is allowed to arrive, and when to give up after interruptions. Adapters
//! (an async streaming download to a file, a blocking `Read` feeding a
//! decoder) own the transport and ask the state machine what to do.
//!
//! [`ResumableReader`] is the blocking adapter shape: it turns a "fetch from
//! offset" closure into one continuous [`Read`] that reconnects on early EOF
//! or read errors.

use std::{io::Read, time::Duration};

use anyhow::{ensure, Context, Result};

/// How the server answered a request that may have carried a `Range` header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseKind {
    /// `200 OK`: the body starts at byte 0 regardless of what was asked.
    Full,
    /// `206 Partial Content`: the body starts at the requested offset.
    Partial,
}

/// What an adapter must do with a response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// The body continues from the current offset; append it.
    Append,
    /// The server ignored the range; the body starts over at byte 0 and the
    /// adapter must discard what it had (or fail if it cannot).
    Restart,
}

/// What to do with a partial file found on disk before the first request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeAction {
    /// The existing bytes already cover the expected length; verify them.
    AlreadyComplete,
    /// The existing bytes cannot be trusted (longer than expected); delete
    /// them and start from zero.
    Restart,
    /// Ask the server for bytes from `offset` onwards.
    Resume { offset: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadPolicy {
    /// Manifest length when known. Enforced as an exact upper bound.
    pub expected_bytes: Option<u64>,
    /// Interruptions tolerated before the download is abandoned.
    pub max_retries: u32,
    /// Pause before reconnecting after an interruption.
    pub retry_delay: Duration,
}

impl DownloadPolicy {
    /// An artifact whose exact size the manifest states.
    pub fn exact(expected_bytes: u64) -> Self {
        DownloadPolicy {
            expected_bytes: Some(expected_bytes),
            max_retries: 5,
            retry_delay: Duration::from_secs(2),
        }
    }

    /// A stream whose length is learned from the first response, if at all.
    pub fn open_ended() -> Self {
        DownloadPolicy {
            expected_bytes: None,
            max_retries: 50,
            retry_delay: Duration::from_secs(2),
        }
    }

    pub fn with_retries(mut self, max_retries: u32, retry_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.retry_delay = retry_delay;
        self
    }
}

#[derive(Clone, Debug)]
pub struct DownloadState {
    policy: DownloadPolicy,
    offset: u64,
    retries: u32,
}

impl DownloadState {
    pub fn new(policy: DownloadPolicy) -> Self {
        DownloadState {
            policy,
            offset: 0,
            retries: 0,
        }
    }

    /// Bytes received (or trusted from disk) so far.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Total length, from the manifest or learned from a response.
    pub fn expected_bytes(&self) -> Option<u64> {
        self.policy.expected_bytes
    }

    pub fn retries(&self) -> u32 {
        self.retries
    }

    /// Decide what to do with `existing` bytes already on disk.
    pub fn resume_from(&mut self, existing: u64) -> ResumeAction {
        match self.policy.expected_bytes {
            Some(expected) if existing > expected => {
                self.offset = 0;
                ResumeAction::Restart
            }
            Some(expected) if existing == expected => {
                self.offset = existing;
                ResumeAction::AlreadyComplete
            }
            _ => {
                self.offset = existing;
                ResumeAction::Resume { offset: existing }
            }
        }
    }

    /// `Range` header value for the next request, or `None` for a plain GET.
    pub fn range_header(&self) -> Option<String> {
        (self.offset > 0).then(|| format!("bytes={}-", self.offset))
    }

    /// Reconcile a response with what was asked. Learns the total length
    /// from `Content-Length` when the manifest did not state it.
    pub fn accept_response(
        &mut self,
        kind: ResponseKind,
        content_length: Option<u64>,
    ) -> Result<Disposition> {
        let disposition = match kind {
            ResponseKind::Full if self.offset > 0 => {
                self.offset = 0;
                Disposition::Restart
            }
            ResponseKind::Partial | ResponseKind::Full => Disposition::Append,
        };
        if let (None, Some(length)) = (self.policy.expected_bytes, content_length) {
            let total = match kind {
                ResponseKind::Partial => self
                    .offset
                    .checked_add(length)
                    .context("artifact size overflow")?,
                ResponseKind::Full => length,
            };
            self.policy.expected_bytes = Some(total);
        }
        if let (Some(expected), Some(length)) = (self.policy.expected_bytes, content_length) {
            let announced = match kind {
                ResponseKind::Partial => self
                    .offset
                    .checked_add(length)
                    .context("artifact size overflow")?,
                ResponseKind::Full => length,
            };
            ensure!(
                announced == expected,
                "server announced {announced} bytes; expected {expected}"
            );
        }
        Ok(disposition)
    }

    /// Account for `len` received bytes; fails if the artifact overruns the
    /// expected length.
    pub fn record_chunk(&mut self, len: usize) -> Result<()> {
        self.offset = self
            .offset
            .checked_add(u64::try_from(len)?)
            .context("artifact size overflow")?;
        if let Some(expected) = self.policy.expected_bytes {
            ensure!(
                self.offset <= expected,
                "artifact is larger than its manifest length ({} > {expected})",
                self.offset
            );
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.policy
            .expected_bytes
            .is_some_and(|expected| self.offset >= expected)
    }

    /// Called when the body ended early or a read failed. Returns how long
    /// to wait before reconnecting, or an error once retries are exhausted.
    pub fn interrupted(&mut self) -> Result<Duration> {
        self.retries += 1;
        ensure!(
            self.retries <= self.policy.max_retries,
            "download interrupted {} times at byte {}; giving up",
            self.retries,
            self.offset
        );
        Ok(self.policy.retry_delay)
    }

    /// Check that a body that ended normally delivered every byte.
    pub fn finish(&self) -> Result<()> {
        if let Some(expected) = self.policy.expected_bytes {
            ensure!(
                self.offset == expected,
                "artifact ended at {} bytes; expected {expected}",
                self.offset
            );
        }
        Ok(())
    }
}

/// One response from a range-capable fetch.
pub struct RangeResponse<R> {
    pub kind: ResponseKind,
    pub content_length: Option<u64>,
    pub body: R,
}

/// Fetch the artifact from `offset`. `range` is the `Range` header value to
/// send when it is `Some`.
pub type RangeFetch<R> = Box<dyn FnMut(u64, Option<String>) -> Result<RangeResponse<R>> + Send>;

/// A body that survives dropped connections: on early EOF or a read error
/// it reconnects from the current offset and carries on, so a decoder above
/// it sees one continuous stream. Because consumed bytes cannot be taken
/// back, a server that ignores the range after bytes were consumed is an
/// error rather than a restart.
pub struct ResumableReader<R: Read> {
    state: DownloadState,
    fetch: RangeFetch<R>,
    body: Option<R>,
    cancelled: Box<dyn Fn() -> bool + Send>,
}

impl<R: Read> ResumableReader<R> {
    /// Connect immediately so the total length is known before reading.
    pub fn open(
        policy: DownloadPolicy,
        fetch: RangeFetch<R>,
        cancelled: Box<dyn Fn() -> bool + Send>,
    ) -> Result<Self> {
        let mut reader = ResumableReader {
            state: DownloadState::new(policy),
            fetch,
            body: None,
            cancelled,
        };
        reader.connect()?;
        Ok(reader)
    }

    pub fn total(&self) -> Option<u64> {
        self.state.expected_bytes()
    }

    pub fn offset(&self) -> u64 {
        self.state.offset()
    }

    fn connect(&mut self) -> Result<()> {
        let offset = self.state.offset();
        let response = (self.fetch)(offset, self.state.range_header())?;
        match self
            .state
            .accept_response(response.kind, response.content_length)?
        {
            Disposition::Append => {}
            Disposition::Restart => {
                anyhow::bail!(
                    "server did not honour a Range request; cannot resume at byte {offset}"
                )
            }
        }
        self.body = Some(response.body);
        Ok(())
    }

    fn reconnect_after(&mut self) -> std::io::Result<()> {
        self.body = None;
        let delay = self.state.interrupted().map_err(std::io::Error::other)?;
        std::thread::sleep(delay);
        Ok(())
    }
}

impl<R: Read> Read for ResumableReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if (self.cancelled)() {
                return Err(std::io::Error::other("cancelled"));
            }
            if self.state.is_complete() {
                return Ok(0);
            }
            let Some(body) = self.body.as_mut() else {
                self.connect().map_err(std::io::Error::other)?;
                continue;
            };
            match body.read(buf) {
                Ok(0) if self.state.expected_bytes().is_none() => return Ok(0),
                Ok(0) => self.reconnect_after()?,
                Ok(n) => {
                    self.state.record_chunk(n).map_err(std::io::Error::other)?;
                    return Ok(n);
                }
                Err(_) => self.reconnect_after()?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn resume_decisions_follow_the_manifest_length() {
        let mut state = DownloadState::new(DownloadPolicy::exact(100));
        assert_eq!(state.resume_from(0), ResumeAction::Resume { offset: 0 });
        assert_eq!(state.range_header(), None);
        assert_eq!(state.resume_from(40), ResumeAction::Resume { offset: 40 });
        assert_eq!(state.range_header().as_deref(), Some("bytes=40-"));
        assert_eq!(state.resume_from(100), ResumeAction::AlreadyComplete);
        assert!(state.is_complete());
        assert_eq!(state.resume_from(101), ResumeAction::Restart);
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn a_full_response_to_a_range_request_restarts_from_zero() {
        let mut state = DownloadState::new(DownloadPolicy::exact(10));
        state.resume_from(4);
        assert_eq!(
            state.accept_response(ResponseKind::Full, Some(10)).unwrap(),
            Disposition::Restart
        );
        assert_eq!(state.offset(), 0);
        let mut state = DownloadState::new(DownloadPolicy::exact(10));
        state.resume_from(4);
        assert_eq!(
            state
                .accept_response(ResponseKind::Partial, Some(6))
                .unwrap(),
            Disposition::Append
        );
        assert!(state
            .accept_response(ResponseKind::Partial, Some(7))
            .is_err());
    }

    #[test]
    fn chunks_are_bounded_and_completion_is_exact() {
        let mut state = DownloadState::new(DownloadPolicy::exact(5));
        state.record_chunk(3).unwrap();
        assert!(state.finish().is_err());
        state.record_chunk(2).unwrap();
        state.finish().unwrap();
        assert!(state.record_chunk(1).is_err());
    }

    #[test]
    fn open_ended_streams_learn_their_length_and_bound_retries() {
        let mut state =
            DownloadState::new(DownloadPolicy::open_ended().with_retries(2, Duration::ZERO));
        assert_eq!(state.expected_bytes(), None);
        state.accept_response(ResponseKind::Full, Some(9)).unwrap();
        assert_eq!(state.expected_bytes(), Some(9));
        state.record_chunk(4).unwrap();
        state
            .accept_response(ResponseKind::Partial, Some(5))
            .unwrap();
        state.interrupted().unwrap();
        state.interrupted().unwrap();
        assert!(state.interrupted().is_err());
    }

    type FakeBody = std::io::Cursor<Vec<u8>>;
    type SeenRanges = Arc<Mutex<Vec<Option<String>>>>;

    /// A fake server that closes the connection after `drop_after` bytes on
    /// every connection, honouring ranges.
    fn flaky_server(
        payload: &'static [u8],
        drop_after: usize,
    ) -> (RangeFetch<FakeBody>, SeenRanges) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let fetch: RangeFetch<FakeBody> = Box::new(move |offset, range| {
            seen.lock().unwrap().push(range);
            let start = usize::try_from(offset).unwrap();
            let end = (start + drop_after).min(payload.len());
            Ok(RangeResponse {
                kind: if offset > 0 {
                    ResponseKind::Partial
                } else {
                    ResponseKind::Full
                },
                content_length: Some((payload.len() - start) as u64),
                body: std::io::Cursor::new(payload[start..end].to_vec()),
            })
        });
        (fetch, requests)
    }

    #[test]
    fn resumable_reader_reconnects_with_ranges_until_the_stream_is_complete() {
        const PAYLOAD: &[u8] = b"0123456789abcdefghij";
        let (fetch, requests) = flaky_server(PAYLOAD, 7);
        let mut reader = ResumableReader::open(
            DownloadPolicy::open_ended().with_retries(10, Duration::ZERO),
            fetch,
            Box::new(|| false),
        )
        .unwrap();
        assert_eq!(reader.total(), Some(20));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, PAYLOAD);
        assert_eq!(
            *requests.lock().unwrap(),
            vec![None, Some("bytes=7-".into()), Some("bytes=14-".into())]
        );
    }

    #[test]
    fn resumable_reader_gives_up_after_the_retry_budget_and_honours_cancel() {
        const PAYLOAD: &[u8] = b"0123456789";
        let (fetch, _) = flaky_server(PAYLOAD, 1);
        let mut reader = ResumableReader::open(
            DownloadPolicy::open_ended().with_retries(2, Duration::ZERO),
            fetch,
            Box::new(|| false),
        )
        .unwrap();
        let mut out = Vec::new();
        assert!(reader.read_to_end(&mut out).is_err());
        assert!(out.len() < PAYLOAD.len());

        let (fetch, _) = flaky_server(PAYLOAD, 10);
        let mut reader =
            ResumableReader::open(DownloadPolicy::open_ended(), fetch, Box::new(|| true)).unwrap();
        assert!(reader.read(&mut [0; 4]).is_err());
    }

    #[test]
    fn resumable_reader_refuses_a_server_that_ignores_ranges_mid_stream() {
        const PAYLOAD: &[u8] = b"0123456789";
        let fetch: RangeFetch<std::io::Cursor<Vec<u8>>> = Box::new(move |_offset, _range| {
            Ok(RangeResponse {
                kind: ResponseKind::Full,
                content_length: Some(10),
                body: std::io::Cursor::new(PAYLOAD[..3].to_vec()),
            })
        });
        let mut reader = ResumableReader::open(
            DownloadPolicy::open_ended().with_retries(3, Duration::ZERO),
            fetch,
            Box::new(|| false),
        )
        .unwrap();
        let mut out = Vec::new();
        let error = reader.read_to_end(&mut out).unwrap_err().to_string();
        assert!(error.contains("did not honour a Range request"), "{error}");
        assert_eq!(out, b"012");
    }
}
