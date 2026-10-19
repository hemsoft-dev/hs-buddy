//! Missing Jira classification is unresolved work, not evidence of non-CapEx.
use std::collections::HashMap;

use super::{Error, IssueId, TimesheetSummary};

pub struct CapitalizationTotals {
    pub capitalized_seconds: u64,
    pub non_capitalized_seconds: u64,
    pub unclassified_seconds: u64,
    pub unclassified_worklogs: usize,
}
impl CapitalizationTotals {
    pub fn all_classified(&self) -> bool {
        self.unclassified_worklogs == 0
    }
}

impl TimesheetSummary {
    /// Use classifications from this snapshot's captured Jira connection only.
    /// A failed lookup may remain absent/None; it must not be cached as false.
    pub fn capitalization_totals(
        &self,
        values: &HashMap<IssueId, Option<bool>>,
    ) -> Result<CapitalizationTotals, Error> {
        let mut totals = CapitalizationTotals {
            capitalized_seconds: 0,
            non_capitalized_seconds: 0,
            unclassified_seconds: 0,
            unclassified_worklogs: 0,
        };
        for row in self.days.iter().flat_map(|day| &day.logged.worklogs) {
            let classification = row
                .issue
                .and_then(|issue| values.get(&issue).copied())
                .flatten();
            let seconds = match classification {
                Some(true) => &mut totals.capitalized_seconds,
                Some(false) => &mut totals.non_capitalized_seconds,
                None => {
                    totals.unclassified_worklogs += 1;
                    &mut totals.unclassified_seconds
                }
            };
            *seconds = seconds
                .checked_add(row.seconds)
                .ok_or(Error::TotalOverflow)?;
        }
        let total = totals
            .capitalized_seconds
            .checked_add(totals.non_capitalized_seconds)
            .and_then(|value| value.checked_add(totals.unclassified_seconds))
            .ok_or(Error::TotalOverflow)?;
        if total != self.logged_seconds {
            return Err(Error::InvalidResponse);
        }
        Ok(totals)
    }
}

#[cfg(test)]
#[path = "capitalization_totals_tests.rs"]
mod tests;
