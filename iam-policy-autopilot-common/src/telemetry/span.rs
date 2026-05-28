//! Task-local telemetry span for implicit context propagation.
//!
//! Provides a scoped key-value accumulator that any code in the async call chain
//! can write to without explicit plumbing through function signatures.
//!
//! # Usage
//!
//! At the entry point (CLI handler or MCP tool), wrap the pipeline in a scope:
//!
//! ```ignore
//! let (result, span) = with_telemetry_scope(async {
//!     generate_policies(&config).await
//! }).await;
//!
//! // Merge span fields into the telemetry event
//! if let Some(mut event) = telemetry_event {
//!     event.merge_result_span(&span);
//!     telemetry::spawn_telemetry(event);
//! }
//! ```
//!
//! Or use the convenience wrapper that auto-merges:
//!
//! ```ignore
//! let gen_result = run_with_telemetry(
//!     handle_generate_policy(&config),
//!     &mut telemetry_event,
//! ).await;
//! ```
//!
//! Deep in the pipeline (extraction, enrichment, policy generation):
//!
//! ```ignore
//! use iam_policy_autopilot_common::telemetry::span;
//!
//! // Record a string result — silently no-ops if no scope is active
//! span::record_result_str("detected_language", "python");
//!
//! // Record a numeric result — silently no-ops if no scope is active
//! span::record_result_number("num_policies_generated", 3);
//!
//! // Record a value into a result set — deduplicates automatically
//! span::record_result_set("services_used", "s3");
//! span::record_result_set("services_used", "dynamodb");
//! ```

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

tokio::task_local! {
    static TELEMETRY_SPAN: RefCell<TelemetrySpan>;
}

/// Internal accumulator for telemetry fields recorded during pipeline execution.
#[derive(Debug, Default)]
struct TelemetrySpan {
    /// Key-value string fields (e.g., `"detected_language" → "python"`)
    strings: HashMap<String, String>,
    /// Key-value numeric fields (e.g., `"num_policies_generated" → 3`)
    numbers: HashMap<String, usize>,
    /// Key-value set fields for deduplication (e.g., `"services_used" → {"s3", "dynamodb"}`)
    sets: HashMap<String, BTreeSet<String>>,
}

/// Collected telemetry fields from a pipeline execution.
///
/// Returned by [`with_telemetry_scope()`] after the wrapped future completes.
/// Use [`TelemetryEvent::merge_result_span()`] to fold these into a telemetry event.
#[derive(Debug, Default)]
pub struct TelemetrySpanSnapshot {
    /// Recorded string fields
    pub strings: HashMap<String, String>,
    /// Recorded numeric fields
    pub numbers: HashMap<String, usize>,
    /// Recorded set fields (sorted, deduplicated)
    pub sets: HashMap<String, Vec<String>>,
}

/// Run an async pipeline with an active telemetry span.
///
/// Any code in the call chain can call [`record_result_str()`],
/// [`record_result_number()`], or [`record_result_set()`] to accumulate
/// telemetry fields. After the future completes, the accumulated fields
/// are returned as a [`TelemetrySpanSnapshot`].
///
/// If telemetry is disabled, returns an empty snapshot — callers should still
/// wrap the pipeline to ensure inner `record_result_*` calls have a scope
/// and don't panic.
pub async fn with_telemetry_scope<F, T>(future: F) -> (T, TelemetrySpanSnapshot)
where
    F: std::future::Future<Output = T>,
{
    // We use a two-step pattern: run the future inside the scope,
    // and capture the span contents by reading it just before the scope ends.
    let snapshot = std::sync::Arc::new(std::sync::Mutex::new(TelemetrySpanSnapshot::default()));
    let snapshot_inner = std::sync::Arc::clone(&snapshot);

    let result = TELEMETRY_SPAN
        .scope(RefCell::new(TelemetrySpan::default()), async {
            let result = future.await;

            // Snapshot the span before the scope closes
            let _ = TELEMETRY_SPAN.try_with(|span| {
                let span = span.borrow();
                let mut snap = snapshot_inner.lock().expect("snapshot lock poisoned");
                snap.strings.clone_from(&span.strings);
                snap.numbers.clone_from(&span.numbers);
                snap.sets = span
                    .sets
                    .iter()
                    .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                    .collect();
            });

            result
        })
        .await;

    // Drop the inner clone so `snapshot` is the sole Arc owner
    drop(snapshot_inner);

    let snapshot = std::sync::Arc::try_unwrap(snapshot)
        .expect("snapshot Arc should have single owner")
        .into_inner()
        .expect("snapshot Mutex should not be poisoned");

    (result, snapshot)
}

