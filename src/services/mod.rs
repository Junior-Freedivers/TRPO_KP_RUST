//! Сервисный слой: бизнес-правила, транзакции, SQL-запросы. Ничего не знает о HTTP, кроме кодов ошибок.

pub mod attachments;
pub mod correspondents;
pub mod dictionaries;
pub mod documents;
pub mod exports;
pub mod reports;
pub mod resolutions;
pub mod users;

/// Шаблон для ILIKE с экранированием спецсимволов (%, _, \). Использовать с `ESCAPE '\'`.
pub fn like_pattern(term: &str) -> String {
    let escaped = term.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests {
    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(super::like_pattern(" 50%_a\\b "), "%50\\%\\_a\\\\b%");
    }
}
