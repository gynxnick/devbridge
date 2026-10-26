//! Timestamps are Unix epoch milliseconds (UTC). They are for display and
//! ordering of local records only; sync decisions never depend on clocks.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