/// Run an async pipeline with an active telemetry span, auto-merging into the event.
///
/// This is a convenience wrapper around [`with_telemetry_scope()`] that eliminates
/// the manual `merge_result_span` boilerplate at call sites. The span fields are
/// merged into the telemetry event (if present) regardless of whether the future
/// succeeded or failed — span data like `detected_language` and `services_used`
/// is valuable for diagnostics even on error paths.
///
/// # Example
///
/// ```ignore
/// let gen_result = run_with_telemetry(
///     handle_generate_policy(&config),
///     &mut telemetry_event,
/// ).await;
/// match gen_result {
///     Ok(()) => ExitCode::Success,
///     Err(e) => { /* handle error */ }
/// }
/// ```
pub async fn run_with_telemetry<F, T>(future: F, event: &mut Option<super::TelemetryEvent>) -> T
where
    F: std::future::Future<Output = T>,
{
    let start = std::time::Instant::now();
    let (result, span) = with_telemetry_scope(future).await;
    if let Some(ref mut ev) = event {
        ev.merge_result_span(&span);
        // Always record wall-clock e2e latency — overwrites any inner runtime_ms
        ev.set_result_number("runtime_ms", start.elapsed().as_millis() as usize);
    }
    result
}

/// Run an async pipeline with full telemetry lifecycle: run with span, set
/// `success`, and fire-and-forget emit.
///
/// This is the highest-level convenience wrapper — it combines
/// [`run_with_telemetry()`] and [`spawn_telemetry()`] into a single call.
/// Ideal for MCP tool handlers where the entire telemetry lifecycle can be
/// handled in two lines instead of 15+.
///
/// Create the telemetry event **before** the future consumes the input, so
/// the telemetry fields are captured even if the future moves the data.
///
/// # Example
///
/// ```ignore
/// // Before (15+ lines of boilerplate per tool):
/// let mut telemetry_event = params.0.to_telemetry_event();
/// let result = telemetry::span::run_with_telemetry(
///     generate_application_policies(params.0),
///     &mut telemetry_event,
/// ).await;
/// match result {
///     Ok(output) => {
///         if let Some(event) = telemetry_event {
///             telemetry::spawn_telemetry(event.with_result_success(true));
///         }
///         Ok(Json(output))
///     }
///     Err(e) => {
///         if let Some(event) = telemetry_event {
///             telemetry::spawn_telemetry(event.with_result_success(false));
///         }
///         Err(format_error(e))
///     }
/// }
///
/// // After (two lines + chaining):
/// let telemetry_event = params.0.to_telemetry_event();
/// run_with_telemetry_emit(telemetry_event, generate_application_policies(params.0))
///     .await
///     .map(Json)
///     .map_err(|e| format_error(e))
/// ```
pub async fn run_with_telemetry_emit<F, T, E>(
    mut telemetry_event: Option<super::TelemetryEvent>,
    future: F,
) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let result = run_with_telemetry(future, &mut telemetry_event).await;
    if let Some(event) = telemetry_event {
        super::spawn_telemetry(event.with_result_success(result.is_ok()));
    }
    result
}

/// Record a string result field into the telemetry span.
///
/// The field will appear in the `result` section of the telemetry event JSON.
/// Silently no-ops if no telemetry scope is active (e.g., in tests or
/// when called outside `with_telemetry_scope`).
pub fn record_result_str(key: &str, value: &str) {
    let _ = TELEMETRY_SPAN.try_with(|span| {
        span.borrow_mut()
            .strings
            .insert(key.to_string(), value.to_string());
    });
}

