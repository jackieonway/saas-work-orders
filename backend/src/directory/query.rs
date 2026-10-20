use uuid::Uuid;

use crate::{authorization::Action, error::ApiError};

pub(super) struct Query {
    pub(super) action: Action,
    pub(super) organization: Option<Uuid>,
    pub(super) page: u32,
    pub(super) size: u32,
    pub(super) q: String,
}

pub(super) fn trim(value: &str) -> &str {
    value.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
            '\u{205f}' | '\u{3000}' | '\u{feff}'
        )
    })
}

fn decode(value: &str) -> Result<String, ApiError> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let pair = bytes
                    .get(index + 1..index + 3)
                    .ok_or_else(ApiError::validation)?;
                let high = (pair[0] as char)
                    .to_digit(16)
                    .ok_or_else(ApiError::validation)?;
                let low = (pair[1] as char)
                    .to_digit(16)
                    .ok_or_else(ApiError::validation)?;
                decoded.push((high * 16 + low) as u8);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    let value = String::from_utf8(decoded).map_err(|_| ApiError::validation())?;
    if value.contains('\0') {
        return Err(ApiError::validation());
    }
    Ok(value)
}

pub(super) fn uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    let id = Uuid::parse_str(value).ok()?;
    id.hyphenated()
        .to_string()
        .eq_ignore_ascii_case(value)
        .then_some(id)
}

fn positive(value: &str, maximum: u32) -> Result<u32, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::validation());
    }
    let value: u32 = value.parse().map_err(|_| ApiError::validation())?;
    if value == 0 || value > maximum {
        return Err(ApiError::validation());
    }
    Ok(value)
}

pub(super) fn parse(raw: Option<&str>) -> Result<Query, ApiError> {
    let raw = raw.unwrap_or_default();
    if raw.len() > 8192 {
        return Err(ApiError::validation());
    }
    let mut action = None;
    let mut query = Query {
        action: Action::Create,
        organization: None,
        page: 1,
        size: 20,
        q: String::new(),
    };
    let mut seen = 0_u8;
    for part in raw.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        let flag = match key.as_str() {
            "action" => {
                action = Some(match value.as_str() {
                    "create" => Action::Create,
                    "update" => Action::Update,
                    _ => return Err(ApiError::validation()),
                });
                1
            }
            "organizationId" => {
                query.organization = Some(uuid(&value).ok_or_else(ApiError::validation)?);
                2
            }
            "page" => {
                query.page = positive(&value, 1_000_000)?;
                4
            }
            "size" => {
                query.size = positive(&value, 50)?;
                8
            }
            "q" => {
                // The old API limits the original text before trimming.
                if value.chars().count() > 100 {
                    return Err(ApiError::validation());
                }
                query.q = trim(&value).to_owned();
                16
            }
            _ => return Err(ApiError::validation()),
        };
        if seen & flag != 0 {
            return Err(ApiError::validation());
        }
        seen |= flag;
    }
    query.action = action.ok_or_else(ApiError::validation)?;
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_fields_and_pagination_match_directory_contract() {
        let value =
            parse(Some("action=update&page=1000000&size=50&q=%25_%5C")).expect("valid query");
        assert_eq!(
            (value.action, value.page, value.size, value.q.as_str()),
            (Action::Update, 1_000_000, 50, "%_\\")
        );
        for query in [
            "",
            "action=read",
            "action=create&action=update",
            "action=create&%61ction=create",
            "action=create&tenantId=other",
            "action=create&resource=ticket",
            "action=create&size=51",
            "action=create&page=18446744073709551616",
            "action=create&page=0",
            "action=create&page=1000001",
            "action=create&q=%FF",
            "action=create&q=%00",
            "action=create&q=%G0",
            "action=create&q=%",
            "action=create&organizationId=00112233445566778899aabbccddeeff",
        ] {
            assert!(parse(Some(query)).is_err(), "{query}");
        }
        let value = parse(Some("action=create")).expect("defaults");
        assert_eq!((value.page, value.size), (1, 20));
    }

    #[test]
    fn unicode_bound_is_applied_before_javascript_compatible_trim() {
        assert!(parse(Some(&format!("action=create&q={}", "😀".repeat(100)))).is_ok());
        assert!(parse(Some(&format!("action=create&q=+{}+", "x".repeat(99)))).is_err());
        let value = parse(Some("action=create&q=%EF%BB%BF+hello+%EF%BB%BF")).expect("BOM trim");
        assert_eq!(value.q, "hello");
    }
}
