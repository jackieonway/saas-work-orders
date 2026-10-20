use uuid::Uuid;

use crate::error::ApiError;

const MAX_QUERY_BYTES: usize = 8192;

#[derive(Debug, Default)]
pub(super) struct Filters {
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) organization_id: Option<Uuid>,
    pub(super) pattern: Option<String>,
}

#[derive(Debug)]
pub(super) struct ListQuery {
    pub(super) filters: Filters,
    pub(super) page: u32,
    pub(super) size: u32,
    pub(super) offset: i64,
}

fn invalid() -> ApiError {
    ApiError::validation()
}

fn trim_space(character: char) -> bool {
    // Preserve JavaScript String.trim, including BOM and excluding U+0085.
    matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// Decode without the replacement characters accepted by URLSearchParams: an
/// invalid byte sequence cannot silently search for a different value.
fn decode(value: &str) -> Result<String, ApiError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let pair = bytes.get(index + 1..index + 3).ok_or_else(invalid)?;
                let high = (pair[0] as char).to_digit(16).ok_or_else(invalid)?;
                let low = (pair[1] as char).to_digit(16).ok_or_else(invalid)?;
                decoded.push((high * 16 + low) as u8);
                index += 2;
            }
            value => decoded.push(value),
        }
        index += 1;
    }
    let text = String::from_utf8(decoded).map_err(|_| invalid())?;
    // PostgreSQL text cannot contain NUL, even when it arrived via %00.
    if text.contains('\0') {
        return Err(invalid());
    }
    Ok(text)
}

pub(super) fn canonical_uuid(value: &str) -> Result<Uuid, ApiError> {
    if value.len() != 36
        || value.bytes().enumerate().any(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte != b'-'
            } else {
                !byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(invalid());
    }
    Uuid::parse_str(value).map_err(|_| invalid())
}

fn positive(value: &str, maximum: u32) -> Result<u32, ApiError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let value = value.parse::<u32>().map_err(|_| invalid())?;
    if value == 0 || value > maximum {
        return Err(invalid());
    }
    Ok(value)
}

fn literal_pattern(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            output.push('\\');
        }
        output.push(character);
    }
    output.push('%');
    output
}

fn parse(raw: Option<&str>, paged: bool) -> Result<ListQuery, ApiError> {
    let raw = raw.unwrap_or_default();
    if raw.len() > MAX_QUERY_BYTES {
        return Err(invalid());
    }
    let mut query = ListQuery {
        filters: Filters::default(),
        page: 1,
        size: 20,
        offset: 0,
    };
    let mut seen = 0_u8;
    for part in raw.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        let flag = match key.as_str() {
            "q" => {
                let value = value.trim_matches(trim_space);
                if value.chars().count() > 200 {
                    return Err(invalid());
                }
                query.filters.pattern = (!value.is_empty()).then(|| literal_pattern(value));
                1
            }
            "status" if ["OPEN", "IN_PROGRESS", "RESOLVED"].contains(&value.as_str()) => {
                query.filters.status = Some(value);
                2
            }
            "priority" if ["LOW", "NORMAL", "HIGH"].contains(&value.as_str()) => {
                query.filters.priority = Some(value);
                4
            }
            "organizationId" => {
                query.filters.organization_id = Some(canonical_uuid(&value)?);
                8
            }
            "page" if paged => {
                query.page = positive(&value, 1_000_000)?;
                16
            }
            "size" if paged => {
                query.size = positive(&value, 100)?;
                32
            }
            _ => return Err(invalid()),
        };
        if seen & flag != 0 {
            return Err(invalid());
        }
        seen |= flag;
    }
    query.offset = i64::from(query.page)
        .checked_sub(1)
        .and_then(|page| page.checked_mul(i64::from(query.size)))
        .ok_or_else(invalid)?;
    Ok(query)
}

pub(super) fn list(raw: Option<&str>) -> Result<ListQuery, ApiError> {
    parse(raw, true)
}

pub(super) fn export(raw: Option<&str>) -> Result<Filters, ApiError> {
    Ok(parse(raw, false)?.filters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_bounds_reject_overflow_and_keep_largest_offset_exact() {
        let query = list(Some("page=1000000&size=100")).expect("accepted upper bounds");
        assert_eq!(
            (query.page, query.size, query.offset),
            (1_000_000, 100, 99_999_900)
        );
        for input in [
            "page=0",
            "page=1000001",
            "page=18446744073709551616",
            "page=-1",
            "page=%2B1",
            "page=1.0",
            "size=101",
            "size=0",
            "size=",
            "page=1e4",
        ] {
            assert!(list(Some(input)).is_err(), "{input}");
        }
        let query = list(None).expect("default pagination");
        assert_eq!((query.page, query.size, query.offset), (1, 20, 0));
    }

    #[test]
    fn identities_unknown_duplicate_and_malformed_fields_fail_closed() {
        for input in [
            "tenantId=other",
            "ownerUserId=other",
            "page=1&page=2",
            "q=x&%71=y",
            "organizationId=00112233445566778899aabbccddeeff",
            "status=open",
            "priority=URGENT",
            "q=%FF",
            "q=%00",
            "q=%",
            "q=%G0",
            "q=%0",
            "=x",
        ] {
            assert!(list(Some(input)).is_err(), "{input}");
        }
        assert!(export(Some("page=1")).is_err());
        assert!(export(Some("size=20")).is_err());
        assert!(canonical_uuid("00112233-4455-6677-8899-AABBCCDDEEFF").is_ok());
    }

    #[test]
    fn unicode_and_literal_search_preserve_meaning() {
        let query = list(Some("q=+%25_%5C%27+你好+%F0%9F%98%80+")).expect("literal search");
        assert_eq!(
            query.filters.pattern.as_deref(),
            Some("%\\%\\_\\\\' 你好 😀%")
        );
        assert!(list(Some(&format!("q={}", "😀".repeat(200)))).is_ok());
        assert!(list(Some(&format!("q={}", "😀".repeat(201)))).is_err());
        assert_eq!(
            list(Some("q=+%20+")).expect("empty search").filters.pattern,
            None
        );
        assert_eq!(
            list(Some("q=%EF%BB%BFquoted%EF%BB%BF"))
                .expect("BOM whitespace")
                .filters
                .pattern
                .as_deref(),
            Some("%quoted%")
        );
    }
}