/// Record a numeric result field into the telemetry span.
///
/// The field will appear in the `result` section of the telemetry event JSON.
/// Silently no-ops if no telemetry scope is active (e.g., in tests or
/// when called outside `with_telemetry_scope`).
pub fn record_result_number(key: &str, value: usize) {
    let _ = TELEMETRY_SPAN.try_with(|span| {
        span.borrow_mut().numbers.insert(key.to_string(), value);
    });
}

/// Add a value to a result set field in the telemetry span.
///
/// The set will appear as a JSON array in the `result` section of the telemetry event.
/// Deduplicates automatically — recording "s3" twice results in a single entry.
/// Silently no-ops if no telemetry scope is active.
pub fn record_result_set(key: &str, value: &str) {
    let _ = TELEMETRY_SPAN.try_with(|span| {
        span.borrow_mut()
            .sets
            .entry(key.to_string())
            .or_default()
            .insert(value.to_string());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{TelemetryEvent, ToTelemetryEvent};
    use rstest::rstest;

    /// Convenience: run a recording closure inside a scope, return snapshot.
    async fn scoped(f: impl std::future::Future<Output = ()>) -> TelemetrySpanSnapshot {
        let ((), snap) = with_telemetry_scope(f).await;
        snap
    }

    // =========================================================================
    // record_result_* — basic recording, last-write-wins, edge cases
    // =========================================================================

    #[tokio::test]
    async fn test_record_result_str() {
        let snap = scoped(async {
            record_result_str("lang", "go");
            record_result_str("lang", "python"); // last-write-wins
            record_result_str("format", "json");
        })
        .await;

        assert_eq!(snap.strings.get("lang").unwrap(), "python");
        assert_eq!(snap.strings.get("format").unwrap(), "json");
    }

    #[tokio::test]
    async fn test_record_result_number() {
        let snap = scoped(async {
            record_result_number("count", 1);
            record_result_number("count", 3); // last-write-wins
            record_result_number("zero", 0); // zero is valid
        })
        .await;

        assert_eq!(snap.numbers.get("count"), Some(&3));
        assert_eq!(snap.numbers.get("zero"), Some(&0));
    }

    #[tokio::test]
    async fn test_record_result_set() {
        let snap = scoped(async {
            record_result_set("svc", "s3");
            record_result_set("svc", "dynamodb");
            record_result_set("svc", "s3"); // duplicate ignored
            record_result_set("regions", "us-east-1");
            record_result_set("regions", "eu-west-1");
        })
        .await;

        assert_eq!(snap.sets["svc"], vec!["dynamodb", "s3"]);
        assert_eq!(snap.sets["regions"], vec!["eu-west-1", "us-east-1"]);
    }

    // =========================================================================
    // Scope edge cases
    // =========================================================================

    #[tokio::test]
    async fn test_empty_span() {
        let snap = scoped(async {}).await;
        assert!(snap.strings.is_empty());
        assert!(snap.numbers.is_empty());
        assert!(snap.sets.is_empty());
    }

    #[test]
    fn test_record_outside_scope_is_noop() {
        // Should not panic — silently ignored when no scope is active
        record_result_str("key", "value");
        record_result_number("num", 42);
        record_result_set("set", "item");
    }

    #[tokio::test]
    async fn test_nested_async_calls_share_span() {
        async fn inner() {
            record_result_str("from_inner", "yes");
            record_result_set("svc", "lambda");
        }

        let snap = scoped(async {
            record_result_str("from_outer", "yes");
            record_result_set("svc", "s3");
            inner().await;
        })
        .await;

        assert_eq!(snap.strings["from_outer"], "yes");
        assert_eq!(snap.strings["from_inner"], "yes");
        assert_eq!(snap.sets["svc"], vec!["lambda", "s3"]);
    }

    // =========================================================================
    // run_with_telemetry — parameterized success/error/none
    // =========================================================================

    #[rstest]
    #[case::success_merges_all_types(true)]
    #[case::error_still_merges(false)]
    #[tokio::test]
    async fn test_run_with_telemetry_merges_span(#[case] succeed: bool) {
        let mut event = Some(TelemetryEvent::new("test-cmd"));

        let result: Result<(), String> = run_with_telemetry(
            async {
                record_result_str("detected_language", "python");
                record_result_number("num_policies_generated", 3);
                record_result_set("services_used", "s3");
                if succeed {
                    Ok(())
                } else {
                    Err("fail".into())
                }
            },
            &mut event,
        )
        .await;

        assert_eq!(result.is_ok(), succeed);

        let result_map = event.unwrap().result.unwrap();
        assert_eq!(
            result_map["detected_language"],
            serde_json::Value::String("python".into()),
        );
        assert_eq!(result_map["num_policies_generated"], serde_json::json!(3),);
        assert_eq!(result_map["services_used"], serde_json::json!(["s3"]),);
    }

    #[tokio::test]
    async fn test_run_with_telemetry_none_event() {
        let mut event: Option<TelemetryEvent> = None;
        let _: () = run_with_telemetry(
            async {
                record_result_str("k", "v");
            },
            &mut event,
        )
        .await;
        assert!(event.is_none());
    }

    #[tokio::test]
    async fn test_run_with_telemetry_preserves_existing_result_fields() {
        let mut event = Some(TelemetryEvent::new("cmd").with_result_success(true));
        let _: () = run_with_telemetry(
            async {
                record_result_str("lang", "go");
            },
            &mut event,
        )
        .await;

        let result_map = event.unwrap().result.unwrap();
        assert_eq!(result_map["success"], serde_json::json!(true));
        assert_eq!(result_map["lang"], serde_json::json!("go"));
    }

    // =========================================================================
    // run_with_telemetry_emit — end-to-end: create event, run, emit
    // =========================================================================

    /// Minimal struct implementing `ToTelemetryEvent` for testing.
    struct TestInput {
        emit: bool,
    }

    impl crate::telemetry::ToTelemetryEvent for TestInput {
        fn to_telemetry_event(&self) -> Option<TelemetryEvent> {
            if self.emit {
                Some(TelemetryEvent::new("test-emit-cmd"))
            } else {
                None
            }
        }
        fn telemetry_fields() -> Vec<crate::telemetry::TelemetryFieldInfo> {
            vec![]
        }
    }

    #[rstest]
    #[case::success_returns_ok(true)]
    #[case::error_returns_err(false)]
    #[tokio::test]
    async fn test_run_with_telemetry_emit_result(#[case] succeed: bool) {
        let input = TestInput { emit: true };
        let event = input.to_telemetry_event();
        let result: Result<&str, String> = run_with_telemetry_emit(event, async {
            record_result_str("detected_language", "python");
            if succeed {
                Ok("ok")
            } else {
                Err("boom".into())
            }
        })
        .await;

        assert_eq!(result.is_ok(), succeed);
        if succeed {
            assert_eq!(result.unwrap(), "ok");
        } else {
            assert_eq!(result.unwrap_err(), "boom");
        }
        // Event was consumed internally (spawn_telemetry fired-and-forgot).
        // We can't inspect it, but we verify no panic occurred.
    }

    #[tokio::test]
    async fn test_run_with_telemetry_emit_none_event_still_runs() {
        let input = TestInput { emit: false };
        let event = input.to_telemetry_event(); // returns None
        let result: Result<i32, String> = run_with_telemetry_emit(event, async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_run_with_telemetry_emit_span_fields_recorded() {
        // Verify that span recording works within run_with_telemetry_emit
        // by checking that the scope is active (no panic on record calls).
        let input = TestInput { emit: true };
        let event = input.to_telemetry_event();
        let result: Result<(), String> = run_with_telemetry_emit(event, async {
            record_result_str("lang", "go");
            record_result_number("count", 5);
            record_result_set("svc", "s3");
            record_result_set("svc", "dynamodb");
            Ok(())
        })
        .await;
        assert!(result.is_ok());
    }
}
