//! Retry-with-backoff decorator for any [`LlmService`].
//!
//! Wraps a provider and retries **transient** failures (connect errors, timeouts,
//! HTTP 429/5xx) with exponential backoff + jitter. **Permanent** errors — bad key,
//! malformed request (4xx other than 429), unparseable responses — are returned
//! immediately: retrying them only wastes time and tokens.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use super::{LlmResponse, LlmService, Message, ToolSchema};

/// How to retry: attempt count, base delay, cap.
/// The nth retry waits `base_delay * 2^n` (capped at `max_delay`) plus up to
/// +50% random jitter so concurrent clients don't retry in lockstep.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    fn delay_for(&self, attempt: u32) -> Duration {
        let exp = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt))
            .min(self.max_delay);
        let jitter_cap = (exp.as_millis() as u64) / 2;
        let jitter = if jitter_cap == 0 { 0 } else { rand::random::<u64>() % jitter_cap };
        exp + Duration::from_millis(jitter)
    }
}

/// Marker error for failures known to be transient. Non-HTTP providers (and test
/// doubles) can put this in their error chain to opt into retries:
/// `Err(anyhow::Error::new(Transient).context("..."))`.
#[derive(Debug)]
pub struct Transient;

impl std::fmt::Display for Transient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transient error")
    }
}

impl std::error::Error for Transient {}

/// Decide whether an error is worth retrying by walking its cause chain.
///
/// - [`Transient`] marker anywhere in the chain → retry.
/// - `reqwest` timeout / connection failure → retry.
/// - HTTP status 429 or 500/502/503/504 → retry (rate limit / server hiccup).
/// - Any other HTTP status (400, 401, 403, …) → do NOT retry (our request or key
///   is wrong; resending it changes nothing and costs money).
/// - Everything else (JSON parse errors, logic errors) → do NOT retry.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.downcast_ref::<Transient>().is_some() {
            return true;
        }
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            if re.is_timeout() || re.is_connect() {
                return true;
            }
            if let Some(status) = re.status() {
                return matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504);
            }
            // Request died without a status (connection dropped mid-flight).
            return re.is_request();
        }
    }
    false
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send>>;

/// Decorator that adds retry-with-backoff to any inner [`LlmService`].
///
/// Transparent otherwise: tool-calling (`chat`) is forwarded to the inner
/// provider, so tool-capable adapters keep emitting tool calls.
pub struct RetryLlm {
    inner: Arc<dyn LlmService>,
    policy: RetryPolicy,
}

impl RetryLlm {
    pub fn new(inner: Arc<dyn LlmService>) -> Self {
        Self { inner, policy: RetryPolicy::default() }
    }

    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    async fn with_retries<T>(&self, mut call: impl FnMut() -> BoxFuture<T> + Send) -> Result<T> {
        let mut attempt = 0u32;
        loop {
            match call().await {
                Ok(v) => return Ok(v),
                Err(err) => {
                    if attempt >= self.policy.max_retries || !is_retryable(&err) {
                        return Err(err);
                    }
                    let delay = self.policy.delay_for(attempt);
                    tracing::warn!(attempt = attempt + 1, ?delay, error = %err, "transient LLM error; retrying");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
}

#[async_trait]
impl LlmService for RetryLlm {
    async fn submit_prompt(&self, messages: Vec<Message>) -> Result<String> {
        self.with_retries(|| {
            let inner = self.inner.clone();
            let messages = messages.clone();
            Box::pin(async move { inner.submit_prompt(messages).await }) as BoxFuture<String>
        })
        .await
    }

    async fn chat(&self, messages: Vec<Message>, tools: &[ToolSchema]) -> Result<LlmResponse> {
        self.with_retries(|| {
            let inner = self.inner.clone();
            let messages = messages.clone();
            let tools = tools.to_vec();
            Box::pin(async move { inner.chat(messages, &tools).await }) as BoxFuture<LlmResponse>
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Fails `transient_failures` times (or always, when `permanent`), counting calls.
    struct FlakyLlm {
        transient_failures: usize,
        permanent: bool,
        calls: AtomicUsize,
    }

    impl FlakyLlm {
        fn transient(n: usize) -> Self {
            Self { transient_failures: n, permanent: false, calls: AtomicUsize::new(0) }
        }
        fn permanent() -> Self {
            Self { transient_failures: 0, permanent: true, calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait]
    impl LlmService for FlakyLlm {
        async fn submit_prompt(&self, _messages: Vec<Message>) -> Result<String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.permanent {
                anyhow::bail!("401 unauthorized: bad API key");
            }
            if n < self.transient_failures {
                return Err(anyhow::Error::new(Transient).context("simulated 503"));
            }
            Ok("ok".into())
        }
    }

    fn fast() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(4),
        }
    }

    #[tokio::test]
    async fn succeeds_after_transient_failures() {
        let inner = Arc::new(FlakyLlm::transient(2));
        let llm = RetryLlm::new(inner.clone()).with_policy(fast());
        let out = llm.submit_prompt(vec![Message::user("q")]).await.unwrap();
        assert_eq!(out, "ok");
        assert_eq!(inner.calls.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let inner = Arc::new(FlakyLlm::transient(100));
        let llm = RetryLlm::new(inner.clone()).with_policy(fast());
        let err = llm.submit_prompt(vec![Message::user("q")]).await.unwrap_err();
        assert!(is_retryable(&err)); // it was transient — we just ran out of attempts
        assert_eq!(inner.calls.load(Ordering::SeqCst), 4); // 1 initial + 3 retries
    }

    #[tokio::test]
    async fn permanent_errors_are_not_retried() {
        let inner = Arc::new(FlakyLlm::permanent());
        let llm = RetryLlm::new(inner.clone()).with_policy(fast());
        let err = llm.submit_prompt(vec![Message::user("q")]).await.unwrap_err();
        assert!(err.to_string().contains("401"));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1); // no retry on 4xx-style errors
    }

    #[tokio::test]
    async fn chat_path_retries_too() {
        let inner = Arc::new(FlakyLlm::transient(1));
        let llm = RetryLlm::new(inner.clone()).with_policy(fast());
        let out = llm.chat(vec![Message::user("q")], &[]).await.unwrap();
        assert_eq!(out.text.as_deref(), Some("ok"));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn classification_walks_the_chain() {
        let transient = anyhow::Error::new(Transient).context("outer context");
        assert!(is_retryable(&transient));
        let plain = anyhow::anyhow!("some logic error");
        assert!(!is_retryable(&plain));
    }

    #[test]
    fn backoff_is_capped() {
        let p = fast();
        // Even at a huge attempt number the delay stays within max + 50% jitter.
        let d = p.delay_for(30);
        assert!(d <= p.max_delay + p.max_delay / 2);
    }
}
