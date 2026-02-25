//! Tool loop detection for agent execution
//!
//! Detect stuck/repeating patterns in tool calls to prevent infinite loops.
//! Mirrors `OpenClaw`'s loop detection with generic repeat, no-progress,
//! ping-pong, and circuit breaker detectors.

use std::collections::VecDeque;

use sha2::{Digest, Sha256};

/// Sliding window size for loop detection
const WINDOW_SIZE: usize = 30;

/// Threshold for generic repeat and no-progress warnings
const WARNING_THRESHOLD: usize = 10;

/// Threshold for critical severity
const CRITICAL_THRESHOLD: usize = 20;

/// Threshold for circuit breaker
const BREAKER_THRESHOLD: usize = 30;

/// Severity of a detected loop pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopSeverity {
    /// No loop detected
    None,
    /// Pattern is suspicious but not yet critical
    Warning,
    /// Loop is likely — inject warning to LLM
    Critical,
    /// Hard limit reached — break the loop
    CircuitBreaker,
}

/// A recorded tool call for pattern analysis
#[derive(Debug, Clone)]
struct ToolCallRecord {
    name: String,
    params_hash: [u8; 32],
    outcome_hash: [u8; 32],
}

/// Detect stuck/repeating tool call patterns
#[derive(Debug)]
pub struct LoopDetector {
    window: VecDeque<ToolCallRecord>,
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_SIZE),
        }
    }
}

impl LoopDetector {
    /// Record a tool call and check for loop patterns.
    ///
    /// Returns the highest severity detected across all pattern checks.
    pub fn record(&mut self, name: &str, params: &str, outcome: &str) -> LoopSeverity {
        let params_hash = sha256(params);
        let outcome_hash = sha256(outcome);

        let record = ToolCallRecord {
            name: name.to_string(),
            params_hash,
            outcome_hash,
        };

        // Maintain sliding window
        if self.window.len() >= WINDOW_SIZE {
            self.window.pop_front();
        }
        self.window.push_back(record);

        // Run all detectors, return the highest severity
        let severities = [
            self.check_circuit_breaker(name, &params_hash),
            self.check_no_progress(name, &params_hash, &outcome_hash),
            self.check_ping_pong(),
            self.check_generic_repeat(name, &params_hash),
        ];

        severities.into_iter().max().unwrap_or(LoopSeverity::None)
    }

    /// Circuit breaker: any single `(name, params)` fingerprint >= `BREAKER_THRESHOLD` times
    fn check_circuit_breaker(&self, name: &str, params_hash: &[u8; 32]) -> LoopSeverity {
        let count = self
            .window
            .iter()
            .filter(|r| r.name == name && r.params_hash == *params_hash)
            .count();

        if count >= BREAKER_THRESHOLD {
            LoopSeverity::CircuitBreaker
        } else {
            LoopSeverity::None
        }
    }

    /// Generic repeat: same `(name, params_hash)` appears >= thresholds
    fn check_generic_repeat(&self, name: &str, params_hash: &[u8; 32]) -> LoopSeverity {
        let count = self
            .window
            .iter()
            .filter(|r| r.name == name && r.params_hash == *params_hash)
            .count();

        if count >= CRITICAL_THRESHOLD {
            LoopSeverity::Critical
        } else if count >= WARNING_THRESHOLD {
            LoopSeverity::Warning
        } else {
            LoopSeverity::None
        }
    }

    /// No-progress: same `(name, params_hash, outcome_hash)` repeats
    fn check_no_progress(
        &self,
        name: &str,
        params_hash: &[u8; 32],
        outcome_hash: &[u8; 32],
    ) -> LoopSeverity {
        let count = self
            .window
            .iter()
            .filter(|r| {
                r.name == name
                    && r.params_hash == *params_hash
                    && r.outcome_hash == *outcome_hash
            })
            .count();

        if count >= CRITICAL_THRESHOLD {
            LoopSeverity::Critical
        } else if count >= WARNING_THRESHOLD {
            LoopSeverity::Warning
        } else {
            LoopSeverity::None
        }
    }

