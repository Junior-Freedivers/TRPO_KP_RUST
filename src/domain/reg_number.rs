//! Формирование регистрационного номера по шаблону REG_NUMBER_TEMPLATE.
//!
//! Поддерживаемые плейсхолдеры: `{YYYY}`, `{YY}`, `{N...}` (число N задает ширину с ведущими нулями).
//! Пример: «ВХ-{YYYY}-{NNNNNN}» -> «ВХ-2026-000001».

use std::sync::LazyLock;

use regex::{Captures, Regex};

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{(YYYY|YY|N+)\}").unwrap());
static SEQUENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{N+\}").unwrap());

pub fn validate_template(template: &str) -> Result<(), String> {
    if SEQUENCE.is_match(template) {
        Ok(())
    } else {
        Err("REG_NUMBER_TEMPLATE должен содержать порядковый номер вида {NNNNNN}".into())
    }
}

pub fn uses_year(template: &str) -> bool {
    template.contains("{YYYY}") || template.contains("{YY}")
}

/// Ключ счетчика: годовой сброс, если в шаблоне есть год; иначе сквозная нумерация (ключ 0).
pub fn counter_key(template: &str, year: i32) -> i32 {
    if uses_year(template) { year } else { 0 }
}

pub fn format_registration_number(template: &str, year: i32, sequence: i64) -> String {
    PLACEHOLDER
        .replace_all(template, |caps: &Captures| {
            let token = &caps[1];
            match token {
                "YYYY" => format!("{year:04}"),
                "YY" => format!("{:02}", year.rem_euclid(100)),
                _ => format!("{sequence:0width$}", width = token.len()),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_default_template() {
        assert_eq!(format_registration_number("ВХ-{YYYY}-{NNNNNN}", 2026, 1), "ВХ-2026-000001");
        assert_eq!(format_registration_number("{YY}/{NNN}", 2026, 42), "26/042");
        assert_eq!(format_registration_number("ВХ-{NN}", 2026, 1234), "ВХ-1234");
    }

    #[test]
    fn counter_key_depends_on_year_placeholder() {
        assert_eq!(counter_key("ВХ-{YYYY}-{NNNNNN}", 2026), 2026);
        assert_eq!(counter_key("ВХ-{YY}-{NNNNNN}", 2026), 2026);
        assert_eq!(counter_key("ВХ-{NNNNNN}", 2026), 0);
    }

    #[test]
    fn template_without_sequence_is_rejected() {
        assert!(validate_template("ВХ-{YYYY}").is_err());
        assert!(validate_template("ВХ-{N}").is_ok());
    }
}
