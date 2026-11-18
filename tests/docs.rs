//! Документация в docs/ должна совпадать с кодом: диаграмма состояний — с машиной состояний,
//! ER-диаграмма — со схемой БД из миграций.

use std::collections::{BTreeSet, HashMap};

use correspondence::domain::workflow::transition_roles;
use correspondence::models::DocumentStatus;
use regex::Regex;

fn mermaid_blocks(text: &str) -> Vec<String> {
    let re = Regex::new(r"(?s)```mermaid\r?\n(.*?)```").unwrap();
    re.captures_iter(text).map(|c| c[1].to_string()).collect()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn state_diagram_matches_transition_matrix() {
    let block = mermaid_blocks(&read("docs/state-diagram.md"))
        .into_iter()
        .find(|b| b.contains("stateDiagram"))
        .expect("stateDiagram в docs/state-diagram.md");
    let aliases: HashMap<String, DocumentStatus> = Regex::new(r#"state "([^"]+)" as (\w+)"#)
        .unwrap()
        .captures_iter(&block)
        .map(|c| (c[2].to_string(), DocumentStatus::parse(&c[1]).expect("известный статус")))
        .collect();
    assert_eq!(aliases.len(), DocumentStatus::ALL.len());

    let mut drawn = BTreeSet::new();
    for c in Regex::new(r"(?m)^\s*(\S+)\s*-->\s*(\w+)\s*:\s*(.+?)\s*$").unwrap().captures_iter(&block) {
        let (Some(old), Some(new)) = (aliases.get(&c[1]), aliases.get(&c[2])) else { continue };
        let roles: BTreeSet<String> = c[3].split(',').map(|r| r.trim().to_string()).collect();
        drawn.insert((old.as_str(), new.as_str(), roles));
    }
    let mut expected = BTreeSet::new();
    for &old in DocumentStatus::ALL {
        for &new in DocumentStatus::ALL {
            if let Some(roles) = transition_roles(old, new) {
                expected.insert((old.as_str(), new.as_str(), roles.iter().map(|r| r.as_str().to_string()).collect()));
            }
        }
    }
    assert_eq!(drawn, expected);
}

/// Таблицы и столбцы из SQL-миграций.
fn schema_from_migrations() -> HashMap<String, BTreeSet<String>> {
    let mut tables = HashMap::new();
    let table_re = Regex::new(r"(?s)CREATE TABLE IF NOT EXISTS (\w+) \((.*?)\n\);").unwrap();
    let mut files: Vec<_> = std::fs::read_dir("migrations").unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    for file in files {
        let sql = read(file.to_str().unwrap());
        for c in table_re.captures_iter(&sql) {
            let columns: BTreeSet<String> = c[2]
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with("--") && !l.starts_with("CONSTRAINT") && !l.starts_with("CHECK"))
                .filter(|l| !l.starts_with('\'') && !l.starts_with('('))
                .filter_map(|l| l.split_whitespace().next())
                .map(str::to_string)
                .collect();
            tables.insert(c[1].to_string(), columns);
        }
    }
    tables
}

#[test]
fn er_diagram_matches_migrations() {
    let text = read("docs/data-schema.md");
    let block = mermaid_blocks(&text).into_iter().find(|b| b.contains("erDiagram")).expect("erDiagram");
    let entities: HashMap<String, BTreeSet<String>> = Regex::new(r"(?ms)^\s*(\w+)\s*\{(.*?)\}")
        .unwrap()
        .captures_iter(&block)
        .map(|c| {
            let columns = c[2].lines().filter_map(|l| l.split_whitespace().nth(1)).map(str::to_string).collect();
            (c[1].to_string(), columns)
        })
        .collect();
    let schema = schema_from_migrations();
    assert!(schema.len() >= 10, "разобрано таблиц: {}", schema.len());
    for (table, columns) in schema {
        let documented = entities.get(&table).unwrap_or_else(|| panic!("нет сущности {table} на ER-диаграмме"));
        assert_eq!(documented, &columns, "{table}");
    }
}
