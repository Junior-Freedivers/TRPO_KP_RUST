//! Генерация PDF-отчетов (genpdf) из данных БД. Для кириллицы нужен TTF-шрифт (DejaVu Sans).

use std::path::Path;
use std::sync::OnceLock;

use chrono::DateTime;
use chrono_tz::Tz;
use genpdf::elements::{Break, FrameCellDecorator, LinearLayout, PaddedElement, Paragraph, TableLayout};
use genpdf::fonts::{FontData, FontFamily};
use genpdf::style::{Color, Style};
use genpdf::{Alignment, Element, Margins, Position, RenderResult, Size};

const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", // Debian/Ubuntu (fonts-dejavu-core)
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", // AlmaLinux/Fedora
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "C:/Windows/Fonts/arial.ttf",
];

/// Ширина страницы A4 (альбомная) за вычетом полей, мм.
const PAGE_WIDTH_MM: f64 = 297.0;
const PAGE_HEIGHT_MM: f64 = 210.0;
const MARGIN_MM: f64 = 12.0;
const HEADER_COLOR: Color = Color::Rgb(0x2F, 0x4B, 0x6E);

#[derive(Debug, Clone)]
pub struct ReportData {
    pub title: String,
    pub period: String,
    pub generated_at: DateTime<Tz>,
    pub generated_by: String,
    pub columns: Vec<String>,
    pub col_weights: Vec<f64>,
    pub rows: Vec<Vec<String>>,
    pub filters: Vec<String>,
    pub summary: Vec<(String, String)>,
}

fn find_font(configured: Option<&str>) -> Result<String, String> {
    configured
        .into_iter()
        .chain(FONT_CANDIDATES.iter().copied())
        .find(|p| Path::new(p).is_file())
        .map(str::to_string)
        .ok_or_else(|| "Не найден TTF-шрифт с кириллицей: установите fonts-dejavu-core или задайте PDF_FONT_PATH".into())
}

fn load_family(configured: Option<&str>) -> Result<FontFamily<FontData>, String> {
    let regular_path = find_font(configured)?;
    let bold_candidate = regular_path.replace("DejaVuSans.ttf", "DejaVuSans-Bold.ttf").replace("arial.ttf", "arialbd.ttf");
    let bold_path = if Path::new(&bold_candidate).is_file() { bold_candidate } else { regular_path.clone() };
    let read = |path: &str| std::fs::read(path).map_err(|e| format!("Не удалось прочитать шрифт {path}: {e}"));
    let font = |bytes: Vec<u8>, builtin| FontData::new(bytes, builtin).map_err(|e| format!("Некорректный шрифт: {e}"));
    let (regular, bold) = (read(&regular_path)?, read(&bold_path)?);
    // курсив в отчетах не используется: чтобы не встраивать TTF еще дважды, для него указан встроенный
    // шрифт PDF (метрики берутся из того же файла)
    Ok(FontFamily {
        italic: font(regular.clone(), Some(printpdf::BuiltinFont::HelveticaOblique))?,
        bold_italic: font(bold.clone(), Some(printpdf::BuiltinFont::HelveticaBoldOblique))?,
        regular: font(regular, None)?,
        bold: font(bold, None)?,
    })
}

/// Шрифты загружаются один раз за время работы процесса.
fn font_family(configured: Option<&str>) -> Result<FontFamily<FontData>, String> {
    static FAMILY: OnceLock<Result<FontFamily<FontData>, String>> = OnceLock::new();
    FAMILY.get_or_init(|| load_family(configured)).clone()
}

