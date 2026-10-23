//! Direct-bus admission. Bridge framing is not a server-side safety boundary.
use crate::task::AbortOnDrop;
use serde::Serialize;
use shelllist_daemon_core::{ApiError, ApiIdentity, ExecutionMode, MethodPolicy, error};
use std::{
    collections::HashMap,
    future::Future,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Default)]
struct Usage {
    owners: HashMap<String, usize>,
    methods: HashMap<String, usize>,
}

pub struct ServiceBoundary {
    api: ApiIdentity,
    calls: Arc<Semaphore>,
    controls: Arc<Semaphore>,
    request_bytes: usize,
    result_bytes: usize,
    usage: Arc<Mutex<Usage>>,
}

fn reject(api: ApiIdentity, code: &str, message: &str) -> String {
    error(api, ApiError::new(code, message).with_retryable(false)).to_string()
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
            calls: Arc::new(Semaphore::new(calls)),
            controls: Arc::new(Semaphore::new(controls)),
            request_bytes,
            result_bytes,
            usage: Arc::default(),
        }
    }

    fn reject(&self, code: &str, message: &str) -> String {
        reject(self.api, code, message)
    }

    fn enter(&self, owner: Option<&str>, lane: &Arc<Semaphore>) -> Result<Admitted, String> {
        if owner.is_none_or(str::is_empty) {
            return Err(self.reject("owner-required", "A D-Bus caller identity is required"));
        }
        let permit = lane.clone().try_acquire_owned().map_err(|_| {
            self.reject(
                "rejected-before-dispatch",
                "Service capacity exceeded; request was not dispatched",
            )
        })?;
        Ok(Admitted {
            api: self.api,
            result_bytes: self.result_bytes,
            policy: None,
            _quota: None,
            _permit: permit,
        })
    }

    pub fn call(
        &self,
        owner: Option<&str>,
        method: &str,
        params: &str,
    ) -> Result<Admitted, String> {
        self.raw(owner, method, params.len())
    }

    /// The policy comes from the server declaration, never caller JSON or route metadata.
    pub fn call_policy(
        &self,
        owner: Option<&str>,
        method: &str,
        params: &str,
        policy: MethodPolicy,
    ) -> Result<Admitted, String> {
        if params.len() > policy.request_bytes {
            return Err(self.reject(
                "request-too-large",
                "Request exceeds method limits; request was not dispatched",
            ));
        }
        let mut admitted = self.call(owner, method, params)?;
        let owner = owner
            .ok_or_else(|| self.reject("owner-required", "A D-Bus caller identity is required"))?;
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        if usage.owners.get(owner).copied().unwrap_or(0) >= policy.max_owner_in_flight
            || usage.methods.get(method).copied().unwrap_or(0) >= policy.max_in_flight
        {
            return Err(self.reject(
                "rejected-before-dispatch",
                "Method or owner capacity exceeded; request was not dispatched",
            ));
        }
        *usage.owners.entry(owner.into()).or_default() += 1;
        *usage.methods.entry(method.into()).or_default() += 1;
        admitted._quota = Some(Quota {
            usage: self.usage.clone(),
            owner: owner.into(),
            method: method.into(),
        });
        admitted.result_bytes = self.result_bytes.min(policy.result_bytes);
        admitted.policy = Some(policy);
        Ok(admitted)
    }

    /// Also covers binary clipboard publication, before domain decoding/copying.
    pub fn raw(&self, owner: Option<&str>, name: &str, bytes: usize) -> Result<Admitted, String> {
        if !(1..=256).contains(&name.len()) || bytes > self.request_bytes {
            return Err(self.reject(
                "request-too-large",
                "Request exceeds service limits; request was not dispatched",
            ));
        }
        self.enter(owner, &self.calls)
    }

    pub fn subscribe(&self, owner: Option<&str>, streams: &[String]) -> Result<Admitted, String> {
        if streams.len() > 64 || streams.iter().any(|s| !(1..=256).contains(&s.len())) {
            return Err(self.reject(
                "request-too-large",
                "Subscription exceeds service limits; request was not dispatched",
            ));
        }
        self.enter(owner, &self.calls)
    }

    pub fn cancel(&self, owner: Option<&str>, id: &str) -> Result<Admitted, String> {
        if !(1..=256).contains(&id.len()) {
            return Err(self.reject(
                "validation-error",
                "Invalid cancellation identifier; request was not dispatched",
            ));
        }
        self.enter(owner, &self.controls)
    }
}

struct Quota {
    usage: Arc<Mutex<Usage>>,
    owner: String,
    method: String,
}
impl Drop for Quota {
    fn drop(&mut self) {
        fn release(map: &mut HashMap<String, usize>, key: &str) {
            if let Some(count) = map.get_mut(key) {
                *count -= 1;
                if *count == 0 {
                    map.remove(key);
                }
            }
        }
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        release(&mut usage.owners, &self.owner);
        release(&mut usage.methods, &self.method);
    }
}

pub struct Admitted {
    api: ApiIdentity,
    result_bytes: usize,
    policy: Option<MethodPolicy>,
    _quota: Option<Quota>,
    _permit: OwnedSemaphorePermit,
}

impl Admitted {
    fn unknown(&self) -> String {
        reject(
            self.api,
            "outcome-unknown",
            "Result could not be delivered within service limits; execution may have completed; do not replay mutations",
        )
    }

    /// Reads may abandon their local future. Effects and operation acknowledgements
    /// retain their permit until execution finishes, even if the caller goes away.
    /// Domain Cancel still means cancellation intent, never rollback of effects.
    pub async fn run<T, F>(self, work: F) -> String
    where
        T: Serialize + Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let Some(policy) = self.policy else {
            return reject(
                self.api,
                "rejected-before-dispatch",
                "Execution requires a declared method policy",
            );
        };
        let api = self.api;
        let mut task = tokio::spawn(async move { self.encode(&work.await) });
        let _cancel =
            (policy.execution == ExecutionMode::Read).then(|| AbortOnDrop(task.abort_handle()));
        match tokio::time::timeout(
            Duration::from_millis(policy.ack_budget_ms.into()),
            &mut task,
        )
        .await
        {
            Ok(Ok(response)) => response,
            _ => reject(
                api,
                "outcome-unknown",
                "Acknowledgement unavailable; execution may have completed; do not replay mutations",
            ),
        }
    }

    pub fn encode<T: Serialize>(&self, value: &T) -> String {
        let mut writer = BoundedWriter {
            bytes: Vec::new(),
            remaining: self.result_bytes,
        };
        if serde_json::to_writer(&mut writer, value).is_err() {
            return self.unknown();
        }
        String::from_utf8(writer.bytes).unwrap_or_else(|_| self.unknown())
    }

    pub fn encoded(&self, response: String) -> String {
        if response.len() > self.result_bytes {
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
