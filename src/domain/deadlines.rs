//! Контроль сроков исполнения (чистые функции).

use chrono::NaiveDate;

use crate::domain::workflow::is_closed;
use crate::models::{DeadlineState, DocumentStatus};

pub fn deadline_state(deadline: NaiveDate, status: DocumentStatus, today: NaiveDate, warning_days: i64) -> DeadlineState {
    if is_closed(status) {
        return DeadlineState::Closed;
    }
    if deadline < today {
        return DeadlineState::Overdue;
    }
    if (deadline - today).num_days() <= warning_days {
        return DeadlineState::Warning;
    }
    DeadlineState::Ok
}

/// Оставшиеся дни; отрицательное значение — просрочка.
pub fn days_left(deadline: NaiveDate, today: NaiveDate) -> i64 {
    (deadline - today).num_days()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn states() {
        let today = d("2026-10-10");
        let open = DocumentStatus::InExecution;
        assert_eq!(deadline_state(d("2026-10-09"), open, today, 3), DeadlineState::Overdue);
        assert_eq!(deadline_state(d("2026-10-10"), open, today, 3), DeadlineState::Warning);
        assert_eq!(deadline_state(d("2026-10-13"), open, today, 3), DeadlineState::Warning);
        assert_eq!(deadline_state(d("2026-10-14"), open, today, 3), DeadlineState::Ok);
        assert_eq!(deadline_state(d("2026-10-01"), DocumentStatus::Executed, today, 3), DeadlineState::Closed);
        assert_eq!(deadline_state(d("2026-10-01"), DocumentStatus::Archived, today, 3), DeadlineState::Closed);
    }

    #[test]
    fn zero_warning_days() {
        let today = d("2026-10-10");
        assert_eq!(deadline_state(today, DocumentStatus::Registered, today, 0), DeadlineState::Warning);
        assert_eq!(deadline_state(d("2026-10-11"), DocumentStatus::Registered, today, 0), DeadlineState::Ok);
    }

    #[test]
    fn days_left_is_signed() {
        assert_eq!(days_left(d("2026-10-12"), d("2026-10-10")), 2);
        assert_eq!(days_left(d("2026-10-08"), d("2026-10-10")), -2);
    }
}
