//! Расчет показателей для отчетов (чистые функции, без обращения к БД).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;

use crate::domain::workflow::is_closed;
use crate::models::DocumentStatus;

#[derive(Debug, Clone)]
pub struct ResolutionFact {
    pub executor_id: String,
    pub executor_name: String,
    pub document_id: String,
    pub created_at: DateTime<Tz>,
    pub executed_at: Option<DateTime<Tz>>,
    pub deadline: NaiveDate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutorStat {
    pub executor_name: String,
    pub documents: usize,
    pub resolutions: usize,
    pub executed: usize,
    pub overdue: usize,
    pub avg_days: Option<f64>,
    pub overdue_pct: f64,
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

pub fn is_resolution_overdue(fact: &ResolutionFact, today: NaiveDate) -> bool {
    match fact.executed_at {
        Some(executed) => executed.date_naive() > fact.deadline,
        None => fact.deadline < today,
    }
}

pub fn compute_executor_stats(facts: &[ResolutionFact], today: NaiveDate) -> Vec<ExecutorStat> {
    let mut grouped: BTreeMap<&str, Vec<&ResolutionFact>> = BTreeMap::new();
    for fact in facts {
        grouped.entry(fact.executor_id.as_str()).or_default().push(fact);
    }

    let mut stats: Vec<ExecutorStat> = grouped
        .values()
        .map(|items| {
            let durations: Vec<f64> = items
                .iter()
                .filter_map(|f| f.executed_at.map(|e| (e - f.created_at).num_milliseconds() as f64 / 86_400_000.0))
                .collect();
            let overdue = items.iter().filter(|f| is_resolution_overdue(f, today)).count();
            let documents: HashSet<&str> = items.iter().map(|f| f.document_id.as_str()).collect();
            ExecutorStat {
                executor_name: items[0].executor_name.clone(),
                documents: documents.len(),
                resolutions: items.len(),
                executed: durations.len(),
                overdue,
                avg_days: (!durations.is_empty()).then(|| round1(durations.iter().sum::<f64>() / durations.len() as f64)),
                overdue_pct: round1(overdue as f64 * 100.0 / items.len() as f64),
            }
        })
        .collect();
    stats.sort_by(|a, b| a.executor_name.cmp(&b.executor_name));
    stats
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeadlineCategory {
    OnTime,
    Late,
    Overdue,
    InProgress,
    Unknown,
}

impl DeadlineCategory {
    pub const ALL: [DeadlineCategory; 5] = [Self::OnTime, Self::Late, Self::Overdue, Self::InProgress, Self::Unknown];

    pub fn label(&self) -> &'static str {
        match self {
            Self::OnTime => "Исполнен в срок",
            Self::Late => "Исполнен с просрочкой",
            Self::Overdue => "Просрочен, не исполнен",
            Self::InProgress => "В работе, срок не наступил",
            Self::Unknown => "Закрыт, дата исполнения не зафиксирована",
        }
    }
}

/// Категория исполнения и отклонение от срока в днях (>0 — просрочка, <0 — раньше срока).
pub fn classify_deadline(
    deadline: NaiveDate,
    status: DocumentStatus,
    executed_date: Option<NaiveDate>,
    today: NaiveDate,
) -> (DeadlineCategory, Option<i64>) {
    if is_closed(status) {
        return match executed_date {
            None => (DeadlineCategory::Unknown, None),
            Some(executed) => {
                let deviation = (executed - deadline).num_days();
                let category = if deviation <= 0 { DeadlineCategory::OnTime } else { DeadlineCategory::Late };
                (category, Some(deviation))
            }
        };
    }
    if deadline < today {
        return (DeadlineCategory::Overdue, Some((today - deadline).num_days()));
    }
    (DeadlineCategory::InProgress, None)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn at(s: &str) -> DateTime<Tz> {
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        chrono_tz::Europe::Moscow.from_local_datetime(&naive).unwrap()
    }

    fn fact(executor: &str, doc: &str, created: &str, executed: Option<&str>, deadline: &str) -> ResolutionFact {
        ResolutionFact {
            executor_id: executor.into(),
            executor_name: format!("Исполнитель {executor}"),
            document_id: doc.into(),
            created_at: at(created),
            executed_at: executed.map(at),
            deadline: d(deadline),
        }
    }

    #[test]
    fn executor_stats() {
        let today = d("2026-10-20");
        let facts = vec![
            fact("b", "d1", "2026-10-01 09:00", Some("2026-10-03 09:00"), "2026-10-05"),
            fact("b", "d2", "2026-10-01 09:00", Some("2026-10-08 09:00"), "2026-10-05"),
            fact("b", "d2", "2026-10-01 09:00", None, "2026-10-10"),
            fact("a", "d3", "2026-10-01 09:00", None, "2026-10-30"),
        ];
        let stats = compute_executor_stats(&facts, today);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].executor_name, "Исполнитель a");
        assert_eq!(stats[0].avg_days, None);
        assert_eq!(stats[0].overdue, 0);
        let b = &stats[1];
        assert_eq!((b.documents, b.resolutions, b.executed, b.overdue), (2, 3, 2, 2));
        assert_eq!(b.avg_days, Some(4.5));
        assert_eq!(b.overdue_pct, 66.7);
    }

    #[test]
    fn deadline_classification() {
        let today = d("2026-10-20");
        let closed = DocumentStatus::Executed;
        assert_eq!(classify_deadline(d("2026-10-10"), closed, Some(d("2026-10-09")), today),
                   (DeadlineCategory::OnTime, Some(-1)));
        assert_eq!(classify_deadline(d("2026-10-10"), closed, Some(d("2026-10-12")), today),
                   (DeadlineCategory::Late, Some(2)));
        assert_eq!(classify_deadline(d("2026-10-10"), closed, None, today), (DeadlineCategory::Unknown, None));
        assert_eq!(classify_deadline(d("2026-10-18"), DocumentStatus::InExecution, None, today),
                   (DeadlineCategory::Overdue, Some(2)));
        assert_eq!(classify_deadline(d("2026-10-25"), DocumentStatus::InExecution, None, today),
                   (DeadlineCategory::InProgress, None));
    }
}
