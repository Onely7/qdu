//! Assess recorded usage against an operational capacity limit.

use std::collections::HashMap;

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::config::ProfileConfig;
use crate::error::{Result, usage};
use crate::query::{directory_allocation, owner_allocation, resolve_user};
use crate::render::{Align, Paint, Renderer};
use crate::report::{Column, Field, Table};
use crate::sys;
use crate::units::format_size;

/// A positive byte limit and the owner it applies to, if any.
#[derive(Clone, Debug)]
pub struct CapacityPolicy {
    pub limit_bytes: u64,
    pub user: Option<String>,
}

/// Capacity options given on the command line; `None` keeps the profile's.
#[derive(Clone, Debug, Default)]
pub struct CapacityOverrides {
    pub limit_bytes: Option<Option<u64>>,
    pub user: Option<Option<String>>,
}

impl CapacityOverrides {
    /// The effective policy from these options and the profile.
    pub fn policy(&self, profile: &ProfileConfig) -> Result<Option<CapacityPolicy>> {
        if self.limit_bytes == Some(None) {
            return Ok(None);
        }
        let limit = self.limit_bytes.unwrap_or(profile.capacity_limit_bytes);
        let user = self
            .user
            .clone()
            .unwrap_or_else(|| profile.capacity_user.clone());
        match limit {
            None if user.is_some() => Err(usage(
                "--capacity-user requires --capacity-limit or a configured limit",
            )),
            None => Ok(None),
            Some(limit_bytes) => Ok(Some(CapacityPolicy { limit_bytes, user })),
        }
    }
}

/// Usage measured against a limit.
#[derive(Clone, Debug)]
pub struct CapacityAssessment {
    pub target: String,
    pub limit_bytes: u64,
    pub used_bytes: u64,
}

impl CapacityAssessment {
    pub fn usage_percent(&self) -> f64 {
        self.used_bytes as f64 * 100.0 / self.limit_bytes as f64
    }

    pub fn exceeded(&self) -> bool {
        self.used_bytes > self.limit_bytes
    }

    pub fn remaining_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.used_bytes)
    }

    pub fn excess_bytes(&self) -> u64 {
        self.used_bytes.saturating_sub(self.limit_bytes)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "limit_bytes": self.limit_bytes,
            "used_bytes": self.used_bytes,
            "usage_percent": self.usage_percent(),
            "remaining_bytes": self.remaining_bytes(),
            "excess_bytes": self.excess_bytes(),
            "exceeded": self.exceeded(),
        })
    }
}

/// Measure a snapshot against a policy.
pub fn assess(
    connection: &Connection,
    metadata: &HashMap<String, String>,
    policy: &CapacityPolicy,
) -> Result<CapacityAssessment> {
    let Some(user) = &policy.user else {
        let used = directory_allocation(connection, ".")?.unwrap_or(0);
        return Ok(CapacityAssessment {
            target: "profile root".into(),
            limit_bytes: policy.limit_bytes,
            used_bytes: used.max(0) as u64,
        });
    };
    if metadata.get("collect_users").map(String::as_str) != Some("true") {
        return Err(usage(
            "capacity usage for one user requires owner statistics; \
             take a new snapshot with '--with-users'",
        ));
    }
    let uid = resolve_user(user)?;
    let used = owner_allocation(connection, uid)?;
    Ok(CapacityAssessment {
        target: format!("user {} (uid {uid})", sys::username_for_uid(uid)),
        limit_bytes: policy.limit_bytes,
        used_bytes: used.max(0) as u64,
    })
}

/// Warn on standard error when usage exceeds the limit.
pub fn warn(renderer: &mut Renderer, assessment: Option<&CapacityAssessment>) {
    let Some(assessment) = assessment.filter(|assessment| assessment.exceeded()) else {
        return;
    };
    renderer.warning(&format!(
        "operational capacity limit exceeded by {}: {} used of {} ({:.1}%; {})",
        format_size(assessment.excess_bytes()),
        format_size(assessment.used_bytes),
        format_size(assessment.limit_bytes),
        assessment.usage_percent(),
        assessment.target
    ));
}

/// The capacity status as a one-row table.
pub fn status_table(assessment: &CapacityAssessment) -> Table {
    let exceeded = assessment.exceeded();
    let mut table = Table::new(vec![
        Column::new("target", "Target", Align::Left),
        Column::new("used_bytes", "Used", Align::Right),
        Column::new("limit_bytes", "Allowed", Align::Right),
        Column::new("usage_percent", "Usage", Align::Right),
        Column::new(
            if exceeded {
                "excess_bytes"
            } else {
                "remaining_bytes"
            },
            if exceeded { "Exceeded by" } else { "Remaining" },
            Align::Right,
        ),
        Column::new("exceeded", "Status", Align::Left),
    ])
    .heading("Operational capacity limit");
    let remaining = if exceeded {
        assessment.excess_bytes()
    } else {
        assessment.remaining_bytes()
    };
    table.push(vec![
        Field::text(assessment.target.clone()),
        Field::new(assessment.used_bytes, format_size(assessment.used_bytes)),
        Field::new(assessment.limit_bytes, format_size(assessment.limit_bytes)),
        Field::new(
            assessment.usage_percent(),
            format!("{:.1}%", assessment.usage_percent()),
        ),
        Field::new(remaining, format_size(remaining)),
        Field::new(exceeded, if exceeded { "EXCEEDED" } else { "within limit" })
            .painted(Some(if exceeded { Paint::Red } else { Paint::Green })),
    ]);
    table
}

#[cfg(test)]
mod tests {
    use super::CapacityAssessment;

    #[test]
    fn assessments_report_remaining_and_excess_space() {
        let within = CapacityAssessment {
            target: "test".into(),
            limit_bytes: 100,
            used_bytes: 25,
        };
        assert!(!within.exceeded());
        assert_eq!(within.remaining_bytes(), 75);
        assert_eq!(within.usage_percent(), 25.0);
        let exceeded = CapacityAssessment {
            used_bytes: 125,
            ..within
        };
        assert!(exceeded.exceeded());
        assert_eq!(exceeded.excess_bytes(), 25);
        assert_eq!(exceeded.remaining_bytes(), 0);
    }
}
