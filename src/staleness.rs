//! Classify content by how long ago it last changed.

use crate::render::Paint;

const SECONDS_PER_DAY: f64 = 86_400.0;

/// Ordered labels for content inactivity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StaleLevel {
    Recent,
    Notice,
    Warning,
    Elevated,
    Critical,
}

impl StaleLevel {
    pub fn name(self) -> &'static str {
        match self {
            StaleLevel::Recent => "recent",
            StaleLevel::Notice => "notice",
            StaleLevel::Warning => "warning",
            StaleLevel::Elevated => "elevated",
            StaleLevel::Critical => "critical",
        }
    }

    /// The table color for this level.
    pub fn paint(self) -> Option<Paint> {
        match self {
            StaleLevel::Recent => None,
            StaleLevel::Notice => Some(Paint::Blue),
            StaleLevel::Warning => Some(Paint::Yellow),
            StaleLevel::Elevated => Some(Paint::Orange),
            StaleLevel::Critical => Some(Paint::Red),
        }
    }
}

/// Four strictly increasing day boundaries for the inactivity levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaleThresholds(pub [u64; 4]);

impl Default for StaleThresholds {
    fn default() -> Self {
        StaleThresholds([30, 90, 180, 365])
    }
}

impl StaleThresholds {
    /// Parse four comma-separated day boundaries such as `30,90,180,365`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let parts: Vec<&str> = value.split(',').map(str::trim).collect();
        let invalid = || {
            "stale thresholds must contain four comma-separated integers, for example 30,90,180,365"
                .to_string()
        };
        if parts.len() != 4
            || parts.iter().any(|part| {
                part.is_empty() || !part.chars().all(|character| character.is_ascii_digit())
            })
        {
            return Err(invalid());
        }
        let mut values = [0u64; 4];
        for (slot, part) in values.iter_mut().zip(&parts) {
            *slot = part.parse().map_err(|_| invalid())?;
        }
        if values.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("stale thresholds must be four strictly increasing day values".into());
        }
        Ok(StaleThresholds(values))
    }

    pub fn as_text(&self) -> String {
        self.0.map(|value| value.to_string()).join(",")
    }

    fn classify(&self, age_days: u64) -> StaleLevel {
        let [notice, warning, elevated, critical] = self.0;
        if age_days >= critical {
            StaleLevel::Critical
        } else if age_days >= elevated {
            StaleLevel::Elevated
        } else if age_days >= warning {
            StaleLevel::Warning
        } else if age_days >= notice {
            StaleLevel::Notice
        } else {
            StaleLevel::Recent
        }
    }
}

/// A whole-day age and its level.
#[derive(Clone, Copy, Debug)]
pub struct StaleAssessment {
    pub age_days: u64,
    pub level: StaleLevel,
}

impl StaleAssessment {
    /// A compact age label, empty for recent content.
    /// The age in days; only the color tells the level.
    pub fn label(&self) -> String {
        format!("{}d", self.age_days)
    }
}

/// Assess a modification time against a reference epoch.
pub fn assess(
    latest_modified_epoch: f64,
    reference_epoch: i64,
    thresholds: &StaleThresholds,
) -> StaleAssessment {
    let age_seconds = (reference_epoch as f64 - latest_modified_epoch).max(0.0);
    let age_days = (age_seconds / SECONDS_PER_DAY).floor() as u64;
    StaleAssessment {
        age_days,
        level: thresholds.classify(age_days),
    }
}

/// The newest modification epoch allowed by a minimum age in days.
pub fn cutoff_epoch(reference_epoch: i64, minimum_age_days: Option<u64>) -> Option<f64> {
    minimum_age_days.map(|days| {
        let age = i64::try_from(days)
            .unwrap_or(i64::MAX)
            .saturating_mul(86_400);
        reference_epoch.saturating_sub(age) as f64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_parse_and_classify() {
        let thresholds = StaleThresholds::parse("30,90,180,365").unwrap();
        assert_eq!(
            assess(0.0, 30 * 86_400, &thresholds).level,
            StaleLevel::Notice
        );
        assert_eq!(assess(0.0, 400 * 86_400, &thresholds).label(), "400d");
        // A recent entry still shows its age.
        assert_eq!(assess(0.0, 5 * 86_400, &thresholds).label(), "5d");
        assert!(StaleThresholds::parse("30,30,180,365").is_err());
        assert!(StaleThresholds::parse("30,90,180").is_err());
    }
}
