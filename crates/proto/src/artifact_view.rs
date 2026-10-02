//! Workflow artifacts as plain data: the parsing of table and metrics
//! content, and a markdown rendering for viewers (the phone) that show every
//! artifact kind through their markdown pipeline.

use serde::Deserialize;
use serde_json::Value;

use crate::ArtifactKind;

/// `1234567` → `1,234,567`; floats keep up to 4 decimals; null is a dash.
pub fn format_value(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                group_digits(&i.to_string())
            } else if let Some(u) = n.as_u64() {
                group_digits(&u.to_string())
            } else {
                let f = n.as_f64().unwrap_or_default();
                let mut s = format!("{f:.4}");
                while s.ends_with('0') {
                    s.pop();
                }
                if s.ends_with('.') {
                    s.pop();
                }
                match s.split_once('.') {
                    Some((int, frac)) => format!("{}.{frac}", group_digits(int)),
                    None => group_digits(&s),
                }
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn group_digits(s: &str) -> String {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", s),
    };
    if digits.len() <= 4 {
        // 1999 reads better ungrouped (years, ids)
        return s.to_owned();
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Right-aligned: every non-null cell of the column is a number.
    pub numeric: Vec<bool>,
    /// Suggested pixel width per column.
    pub widths: Vec<f32>,
}

pub fn parse_table(json: &str) -> Result<TableData, String> {
    #[derive(Deserialize)]
    struct Raw {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    }
    let raw: Raw = serde_json::from_str(json).map_err(|e| format!("not a table: {e}"))?;
    let n = raw.columns.len();
    let mut numeric = vec![true; n];
    let mut longest: Vec<usize> = raw.columns.iter().map(|c| c.chars().count()).collect();
    let rows: Vec<Vec<String>> = raw
        .rows
        .iter()
        .map(|row| {
            (0..n)
                .map(|c| {
                    let cell = row.get(c).unwrap_or(&Value::Null);
                    if !matches!(cell, Value::Number(_) | Value::Null) {
                        numeric[c] = false;
                    }
                    let text = format_value(cell);
                    longest[c] = longest[c].max(text.chars().take(80).count());
                    text
                })
                .collect()
        })
        .collect();
    let widths = longest
        .iter()
        .map(|chars| (*chars as f32 * 7.4 + 28.0).clamp(72.0, 320.0))
        .collect();
    Ok(TableData {
        columns: raw.columns,
        rows,
        numeric,
        widths,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricTile {
    pub label: String,
    pub value: String,
    pub unit: Option<String>,
}

pub fn parse_metrics(json: &str) -> Result<Vec<MetricTile>, String> {
    #[derive(Deserialize)]
    struct Raw {
        label: String,
        #[serde(default)]
        value: Value,
        #[serde(default)]
        unit: Option<String>,
    }
    let raw: Vec<Raw> = serde_json::from_str(json).map_err(|e| format!("not metrics: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|m| MetricTile {
            label: m.label,
            value: format_value(&m.value),
            unit: m.unit.filter(|u| !u.is_empty()),
        })
        .collect())
}

/// Longest file artifact rendered, in lines.
pub const FILE_MAX_LINES: usize = 2000;

/// Table rows a markdown rendering keeps (the rest are counted).
pub const MARKDOWN_TABLE_ROWS: usize = 60;
/// Longest cell in a markdown rendering, in characters.
const CELL_CHARS: usize = 80;

fn cell(text: &str) -> String {
    let flat = crate::view::single_line(text).replace('|', "\\|");
    crate::truncate_chars(&flat, CELL_CHARS)
}

/// One artifact page as markdown, so a viewer needs no per-kind widgets:
/// markdown as is, a table as a pipe table (first [`MARKDOWN_TABLE_ROWS`]
/// rows, then a count), metrics as a label / value table, a text file as a
/// fenced block (first `max_lines`). `text` is `None` for binary content.
pub fn to_markdown(
    kind: ArtifactKind,
    content_type: &str,
    text: Option<&str>,
    max_lines: usize,
) -> Result<String, String> {
    let Some(text) = text else {
        return Ok("_Binary content: open it on a computer._".to_owned());
    };
    match kind {
        ArtifactKind::Markdown => Ok(text.to_owned()),
        ArtifactKind::Table => {
            let t = parse_table(text)?;
            if t.columns.is_empty() {
                return Ok("_Empty table._".to_owned());
            }
            let mut out = String::new();
            out.push_str(&format!(
                "| {} |\n",
                t.columns
                    .iter()
                    .map(|c| cell(c))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            out.push_str(&format!(
                "|{}|\n",
                t.numeric
                    .iter()
                    .map(|n| if *n { " ---: " } else { " --- " })
                    .collect::<Vec<_>>()
                    .join("|")
            ));
            for row in t.rows.iter().take(MARKDOWN_TABLE_ROWS) {
                out.push_str(&format!(
                    "| {} |\n",
                    row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
                ));
            }
            if t.rows.len() > MARKDOWN_TABLE_ROWS {
                out.push_str(&format!(
                    "\n_{} more rows on a computer._",
                    t.rows.len() - MARKDOWN_TABLE_ROWS
                ));
            }
            Ok(out)
        }
        ArtifactKind::Metrics => {
            let tiles = parse_metrics(text)?;
            if tiles.is_empty() {
                return Ok("_No metrics._".to_owned());
            }
            let mut out = String::from("| Metric | Value |\n| --- | ---: |\n");
            for m in tiles.iter().take(MARKDOWN_TABLE_ROWS) {
                let value = match &m.unit {
                    Some(u) => format!("{} {u}", m.value),
                    None => m.value.clone(),
                };
                out.push_str(&format!("| {} | {} |\n", cell(&m.label), cell(&value)));
            }
            Ok(out)
        }
        ArtifactKind::File => {
            if content_type == "application/octet-stream" {
                return Ok("_Binary content: open it on a computer._".to_owned());
            }
            let cap = max_lines.min(FILE_MAX_LINES);
            let lines: Vec<&str> = text.lines().take(cap + 1).collect();
            let cut = lines.len() > cap;
            let shown = lines[..lines.len().min(cap)].join("\n");
            // A fence the content cannot close early.
            let mut fence = "```".to_owned();
            while shown.contains(&fence) {
                fence.push('`');
            }
            let mut out = format!("{fence}\n{shown}\n{fence}");
            if cut {
                out.push_str("\n\n_The rest is on a computer._");
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_thousands_and_trim_fractions() {
        let f = |s: &str| format_value(&serde_json::from_str(s).unwrap());
        assert_eq!(f("1234567"), "1,234,567");
        assert_eq!(f("-98765"), "-98,765");
        assert_eq!(f("1999"), "1999", "short numbers (years, ids) stay whole");
        assert_eq!(f("12.50"), "12.5");
        assert_eq!(f("3.0"), "3");
        assert_eq!(f("1234567.8912"), "1,234,567.8912");
        assert_eq!(f("0.000049"), "0");
        assert_eq!(f("null"), "—");
        assert_eq!(f("true"), "true");
        assert_eq!(f("\"hi\""), "hi");
        assert_eq!(f("[1,2]"), "[1,2]");
    }

    #[test]
    fn a_table_parses_with_numeric_columns_and_widths() {
        let t = parse_table(
            r#"{"columns":["Region","Revenue","Note"],
                "rows":[["EMEA",1200000,"strong"],["APAC",null,"n/a"],["AMER",980000.5,null]]}"#,
        )
        .unwrap();
        assert_eq!(t.columns, ["Region", "Revenue", "Note"]);
        assert_eq!(t.rows[0], ["EMEA", "1,200,000", "strong"]);
        assert_eq!(t.rows[1][1], "—");
        assert_eq!(t.rows[2][1], "980,000.5");
        assert_eq!(t.numeric, [false, true, false]);
        assert!(t.widths.iter().all(|w| (72.0..=320.0).contains(w)));
        // short rows are padded, never panic
        let t = parse_table(r#"{"columns":["a","b"],"rows":[["x"]]}"#).unwrap();
        assert_eq!(t.rows[0], ["x", "—"]);
        assert!(parse_table("{}").is_err());
        assert!(parse_table("not json").is_err());
    }

    #[test]
    fn long_cells_cap_their_column_width() {
        let long = "x".repeat(500);
        let t = parse_table(&format!(r#"{{"columns":["c"],"rows":[["{long}"]]}}"#)).unwrap();
        assert_eq!(t.widths, [320.0]);
    }

    #[test]
    fn metrics_parse_with_optional_units() {
        let m = parse_metrics(
            r#"[{"label":"Files","value":412},{"label":"Coverage","value":87.5,"unit":"%"},{"label":"x","value":"n/a","unit":""}]"#,
        )
        .unwrap();
        assert_eq!(m[0].value, "412");
        assert_eq!(m[0].unit, None);
        assert_eq!(m[1].value, "87.5");
        assert_eq!(m[1].unit.as_deref(), Some("%"));
        assert_eq!(m[2].unit, None, "an empty unit is no unit");
        assert!(parse_metrics("{}").is_err());
    }

    #[test]
    fn a_table_becomes_a_markdown_table_with_a_row_cap() {
        let rows: Vec<String> = (0..80)
            .map(|i| format!(r#"["r{i}", {}]"#, i * 1000))
            .collect();
        let json = format!(
            r#"{{"columns":["Name","Bytes"],"rows":[{}]}}"#,
            rows.join(",")
        );
        let md = to_markdown(ArtifactKind::Table, "application/json", Some(&json), 100).unwrap();
        assert!(md.starts_with("| Name | Bytes |\n| --- | ---: |\n| r0 | 0 |"));
        assert!(md.contains("| r59 | 59,000 |"));
        assert!(!md.contains("| r60 |"));
        assert!(md.ends_with("_20 more rows on a computer._"));
    }

    #[test]
    fn cells_cannot_break_the_table() {
        let json = r#"{"columns":["a|b"],"rows":[["x | y\nz"]]}"#;
        let md = to_markdown(ArtifactKind::Table, "application/json", Some(json), 10).unwrap();
        assert_eq!(md, "| a\\|b |\n| --- |\n| x \\| y z |\n");
    }

    #[test]
    fn metrics_and_files_render_as_markdown() {
        let md = to_markdown(
            ArtifactKind::Metrics,
            "application/json",
            Some(r#"[{"label":"Coverage","value":87.5,"unit":"%"},{"label":"Files","value":120400}]"#),
            10,
        )
        .unwrap();
        assert_eq!(
            md,
            "| Metric | Value |\n| --- | ---: |\n| Coverage | 87.5 % |\n| Files | 120,400 |\n"
        );
        let file = to_markdown(ArtifactKind::File, "text/plain", Some("a\nb\nc\nd"), 2).unwrap();
        assert_eq!(file, "```\na\nb\n```\n\n_The rest is on a computer._");
        // Content with its own fence gets a longer one.
        let fenced =
            to_markdown(ArtifactKind::File, "text/plain", Some("```\nx\n```"), 10).unwrap();
        assert!(fenced.starts_with("````\n```"));
        let bin = to_markdown(
            ArtifactKind::File,
            "application/octet-stream",
            Some("zz"),
            10,
        )
        .unwrap();
        assert!(bin.contains("Binary"));
        assert!(
            to_markdown(ArtifactKind::Markdown, "text/markdown", None, 10)
                .unwrap()
                .contains("Binary")
        );
        assert_eq!(
            to_markdown(ArtifactKind::Markdown, "text/markdown", Some("# hi"), 10).unwrap(),
            "# hi"
        );
        assert!(to_markdown(ArtifactKind::Table, "application/json", Some("oops"), 10).is_err());
    }
}
