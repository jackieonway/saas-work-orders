use crate::error::ApiError;

use super::Ticket;

pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn byte_limit() -> ApiError {
    ApiError::export_limit()
}

struct Csv {
    text: String,
}

impl Csv {
    fn new() -> Self {
        Self {
            text: String::from('\u{feff}'),
        }
    }

    fn push(&mut self, value: &str) -> Result<(), ApiError> {
        if self
            .text
            .len()
            .checked_add(value.len())
            .is_none_or(|size| size > MAX_BYTES)
        {
            return Err(byte_limit());
        }
        self.text.push_str(value);
        Ok(())
    }

    fn cell(&mut self, value: &str) -> Result<(), ApiError> {
        self.push("\"")?;
        // Spreadsheet formulas may be preceded by whitespace or control bytes.
        // Quote the original value, with an apostrophe inside the CSV quoting.
        let trimmed = value.trim_start_matches(|character: char| {
            character.is_whitespace() || character.is_control() || character == '\u{feff}'
        });
        if value.starts_with(|character: char| character.is_control())
            || matches!(trimmed.chars().next(), Some('=' | '+' | '-' | '@'))
        {
            self.push("'")?;
        }
        let mut pieces = value.split('"');
        if let Some(first) = pieces.next() {
            self.push(first)?;
        }
        for piece in pieces {
            self.push("\"\"")?;
            self.push(piece)?;
        }
        self.push("\"")
    }

    fn line(&mut self, values: &[&str]) -> Result<(), ApiError> {
        for (index, value) in values.iter().enumerate() {
            if index != 0 {
                self.push(",")?;
            }
            self.cell(value)?;
        }
        self.push("\r\n")
    }
}

pub(super) fn encode(records: &[Ticket]) -> Result<Vec<u8>, ApiError> {
    let mut csv = Csv::new();
    csv.line(&[
        "编号",
        "标题",
        "描述",
        "优先级",
        "状态",
        "组织",
        "创建人",
        "处理人",
        "创建时间",
        "更新时间",
    ])?;
    for record in records {
        let priority = match record.priority.as_str() {
            "LOW" => "低",
            "NORMAL" => "普通",
            "HIGH" => "高",
            _ => return Err(ApiError::database_unavailable()),
        };
        let status = match record.status.as_str() {
            "OPEN" => "待处理",
            "IN_PROGRESS" => "处理中",
            "RESOLVED" => "已解决",
            _ => return Err(ApiError::database_unavailable()),
        };
        csv.line(&[
            &record.number,
            &record.title,
            &record.description,
            priority,
            status,
            &record.organization_name,
            &record.owner_display_name,
            record.assignee_display_name.as_deref().unwrap_or_default(),
            &record.created_at,
            &record.updated_at,
        ])?;
    }
    Ok(csv.text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(input: &str) -> String {
        let mut csv = Csv::new();
        csv.cell(input).expect("small CSV cell");
        csv.text
    }

    #[test]
    fn formula_and_control_prefixes_are_neutralized_inside_csv_quotes() {
        for input in [
            "=1+1",
            " +2",
            "\r@cmd",
            "\nplain",
            "\tplain",
            "\u{000b}plain",
            "\u{feff}=1",
            "\u{001f} -2",
        ] {
            assert_eq!(cell(input), format!("\u{feff}\"'{input}\""), "{input:?}");
        }
        for input in ["plain", "1+2", "中文 😀", "", "  ordinary"] {
            assert_eq!(cell(input), format!("\u{feff}\"{input}\""), "{input:?}");
        }
    }

    #[test]
    fn encoding_keeps_bom_crlf_and_quotes_embedded_csv_syntax() {
        let mut csv = Csv::new();
        csv.line(&["a,b", "双\"引号", "line\r\nbreak", "😀"])
            .expect("small record");
        assert_eq!(csv.text.as_bytes().get(..3), Some(&[0xef, 0xbb, 0xbf][..]));
        assert_eq!(
            csv.text,
            "\u{feff}\"a,b\",\"双\"\"引号\",\"line\r\nbreak\",\"😀\"\r\n"
        );
        let empty = String::from_utf8(encode(&[]).expect("header")).expect("UTF-8");
        assert!(empty.starts_with("\u{feff}\"编号\",\"标题\""));
        assert!(empty.ends_with("\r\n"));
    }

    #[test]
    fn human_columns_translate_enums_and_neutralize_every_text_snapshot() {
        let record = Ticket {
            id: String::new(),
            number: "WO-42".into(),
            title: "=title".into(),
            description: " +description".into(),
            priority: "HIGH".into(),
            status: "IN_PROGRESS".into(),
            tenant_id: String::new(),
            organization_id: String::new(),
            organization_name: "-organization".into(),
            owner_user_id: String::new(),
            owner_display_name: "@owner".into(),
            assignee_user_id: None,
            assignee_display_name: None,
            version: 1,
            created_at: "2026-10-17T00:00:00.000Z".into(),
            updated_at: "2026-10-17T01:00:00.000Z".into(),
        };
        let csv = String::from_utf8(encode(&[record]).expect("small ticket")).expect("UTF-8 CSV");
        assert!(csv.contains("\"WO-42\",\"'=title\",\"' +description\",\"高\",\"处理中\",\"'-organization\",\"'@owner\",\"\""));
        assert!(csv.ends_with("\"2026-10-17T00:00:00.000Z\",\"2026-10-17T01:00:00.000Z\"\r\n"));
    }

    #[test]
    fn exact_limit_counts_utf8_bom_quote_expansion_and_line_endings() {
        let mut csv = Csv::new();
        let value = "x".repeat(MAX_BYTES - 7);
        csv.line(&[&value]).expect("exact 4 MiB");
        assert_eq!(csv.text.len(), MAX_BYTES);
        assert!(csv.push("x").is_err());

        let mut csv = Csv::new();
        let quotes = "\"".repeat(MAX_BYTES / 2);
        assert!(
            csv.cell(&quotes).is_err(),
            "double-quote expansion must count"
        );
        assert!(csv.text.len() <= MAX_BYTES);

        let mut csv = Csv::new();
        let chinese = "中".repeat(MAX_BYTES / 3);
        assert!(
            csv.cell(&chinese).is_err(),
            "limit is bytes, not characters"
        );
        assert!(csv.text.len() <= MAX_BYTES);
    }
}
