use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

impl Priority {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Normal => "NORMAL",
            Self::High => "HIGH",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum Status {
    Open,
    InProgress,
    Resolved,
}

impl Status {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::InProgress => "IN_PROGRESS",
            Self::Resolved => "RESOLVED",
        }
    }

    pub(super) fn from_database(value: &str) -> Result<Self, ApiError> {
        match value {
            "OPEN" => Ok(Self::Open),
            "IN_PROGRESS" => Ok(Self::InProgress),
            "RESOLVED" => Ok(Self::Resolved),
            _ => Err(ApiError::database_unavailable()),
        }
    }

    pub(super) const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Open, Self::Open | Self::InProgress)
                | (Self::InProgress, Self::InProgress | Self::Resolved)
                | (Self::Resolved, Self::Resolved | Self::InProgress)
        )
    }
}

#[derive(Debug, Default)]
pub(super) enum Patch<T> {
    #[default]
    Absent,
    Present(T),
}

impl<T> Patch<T> {
    fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Version(pub(super) i32);

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumericVersion;
        impl de::Visitor<'_> for NumericVersion {
            type Value = Version;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an integer version between 1 and 2147483646")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                if value == 0 || value >= i32::MAX as u64 {
                    Err(E::custom("version out of range"))
                } else {
                    Ok(Version(value as i32))
                }
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                if value < 1 || value >= i64::from(i32::MAX) {
                    Err(E::custom("version out of range"))
                } else {
                    Ok(Version(value as i32))
                }
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                if !value.is_finite()
                    || value.fract() != 0.0
                    || value < 1.0
                    || value >= f64::from(i32::MAX)
                {
                    Err(E::custom("version out of range"))
                } else {
                    Ok(Version(value as i32))
                }
            }
        }
        // JSON 1.0 and 1e0 have the same integer meaning as 1 in the old API.
        deserializer.deserialize_any(NumericVersion)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Create {
    pub(super) title: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) priority: Priority,
    #[serde(default)]
    pub(super) assignee_user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Update {
    pub(super) version: Version,
    #[serde(default)]
    pub(super) title: Patch<String>,
    #[serde(default)]
    pub(super) description: Patch<String>,
    #[serde(default)]
    pub(super) priority: Patch<Priority>,
    #[serde(default)]
    pub(super) status: Patch<Status>,
    #[serde(default)]
    pub(super) assignee_user_id: Patch<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Delete {
    pub(super) version: Version,
}

fn trim_space(character: char) -> bool {
    matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn title(value: &str) -> Result<String, ApiError> {
    let value = value.trim_matches(trim_space);
    if value.is_empty() || value.chars().count() > 200 || value.contains('\0') {
        return Err(ApiError::validation());
    }
    Ok(value.to_owned())
}

fn description(value: &str) -> Result<(), ApiError> {
    if value.chars().count() > 8192 || value.contains('\0') {
        return Err(ApiError::validation());
    }
    Ok(())
}

fn parse<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::new(413, "REQUEST_LIMIT", "请求体超过 64 KiB"));
    }
    // Serde's struct visitor also accepts a positional JSON sequence. Require
    // an object before invoking it, while retaining derived duplicate-field
    // checks instead of first parsing into Value (which would lose duplicates).
    let first = body
        .iter()
        .copied()
        .find(|byte| !matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'));
    if first != Some(b'{') {
        return Err(ApiError::validation());
    }
    serde_json::from_slice(body).map_err(|_| ApiError::validation())
}

pub(super) fn create(body: &[u8]) -> Result<Create, ApiError> {
    let mut value: Create = parse(body)?;
    value.title = title(&value.title)?;
    description(&value.description)?;
    if let Some(raw) = &mut value.assignee_user_id {
        *raw = canonical_uuid(raw)?.to_string();
    }
    Ok(value)
}

pub(super) fn update(body: &[u8]) -> Result<Update, ApiError> {
    let mut value: Update = parse(body)?;
    if !value.title.is_present()
        && !value.description.is_present()
        && !value.priority.is_present()
        && !value.status.is_present()
        && !value.assignee_user_id.is_present()
    {
        return Err(ApiError::validation());
    }
    if let Patch::Present(ref mut value) = value.title {
        *value = title(value)?;
    }
    if let Patch::Present(ref value) = value.description {
        description(value)?;
    }
    if let Patch::Present(Some(raw)) = &mut value.assignee_user_id {
        *raw = canonical_uuid(raw)?.to_string();
    }
    Ok(value)
}

pub(super) fn delete(body: &[u8]) -> Result<Delete, ApiError> {
    parse(body)
}

pub(super) fn idempotency_key(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"._:-".contains(&value))
    {
        return Err(ApiError::validation());
    }
    Ok(())
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
        return Err(ApiError::validation());
    }
    Uuid::parse_str(value).map_err(|_| ApiError::validation())
}

