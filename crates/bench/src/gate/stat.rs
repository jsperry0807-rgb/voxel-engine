//! Shared nearest-rank summaries for the M1 gate harnesses.
//!
//! The edit (M1-37) and probe-update (M1-38) reports both answer the same
//! question — what are p50/p95/max for one content-timeline event — so they
//! share the sample summary, the budget comparison and the formatting. Keeping
//! one implementation means the gate report (M1-39) compares like with like.

use serde::{Deserialize, Serialize};

use crate::save_scan::Distribution;

/// Nearest-rank summary of one component or term.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    /// Nearest-rank median.
    pub p50_ns: u64,
    /// Nearest-rank 95th percentile.
    pub p95_ns: u64,
    /// Maximum sample.
    pub max_ns: u64,
    /// Integer mean of the samples.
    pub mean_ns: u64,
    /// Sum of the samples.
    pub total_ns: u64,
}

impl Term {
    /// Summarizes raw samples in place.
    ///
    /// # Panics
    ///
    /// Panics when `values` is empty; a term always has at least one sample.
    pub fn from_values(values: &mut [u64]) -> Self {
        assert!(!values.is_empty(), "a term needs at least one sample");
        let total_ns = values.iter().sum();
        let mean_ns = total_ns / values.len() as u64;
        let distribution = Distribution::from_values(values);
        Self {
            p50_ns: distribution.p50.unwrap_or(0),
            p95_ns: distribution.p95.unwrap_or(0),
            max_ns: distribution.max.unwrap_or(0),
            mean_ns,
            total_ns,
        }
    }
}

/// Summarizes one field of a sample slice.
pub fn term<T>(items: &[T], field: impl Fn(&T) -> u64) -> Term {
    let mut values: Vec<u64> = items.iter().map(field).collect();
    Term::from_values(&mut values)
}

/// Integer mean of a sample stream, saturating rather than overflowing.
pub fn mean(values: impl Iterator<Item = u64>) -> u64 {
    let (sum, count) = values.fold((0u64, 0u64), |(sum, count), value| {
        (sum.saturating_add(value), count + 1)
    });
    if count == 0 {
        0
    } else {
        sum.checked_div(count).unwrap_or(0)
    }
}

/// A pre-registered budget compared with the measured p95.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetCheck {
    /// Pre-registered p95 budget.
    pub budget_ns: u64,
    /// Measured p95.
    pub measured_p95_ns: u64,
    /// `budget_ns - measured_p95_ns`; negative means the budget was missed.
    pub margin_ns: i64,
    /// True when the measured p95 is inside the budget.
    pub within_budget: bool,
}

impl BudgetCheck {
    /// Compares a measured p95 with a pre-registered budget.
    pub fn new(budget_ns: u64, measured_p95_ns: u64) -> Self {
        let margin_ns = i64::try_from(budget_ns).unwrap_or(i64::MAX)
            - i64::try_from(measured_p95_ns).unwrap_or(i64::MAX);
        Self {
            budget_ns,
            measured_p95_ns,
            margin_ns,
            within_budget: margin_ns >= 0,
        }
    }
}

/// Formats a duration at the scale a gate table reads best.
pub fn format_duration(ns: u64) -> String {
    let value = ns as f64;
    if value >= 1e6 {
        format!("{:.2} ms", value / 1e6)
    } else if value >= 1e3 {
        format!("{:.3} µs", value / 1e3)
    } else {
        format!("{ns} ns")
    }
}

/// Formats a signed margin; negative values keep the sign and unit.
pub fn format_signed_duration(ns: i64) -> String {
    if ns < 0 {
        format!("-{}", format_duration(ns.unsigned_abs()))
    } else {
        format_duration(ns as u64)
    }
}

/// Groups an integer with thousands separators for the Markdown report.
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
