//! Deletion tombstones: the record that the local `reap` tool deleted a resource.
//!
//! Resource Explorer keeps listing a resource for a few minutes after it is gone,
//! so a reap is immediately followed by a window in which the index still reports
//! the deleted ARNs. The refresh an operator naturally runs right after reaping
//! lands squarely in that window, and the resulting snapshot — complete with the
//! resources they just deleted — is what gets cached for a full TTL, making a
//! successful reap look like it failed.
//!
//! `reap` writes a tombstone (below) and the dashboard suppresses tombstoned ARNs
//! for [`WINDOW_SECS`], so the cached snapshot is correct the first time.
//!
//! Shared by the library so the writer (`reap`, running on the operator's own
//! credentials) and the reader (the Lambda) can't disagree on the attribute name.

use aws_sdk_dynamodb::types::AttributeValue;

/// How long a reaped ARN stays suppressed from inventory. Comfortably longer than
/// the observed Resource Explorer index lag, and short enough that a resource which
/// somehow survived deletion becomes visible again rather than being hidden forever.
pub const WINDOW_SECS: i64 = 30 * 60;

/// The DynamoDB attribute holding the deletion time, in Unix seconds.
pub const ATTR: &str = "deleted_at";

/// True if `deleted_at` is recent enough that the resource should still be hidden.
pub fn suppressed(deleted_at: i64, now: i64) -> bool {
    now - deleted_at < WINDOW_SECS
}

/// Record that `arn` was deleted: stamps the deletion time and clears the deletion
/// mark, which the reap tool has now acted on.
///
/// Takes a raw client rather than the Lambda's `AppState` because the caller is the
/// local operator tool — the deployed dashboard never deletes anything.
pub async fn record(
    ddb: &aws_sdk_dynamodb::Client,
    table: &str,
    arn: &str,
    now: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    ddb.update_item()
        .table_name(table)
        .key("arn", AttributeValue::S(arn.to_string()))
        .update_expression("SET #d = :now REMOVE #m")
        .expression_attribute_names("#d", ATTR)
        .expression_attribute_names("#m", "mark")
        .expression_attribute_values(":now", AttributeValue::N(now.to_string()))
        .send()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_785_000_000;

    #[test]
    fn a_just_deleted_resource_is_suppressed() {
        assert!(suppressed(NOW, NOW));
        assert!(suppressed(NOW - 60, NOW));
    }

    #[test]
    fn suppression_lapses_at_the_window_edge() {
        assert!(suppressed(NOW - WINDOW_SECS + 1, NOW));
        assert!(!suppressed(NOW - WINDOW_SECS, NOW));
        assert!(!suppressed(NOW - WINDOW_SECS - 1, NOW));
    }

    /// Clock skew between the operator's machine and the Lambda shouldn't resurrect
    /// a row: a tombstone stamped slightly in the future still suppresses.
    #[test]
    fn a_future_timestamp_still_suppresses() {
        assert!(suppressed(NOW + 30, NOW));
    }
}