pub(super) fn fingerprint(value: &Create) -> Result<String, ApiError> {
    // Struct field order is fixed. Defaults, title trim and missing/null
    // assignee are normalized before these bytes are hashed.
    let bytes = serde_json::to_vec(value).map_err(|_| ApiError::database_unavailable())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_hash_uses_normalized_fields_not_json_spelling_or_field_order() {
        let first = create(br#"{"title":"  x  "}"#).expect("minimal create");
        let second =
            create(br#"{"assigneeUserId":null,"priority":"NORMAL","description":"","title":"x"}"#)
                .expect("explicit defaults");
        assert_eq!(fingerprint(&first), fingerprint(&second));
        let different =
            create(br#"{"title":"x","description":"different"}"#).expect("different body");
        assert_ne!(fingerprint(&first), fingerprint(&different));
        assert_eq!(fingerprint(&first).expect("hash").len(), 64);
    }

    #[test]
    fn bodies_reject_unknown_duplicate_null_and_malformed_member_fields() {
        for body in [
            r#"{"title":"x","tenantId":"tenant"}"#,
            r#"{"title":"x","title":"y"}"#,
            r#"{"title":"x","description":null}"#,
            r#"{"title":"x","priority":null}"#,
            r#"{"title":"x","assigneeUserId":"not-a-uuid"}"#,
            r#"{"title":"x","assigneeUserId":""}"#,
            r#"{"title":null}"#,
            r#"{"title":"\u0000"}"#,
            "[]",
            r#"["x"]"#,
            r#"["x","","NORMAL",null]"#,
            "null",
            r#"{"title":"x"}{}"#,
        ] {
            assert!(create(body.as_bytes()).is_err(), "{body}");
        }
        for body in [
            r#"{"version":1}"#,
            r#"{"version":1,"title":null}"#,
            r#"{"version":1,"status":null}"#,
            r#"{"version":1,"priority":"INVALID"}"#,
            r#"{"version":1,"ownerUserId":"other"}"#,
            r#"{"version":1,"assigneeUserId":"not-a-uuid"}"#,
            r#"[1,"x"]"#,
        ] {
            assert!(update(body.as_bytes()).is_err(), "{body}");
        }
        assert!(matches!(
            update(br#"{"version":1,"assigneeUserId":null}"#)
                .expect("clear member")
                .assignee_user_id,
            Patch::Present(None)
        ));
        assert!(delete(br#"{"version":1,"title":"x"}"#).is_err());
        assert!(delete(b"[1]").is_err());
        assert!(create(b" \t\r\n{\"title\":\"x\"}").is_ok());
    }

    #[test]
    fn assignment_uuid_normalization_preserves_create_identity_and_patch_presence() {
        let lower =
            create(br#"{"title":"x","assigneeUserId":"00112233-4455-6677-8899-aabbccddeeff"}"#)
                .expect("lowercase");
        let upper =
            create(br#"{"title":"x","assigneeUserId":"00112233-4455-6677-8899-AABBCCDDEEFF"}"#)
                .expect("uppercase");
        assert_eq!(fingerprint(&lower), fingerprint(&upper));
        assert_ne!(
            fingerprint(&lower),
            fingerprint(&create(br#"{"title":"x"}"#).expect("unassigned"))
        );
        assert!(matches!(
            update(br#"{"version":1,"title":"x"}"#)
                .expect("omitted")
                .assignee_user_id,
            Patch::Absent
        ));
        match update(br#"{"version":1,"assigneeUserId":"00112233-4455-6677-8899-AABBCCDDEEFF"}"#)
            .expect("normalized patch")
            .assignee_user_id
        {
            Patch::Present(Some(value)) => {
                assert_eq!(value, "00112233-4455-6677-8899-aabbccddeeff")
            }
            _ => panic!("explicit assignment was lost"),
        }
    }

    #[test]
    fn unicode_limits_count_characters_and_body_limit_counts_bytes() {
        let body = serde_json::json!({"title": "😀".repeat(200), "description": "中".repeat(8192)});
        assert!(create(body.to_string().as_bytes()).is_ok());
        let body = serde_json::json!({"title": "😀".repeat(201)});
        assert!(create(body.to_string().as_bytes()).is_err());
        let body = serde_json::json!({"title": "x", "description": "中".repeat(8193)});
        assert!(create(body.to_string().as_bytes()).is_err());
        let mut body = br#"{"title":"x"}"#.to_vec();
        body.resize(MAX_BODY_BYTES, b' ');
        assert!(create(&body).is_ok());
        body.push(b' ');
        assert_eq!(create(&body).expect_err("too many bytes").status(), 413);
        let body = serde_json::json!({"title": "\u{feff} \t x \u{feff}"});
        assert_eq!(
            create(body.to_string().as_bytes()).expect("trim").title,
            "x"
        );
    }

    #[test]
    fn versions_accept_json_integer_semantics_without_allowing_increment_overflow() {
        for version in ["1", "1.0", "1e0", "2147483646"] {
            let body = format!("{{\"version\":{version}}}");
            assert!(delete(body.as_bytes()).is_ok(), "{version}");
        }
        for version in [
            "0",
            "-1",
            "1.1",
            "2147483647",
            "18446744073709551616",
            "\"1\"",
            "null",
        ] {
            let body = format!("{{\"version\":{version}}}");
            assert!(delete(body.as_bytes()).is_err(), "{version}");
        }
    }

    #[test]
    fn transitions_preserve_exact_old_state_machine() {
        let states = [Status::Open, Status::InProgress, Status::Resolved];
        for (from_index, from) in states.into_iter().enumerate() {
            for (to_index, to) in states.into_iter().enumerate() {
                let expected = [
                    [true, true, false],
                    [false, true, true],
                    [false, true, true],
                ][from_index][to_index];
                assert_eq!(from.can_transition_to(to), expected);
            }
        }
    }

    #[test]
    fn key_and_uuid_boundaries_are_explicit() {
        for key in ["a", "A0._:-", &"x".repeat(128)] {
            assert!(idempotency_key(key).is_ok());
        }
        for key in ["", "space key", "é", "x\ny", &"x".repeat(129)] {
            assert!(idempotency_key(key).is_err());
        }
        assert!(canonical_uuid("00112233-4455-6677-8899-AABBCCDDEEFF").is_ok());
        assert!(canonical_uuid("00112233445566778899aabbccddeeff").is_err());
    }
}