/// Разбивает слишком длинные слова: genpdf без переносов молча обрезает слово шире колонки.
fn breakable(text: &str, max_chars: usize) -> String {
    text.split(' ')
        .map(|word| {
            let chars: Vec<char> = word.chars().collect();
            if chars.len() <= max_chars {
                word.to_string()
            } else {
                chars.chunks(max_chars).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cell(text: &str, max_chars: usize, style: Style) -> PaddedElement<Paragraph> {
    Paragraph::new(genpdf::style::StyledString::new(breakable(text, max_chars), style))
        .padded(Margins::trbl(1, 1.2, 1, 1.2))
}

/// Таблица, у которой строка заголовка повторяется на каждой странице.
struct RepeatingHeaderTable {
    columns: Vec<String>,
    weights: Vec<usize>,
    max_chars: Vec<usize>,
    body: TableLayout,
}

impl RepeatingHeaderTable {
    fn header(&self) -> TableLayout {
        let mut table = TableLayout::new(self.weights.clone());
        table.set_cell_decorator(FrameCellDecorator::new(true, true, false));
        let style = Style::new().bold().with_font_size(8).with_color(HEADER_COLOR);
        let mut row = table.row();
        for (title, max_chars) in self.columns.iter().zip(&self.max_chars) {
            row.push_element(cell(title, *max_chars, style));
        }
        row.push().expect("число колонок заголовка");
        table
    }
}

impl Element for RepeatingHeaderTable {
    fn render(
        &mut self,
        context: &genpdf::Context,
        mut area: genpdf::render::Area<'_>,
        style: Style,
    ) -> Result<RenderResult, genpdf::error::Error> {
        let header = self.header().render(context, area.clone(), style)?;
        area.add_offset(Position::new(0, header.size.height));
        let mut body = self.body.render(context, area, style)?;
        body.size.height += header.size.height;
        body.size.width = body.size.width.max(header.size.width);
        Ok(body)
    }
}

fn weights(col_weights: &[f64]) -> Vec<usize> {
    col_weights.iter().map(|w| ((w * 10.0).round() as usize).max(1)).collect()
}

/// Сколько символов шрифта 8 pt помещается в колонку (с запасом).
fn max_chars_per_column(weights: &[usize], width_mm: f64) -> Vec<usize> {
    let total: usize = weights.iter().sum::<usize>().max(1);
    weights.iter().map(|w| (((width_mm * *w as f64 / total as f64) - 3.0) / 1.9).floor().max(4.0) as usize).collect()
}

pub fn render_report(data: &ReportData, font_path: Option<&str>) -> Result<Vec<u8>, String> {
    let family = font_family(font_path)?;
    let mut doc = genpdf::Document::new(family);
    doc.set_title(&data.title);
    doc.set_paper_size(Size::new(PAGE_WIDTH_MM, PAGE_HEIGHT_MM));
    doc.set_font_size(9);
    doc.set_line_spacing(1.15);

    let title = data.title.clone();
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(Margins::trbl(MARGIN_MM - 4.0, MARGIN_MM, MARGIN_MM, MARGIN_MM));
    decorator.set_header(move |page| {
        let mut table = TableLayout::new(vec![4, 1]);
        let style = Style::new().with_font_size(8).with_color(Color::Greyscale(110));
        table
            .row()
            .element(Paragraph::new(genpdf::style::StyledString::new(title.clone(), style)))
            .element(Paragraph::new(genpdf::style::StyledString::new(format!("Стр. {page}"), style)).aligned(Alignment::Right))
            .push()
            .expect("колонтитул");
        table.padded(Margins::trbl(0, 0, 3, 0))
    });
    doc.set_page_decorator(decorator);

    let meta = Style::new().with_font_size(9);
    let mut head = LinearLayout::vertical();
    head.push(Paragraph::new(genpdf::style::StyledString::new(data.title.clone(), Style::new().bold().with_font_size(15))));
    head.push(Break::new(0.3));
    head.push(Paragraph::new(genpdf::style::StyledString::new(format!("Период: {}", data.period), meta)));
    let filters = if data.filters.is_empty() { "не заданы".to_string() } else { data.filters.join("; ") };
    head.push(Paragraph::new(genpdf::style::StyledString::new(format!("Фильтры: {filters}"), meta)));
    head.push(Paragraph::new(genpdf::style::StyledString::new(
        format!(
            "Дата формирования: {}. Сформировал: {}",
            data.generated_at.format("%d.%m.%Y %H:%M"),
            data.generated_by
        ),
        meta,
    )));
    head.push(Break::new(1.0));
    doc.push(head);

    let content_width = PAGE_WIDTH_MM - 2.0 * MARGIN_MM;
    if data.rows.is_empty() {
        doc.push(Paragraph::new(genpdf::style::StyledString::new(
            "Нет данных за выбранный период и заданные фильтры.",
            meta,
        )));
    } else {
        let weights = weights(&data.col_weights);
        let max_chars = max_chars_per_column(&weights, content_width);
        let mut body = TableLayout::new(weights.clone());
        body.set_cell_decorator(FrameCellDecorator::new(true, true, true));
        let cell_style = Style::new().with_font_size(8);
        for values in &data.rows {
            let mut row = body.row();
            for (value, limit) in values.iter().zip(&max_chars) {
                row.push_element(cell(value, *limit, cell_style));
            }
            row.push().map_err(|e| e.to_string())?;
        }
        doc.push(RepeatingHeaderTable { columns: data.columns.clone(), weights, max_chars, body });
    }

    if !data.summary.is_empty() {
        doc.push(Break::new(1.0));
        doc.push(Paragraph::new(genpdf::style::StyledString::new("Итоговые показатели", Style::new().bold().with_font_size(11))));
        doc.push(Break::new(0.3));
        // таблица итогов занимает 65% ширины (45% + 20%), как в прежней версии
        let mut summary = TableLayout::new(vec![45, 20, 35]);
        summary.set_cell_decorator(SummaryDecorator(FrameCellDecorator::new(true, true, false)));
        let style = Style::new().with_font_size(8);
        for (key, value) in &data.summary {
            summary
                .row()
                .element(cell(key, 60, style))
                .element(cell(value, 30, style))
                .element(Paragraph::new(""))
                .push()
                .map_err(|e| e.to_string())?;
        }
        doc.push(summary);
    }

    let mut buffer = Vec::new();
    doc.render(&mut buffer).map_err(|e| format!("Ошибка формирования PDF: {e}"))?;
    Ok(compress(buffer))
}

/// Сжатие потоков (Flate): genpdf пишет шрифты и содержимое страниц без сжатия.
fn compress(pdf: Vec<u8>) -> Vec<u8> {
    let Ok(mut document) = lopdf::Document::load_mem(&pdf) else { return pdf };
    document.compress();
    let mut out = Vec::with_capacity(pdf.len() / 2);
    match document.save_to(&mut out) {
        Ok(()) if out.starts_with(b"%PDF") => out,
        _ => pdf,
    }
}

/// Рамки только у двух первых колонок таблицы итогов (третья — пустой отступ).
struct SummaryDecorator(FrameCellDecorator);

impl genpdf::elements::CellDecorator for SummaryDecorator {
    fn set_table_size(&mut self, num_columns: usize, num_rows: usize) {
        self.0.set_table_size(num_columns - 1, num_rows);
    }

    fn decorate_cell(&mut self, column: usize, row: usize, has_more: bool, area: genpdf::render::Area<'_>, style: Style) {
        if column < 2 {
            self.0.decorate_cell(column, row, has_more, area, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_words_are_split() {
        assert_eq!(breakable("abcdefgh ij", 3), "abc def gh ij");
        assert_eq!(breakable("ВХ-2026-000001", 20), "ВХ-2026-000001");
    }

    #[test]
    fn renders_cyrillic_report_when_font_available() {
        if find_font(None).is_err() {
            eprintln!("шрифт не найден — тест пропущен");
            return;
        }
        let rows: Vec<Vec<String>> = (0..80)
            .map(|i| vec![format!("ВХ-2026-{i:06}"), "Очень длинное краткое содержание документа ".repeat(4)])
            .collect();
        let data = ReportData {
            title: "Отчет по входящим документам".into(),
            period: "01.09.2026 — 30.09.2026".into(),
            generated_at: chrono::Utc::now().with_timezone(&chrono_tz::Europe::Moscow),
            generated_by: "Иванова А. С. (делопроизводитель)".into(),
            columns: vec!["Рег. номер".into(), "Краткое содержание".into()],
            col_weights: vec![1.7, 4.2],
            rows,
            filters: vec![],
            summary: vec![("Всего документов".into(), "80".into())],
        };
        let pdf = render_report(&data, None).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(pdf.len() > 1000);
        // для визуальной проверки: REPORT_SAMPLE_PATH=/tmp/sample.pdf cargo test renders_cyrillic
        if let Ok(path) = std::env::var("REPORT_SAMPLE_PATH") {
            std::fs::write(path, &pdf).unwrap();
        }
    }
}
