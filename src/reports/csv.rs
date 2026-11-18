//! CSV-выгрузка: UTF-8 с BOM, разделитель «;», защита от CSV-инъекций (формулы в Excel).

/// Значение ячейки CSV.
pub enum CsvValue {
    Text(String),
    Number(i64),
}

impl From<String> for CsvValue {
    fn from(value: String) -> Self {
        CsvValue::Text(value)
    }
}

impl From<&str> for CsvValue {
    fn from(value: &str) -> Self {
        CsvValue::Text(value.to_string())
    }
}

impl From<i64> for CsvValue {
    fn from(value: i64) -> Self {
        CsvValue::Number(value)
    }
}

const BOM: &str = "\u{feff}";
const FORMULA_PREFIXES: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

fn escape(value: &str) -> String {
    if value.contains([';', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn cell(value: &CsvValue) -> String {
    match value {
        CsvValue::Number(n) => n.to_string(),
        CsvValue::Text(s) if s.starts_with(FORMULA_PREFIXES) => escape(&format!("'{s}")),
        CsvValue::Text(s) => escape(s),
    }
}

pub fn render_csv(headers: &[&str], rows: &[Vec<CsvValue>]) -> Vec<u8> {
    let mut out = String::from(BOM);
    out.push_str(&headers.iter().map(|h| escape(h)).collect::<Vec<_>>().join(";"));
    out.push_str("\r\n");
    for row in rows {
        out.push_str(&row.iter().map(cell).collect::<Vec<_>>().join(";"));
        out.push_str("\r\n");
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_delimiter_and_quoting() {
        let csv = render_csv(&["А", "Б"], &[vec!["x;y".into(), "он сказал \"да\"".into()], vec!["=1+2".into(), 5.into()]]);
        let text = String::from_utf8(csv).unwrap();
        assert!(text.starts_with('\u{feff}'));
        assert_eq!(text, "\u{feff}А;Б\r\n\"x;y\";\"он сказал \"\"да\"\"\"\r\n'=1+2;5\r\n");
    }

    #[test]
    fn formula_prefixes_are_neutralized() {
        for prefix in ["=", "+", "-", "@"] {
            let csv = String::from_utf8(render_csv(&["a"], &[vec![format!("{prefix}cmd").into()]])).unwrap();
            assert!(csv.contains(&format!("'{prefix}cmd")));
        }
        let negative = String::from_utf8(render_csv(&["a"], &[vec![(-5i64).into()]])).unwrap();
        assert!(negative.ends_with("-5\r\n"));
    }
}
