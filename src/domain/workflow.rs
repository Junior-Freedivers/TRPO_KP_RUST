//! Централизованная машина состояний карточки документа.
//!
//! Единственное место, где описаны допустимые переходы статусов и роли, которым эти переходы разрешены.

use crate::models::{DocumentStatus as S, Role};

/// Роли, которым разрешен переход; `None` — если переход вообще не существует.
pub fn transition_roles(old: S, new: S) -> Option<&'static [Role]> {
    match (old, new) {
        (S::Registered, S::UnderReview) => Some(&[Role::Clerk]),
        // переход выполняется автоматически при создании резолюции руководителем
        (S::UnderReview, S::InExecution) => Some(&[Role::Manager]),
        (S::InExecution, S::Executed) => Some(&[Role::Executor]),
        (S::Executed, S::RemovedFromControl) => Some(&[Role::Clerk]),
        (S::RemovedFromControl, S::Archived) => Some(&[Role::Clerk]),
        // аннулирование: из любого статуса обратно в «зарегистрирован» — только администратор
        (old, S::Registered) if old != S::Registered => Some(&[Role::Admin]),
        _ => None,
    }
}

pub fn is_transition_allowed(old: S, new: S) -> bool {
    transition_roles(old, new).is_some()
}

pub fn allowed_targets(old: S) -> Vec<S> {
    S::ALL.iter().copied().filter(|&new| is_transition_allowed(old, new)).collect()
}

/// Статусы, в которых срок исполнения уже не контролируется.
pub const CLOSED_STATUSES: [S; 3] = [S::Executed, S::RemovedFromControl, S::Archived];

/// Статусы, в которых резолюции можно создавать и редактировать.
pub const RESOLUTION_OPEN_STATUSES: [S; 2] = [S::UnderReview, S::InExecution];

pub fn is_closed(status: S) -> bool {
    CLOSED_STATUSES.contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_chain_is_allowed_for_expected_roles() {
        assert_eq!(transition_roles(S::Registered, S::UnderReview), Some(&[Role::Clerk][..]));
        assert_eq!(transition_roles(S::UnderReview, S::InExecution), Some(&[Role::Manager][..]));
        assert_eq!(transition_roles(S::InExecution, S::Executed), Some(&[Role::Executor][..]));
        assert_eq!(transition_roles(S::Executed, S::RemovedFromControl), Some(&[Role::Clerk][..]));
        assert_eq!(transition_roles(S::RemovedFromControl, S::Archived), Some(&[Role::Clerk][..]));
    }

    #[test]
    fn skipping_steps_is_forbidden() {
        assert!(!is_transition_allowed(S::Registered, S::Executed));
        assert!(!is_transition_allowed(S::Registered, S::Archived));
        assert!(!is_transition_allowed(S::Archived, S::Executed));
        assert!(!is_transition_allowed(S::Registered, S::Registered));
    }

    #[test]
    fn annulment_only_by_admin() {
        for &status in S::ALL {
            if status != S::Registered {
                assert_eq!(transition_roles(status, S::Registered), Some(&[Role::Admin][..]));
            }
        }
    }

    #[test]
    fn allowed_targets_of_registered() {
        assert_eq!(allowed_targets(S::Registered), vec![S::UnderReview]);
        assert_eq!(allowed_targets(S::Archived), vec![S::Registered]);
    }
}
