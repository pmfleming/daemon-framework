//! Direct-bus admission. Bridge framing is not a server-side safety boundary.
use serde::Serialize;
use shelllist_daemon_core::{ApiError, ApiIdentity, error};
use std::io::{self, Write};
use tokio::sync::{Semaphore, SemaphorePermit};

pub struct ServiceBoundary {
    api: ApiIdentity,
    calls: Semaphore,
    controls: Semaphore,
    request_bytes: usize,
    result_bytes: usize,
}

impl ServiceBoundary {
    pub fn new(api: ApiIdentity) -> Self {
        Self::with_limits(api, 64, 16, 1024 * 1024, 16 * 1024 * 1024)
    }

    pub fn with_limits(
        api: ApiIdentity,
        calls: usize,
        controls: usize,
        request_bytes: usize,
        result_bytes: usize,
    ) -> Self {
        Self {
            api,
            calls: Semaphore::new(calls),
            controls: Semaphore::new(controls),
            request_bytes,
            result_bytes,
        }
    }

    fn reject(&self, code: &str, message: &str) -> String {
        error(self.api, ApiError::new(code, message).with_retryable(false)).to_string()
    }

    fn enter(&self, owner: Option<&str>, controls: bool) -> Result<Admitted<'_>, String> {
        if owner.is_none_or(str::is_empty) {
            return Err(self.reject("owner-required", "A D-Bus caller identity is required"));
        }
        let lane = if controls {
            &self.controls
        } else {
            &self.calls
        };
        let permit = lane.try_acquire().map_err(|_| {
            self.reject(
                "rejected-before-dispatch",
                "Service capacity exceeded; request was not dispatched",
            )
        })?;
        Ok(Admitted {
            boundary: self,
            _permit: permit,
        })
    }

    pub fn call(
        &self,
        owner: Option<&str>,
        method: &str,
        params: &str,
    ) -> Result<Admitted<'_>, String> {
        self.raw(owner, method, params.len())
    }

    /// Also covers binary clipboard publication, before domain decoding/copying.
    pub fn raw(
        &self,
        owner: Option<&str>,
        name: &str,
        bytes: usize,
    ) -> Result<Admitted<'_>, String> {
        if name.is_empty() || name.len() > 256 || bytes > self.request_bytes {
            return Err(self.reject(
                "request-too-large",
                "Request exceeds service limits; request was not dispatched",
            ));
        }
        self.enter(owner, false)
    }

    pub fn subscribe(
        &self,
        owner: Option<&str>,
        streams: &[String],
    ) -> Result<Admitted<'_>, String> {
        if streams.len() > 64 || streams.iter().any(|s| s.is_empty() || s.len() > 256) {
            return Err(self.reject(
                "request-too-large",
                "Subscription exceeds service limits; request was not dispatched",
            ));
        }
        self.enter(owner, false)
    }

    pub fn cancel(&self, owner: Option<&str>, id: &str) -> Result<Admitted<'_>, String> {
        if id.is_empty() || id.len() > 256 {
            return Err(self.reject(
                "validation-error",
                "Invalid cancellation identifier; request was not dispatched",
            ));
        }
        self.enter(owner, true)
    }
}

pub struct Admitted<'a> {
    boundary: &'a ServiceBoundary,
    _permit: SemaphorePermit<'a>,
}

impl Admitted<'_> {
    fn unknown(&self) -> String {
        self.boundary.reject("outcome-unknown", "Result could not be delivered within service limits; execution may have completed; do not replay mutations")
    }

    /// Bound serialization itself, not just the resulting string. This does not
    /// undo a handler's side effects or claim a rejected-before-dispatch outcome.
    pub fn encode<T: Serialize>(&self, value: &T) -> String {
        let mut writer = BoundedWriter {
            bytes: Vec::new(),
            remaining: self.boundary.result_bytes,
        };
        if serde_json::to_writer(&mut writer, value).is_err() {
            return self.unknown();
        }
        String::from_utf8(writer.bytes).expect("JSON serializer emits UTF-8")
    }

    /// For dispatchers that already own a serialized envelope.
    pub fn encoded(&self, response: String) -> String {
        if response.len() > self.boundary.result_bytes {
            self.unknown()
        } else {
            response
        }
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    remaining: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(io::Error::other("service result limit"));
        }
        self.bytes.extend_from_slice(buf);
        self.remaining -= buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