    /// Ping-pong: alternating `(A, B, A, B)` pattern with stable outcomes
    fn check_ping_pong(&self) -> LoopSeverity {
        if self.window.len() < 4 {
            return LoopSeverity::None;
        }

        // Look for the pattern in the last N entries: A, B, A, B, ...
        let items: Vec<_> = self.window.iter().collect();
        let len = items.len();

        // Need at least 2 distinct alternating entries
        if len < 4 {
            return LoopSeverity::None;
        }

        // Check if the last entries form an alternating pattern
        let a = &items[len - 2];
        let b = &items[len - 1];

        // They must be different calls
        if a.name == b.name && a.params_hash == b.params_hash {
            return LoopSeverity::None;
        }

        // Count alternating pairs going backwards
        let mut alternations = 0;
        let mut i = len.saturating_sub(1);
        while i >= 1 {
            let expected = if i % 2 == (len - 1) % 2 { b } else { a };
            let current = &items[i];

            if current.name == expected.name
                && current.params_hash == expected.params_hash
                && current.outcome_hash == expected.outcome_hash
            {
                alternations += 1;
            } else {
                break;
            }

            if i == 0 {
                break;
            }
            i -= 1;
        }

        // Each pair = 2 alternations
        let pair_count = alternations / 2;

        if pair_count >= CRITICAL_THRESHOLD {
            LoopSeverity::Critical
        } else if pair_count >= WARNING_THRESHOLD {
            LoopSeverity::Warning
        } else {
            LoopSeverity::None
        }
    }
}

/// Compute SHA-256 hash of input
fn sha256(input: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher.finalize().into()
}

impl PartialOrd for LoopSeverity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoopSeverity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl LoopSeverity {
    const fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Warning => 1,
            Self::Critical => 2,
            Self::CircuitBreaker => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_false_positives_with_varied_calls() {
        let mut detector = LoopDetector::default();
        for i in 0..9 {
            let severity = detector.record(
                &format!("tool_{i}"),
                &format!("{{\"arg\": {i}}}"),
                &format!("result_{i}"),
            );
            assert_eq!(severity, LoopSeverity::None);
        }
    }

    #[test]
    fn generic_repeat_triggers_warning() {
        let mut detector = LoopDetector::default();
        for i in 0..WARNING_THRESHOLD {
            let severity = detector.record("web_search", "{\"q\": \"rust\"}", &format!("result_{i}"));
            if i + 1 >= WARNING_THRESHOLD {
                assert!(severity >= LoopSeverity::Warning, "expected Warning at iteration {i}");
            }
        }
    }

    #[test]
    fn no_progress_escalates_to_critical() {
        let mut detector = LoopDetector::default();
        for i in 0..CRITICAL_THRESHOLD {
            let severity = detector.record("read_file", "{\"path\": \"/a\"}", "contents");
            if i + 1 >= CRITICAL_THRESHOLD {
                assert!(severity >= LoopSeverity::Critical, "expected Critical at iteration {i}");
            }
        }
    }

    #[test]
    fn circuit_breaker_fires() {
        let mut detector = LoopDetector::default();
        for i in 0..BREAKER_THRESHOLD {
            let severity = detector.record("shell", "{\"cmd\": \"ls\"}", "files");
            if i + 1 >= BREAKER_THRESHOLD {
                assert_eq!(severity, LoopSeverity::CircuitBreaker);
            }
        }
    }

    #[test]
    fn sliding_window_evicts_old_entries() {
        let mut detector = LoopDetector::default();

        // Fill window with varied calls
        for i in 0..WINDOW_SIZE {
            detector.record(&format!("tool_{i}"), "{}", "ok");
        }

        // Now add repeated calls - should start from 1 in the window
        for i in 0..(WARNING_THRESHOLD - 1) {
            let severity = detector.record("repeat_tool", "{\"a\": 1}", "result");
            // Should not trigger yet because old entries are being evicted
            if i + 1 < WARNING_THRESHOLD {
                assert_eq!(severity, LoopSeverity::None, "false positive at iteration {i}");
            }
        }
    }

    #[test]
    fn severity_ordering() {
        assert!(LoopSeverity::CircuitBreaker > LoopSeverity::Critical);
        assert!(LoopSeverity::Critical > LoopSeverity::Warning);
        assert!(LoopSeverity::Warning > LoopSeverity::None);
    }
}
