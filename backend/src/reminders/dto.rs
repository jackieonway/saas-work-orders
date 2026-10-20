use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub(super) idempotency_key: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Payload {
    idempotency_key: String,
    target: Target,
    #[serde(rename = "type")]
    notification_type: String,
    title: String,
    content: String,
    link: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    kind: String,
    user_id: String,
    organization_id: String,
}

pub(super) struct Ticket {
    pub(super) id: Uuid,
    pub(super) number: String,
    pub(super) organization: Uuid,
    pub(super) owner: Uuid,
    pub(super) assignee: Option<Uuid>,
}

impl Ticket {
    pub(super) fn recipient(&self) -> Uuid {
        self.assignee.unwrap_or(self.owner)
    }
}

fn object(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .copied()
        .find(|byte| !matches!(*byte, b' ' | b'\t' | b'\r' | b'\n'))
        == Some(b'{')
}

pub(super) fn request(bytes: &[u8]) -> Result<Request, ApiError> {
    if bytes.len() > 65_536 {
        return Err(ApiError::new(413, "REQUEST_LIMIT", "请求体超过 64 KiB"));
    }
    if !object(bytes) {
        return Err(ApiError::validation());
    }
    let value: Request = serde_json::from_slice(bytes).map_err(|_| ApiError::validation())?;
    if !valid_key(&value.idempotency_key) {
        return Err(ApiError::validation());
    }
    Ok(value)
}

fn valid_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

pub(super) fn uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    let parsed = Uuid::parse_str(value).ok()?;
    parsed
        .hyphenated()
        .to_string()
        .eq_ignore_ascii_case(value)
        .then_some(parsed)
}

pub(super) fn payload(ticket: &Ticket, key: &str) -> Payload {
    Payload {
        idempotency_key: key.to_owned(),
        target: Target {
            kind: "USER".into(),
            user_id: ticket.recipient().to_string(),
            organization_id: ticket.organization.to_string(),
        },
        notification_type: "INFO".into(),
        title: format!("工单提醒 {}", ticket.number),
        content: format!("请查看工单 {} 的详情并处理。", ticket.number),
        link: format!("/micro/work-orders/tickets/{}", ticket.id),
    }
}

pub(super) fn stored_payload(
    json: &str,
    key: &str,
    ticket: Uuid,
    organization: Uuid,
    recipient: Uuid,
) -> Result<Payload, ApiError> {
    let invalid = ApiError::database_unavailable;
    if json.len() > 65_536 || !object(json.as_bytes()) {
        return Err(invalid());
    }
    let parsed: Payload = serde_json::from_str(json).map_err(|_| invalid())?;
    let number = parsed.title.strip_prefix("工单提醒 ").ok_or_else(invalid)?;
    if parsed.idempotency_key != key
        || !valid_key(key)
        || parsed.target.kind != "USER"
        || parsed.notification_type != "INFO"
        || parsed.target.user_id != recipient.to_string()
        || parsed.target.organization_id != organization.to_string()
        || parsed.link != format!("/micro/work-orders/tickets/{ticket}")
        || number.is_empty()
        || number.chars().count() > 40
        || number.contains('\0')
        || parsed.content != format!("请查看工单 {number} 的详情并处理。")
        || serde_json::to_string(&parsed).map_err(|_| invalid())? != json
    {
        return Err(invalid());
    }
    Ok(parsed)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Publication {
    publication_id: String,
    recipient_count: u32,
    delivery_count: u32,
    duplicate: bool,
    created_at: String,
}

impl Publication {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, ApiError> {
        let invalid = super::result_unknown;
        if !object(bytes) {
            return Err(invalid());
        }
        let mut result: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        let publication = uuid(&result.publication_id).ok_or_else(invalid)?;
        if result.recipient_count != 1
            || result.delivery_count != 1
            || result.created_at.len() > 64
            || chrono::DateTime::parse_from_rfc3339(&result.created_at).is_err()
        {
            return Err(invalid());
        }
        result.publication_id = publication.to_string();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_input_rejects_sequences_duplicates_overrides_and_invalid_keys() {
        for value in [
            r#"["event"]"#,
            r#"{"idempotencyKey":"a","idempotencyKey":"b"}"#,
            r#"{"idempotencyKey":"a","recipient":"other"}"#,
            r#"{"idempotencyKey":null}"#,
            r#"{"idempotencyKey":""}"#,
            r#"{"idempotencyKey":"has space"}"#,
            r#"{"idempotencyKey":"中文"}"#,
            "null",
        ] {
            assert!(request(value.as_bytes()).is_err(), "{value}");
        }
        for key in ["event.A_1:-", &"a".repeat(128)] {
            assert!(request(
                serde_json::json!({"idempotencyKey": key})
                    .to_string()
                    .as_bytes()
            )
            .is_ok());
        }
        assert!(request(
            serde_json::json!({"idempotencyKey": "a".repeat(129)})
                .to_string()
                .as_bytes()
        )
        .is_err());
        let mut body = br#"{"idempotencyKey":"event"}"#.to_vec();
        body.resize(65_536, b' ');
        assert!(request(&body).is_ok());
        body.push(b' ');
        assert_eq!(request(&body).expect_err("body limit").status(), 413);
    }

    fn ticket() -> Ticket {
        Ticket {
            id: Uuid::from_u128(1),
            number: "WO-000042".into(),
            organization: Uuid::from_u128(2),
            owner: Uuid::from_u128(3),
            assignee: None,
        }
    }

    #[test]
    fn fixed_event_template_and_owner_fallback_bind_the_persisted_target() {
        let mut ticket = ticket();
        let original = payload(&ticket, "event");
        let value = serde_json::to_value(&original).expect("payload");
        assert_eq!(value["target"]["userId"], ticket.owner.to_string());
        assert_eq!(value["title"], "工单提醒 WO-000042");
        assert_eq!(value["content"], "请查看工单 WO-000042 的详情并处理。");
        assert_eq!(value["type"], "INFO");
        assert_eq!(
            value["link"],
            format!("/micro/work-orders/tickets/{}", ticket.id)
        );
        ticket.assignee = Some(Uuid::from_u128(4));
        assert_ne!(payload(&ticket, "event"), original);
        let json = serde_json::to_string(&original).expect("payload bytes");
        assert!(
            stored_payload(&json, "event", ticket.id, ticket.organization, ticket.owner).is_ok()
        );
        assert!(stored_payload(
            &json,
            "event",
            ticket.id,
            ticket.organization,
            ticket.recipient()
        )
        .is_err());
        assert!(stored_payload(
            &format!(" {json}"),
            "event",
            ticket.id,
            ticket.organization,
            ticket.owner
        )
        .is_err());
    }

    #[test]
    fn publication_projection_rejects_duplicate_required_fields_and_noninteger_counts() {
        let base = serde_json::json!({"publicationId": Uuid::from_u128(1).to_string(), "recipientCount":1, "deliveryCount":1, "duplicate":false, "createdAt":"2026-10-17T12:00:00.123Z", "extra":"discard"});
        let parsed = Publication::from_bytes(base.to_string().as_bytes()).expect("publication");
        assert_eq!(
            serde_json::to_value(parsed)
                .expect("projection")
                .as_object()
                .expect("object")
                .len(),
            5
        );
        for (field, value) in [
            ("recipientCount", serde_json::json!(1.0)),
            ("deliveryCount", serde_json::json!(0)),
            ("publicationId", serde_json::json!("invalid")),
            ("duplicate", serde_json::json!("false")),
            ("createdAt", serde_json::json!("invalid")),
        ] {
            let mut changed = base.clone();
            changed[field] = value;
            assert_eq!(
                Publication::from_bytes(changed.to_string().as_bytes())
                    .expect_err("invalid response")
                    .code(),
                "REMINDER_RESULT_UNKNOWN"
            );
        }
        let duplicate = format!("{{\"duplicate\":true,{}", &base.to_string()[1..]);
        assert!(Publication::from_bytes(duplicate.as_bytes()).is_err());
        assert!(Publication::from_bytes(br#"["id",1,1,false,"2026-10-17T12:00:00Z"]"#).is_err());
    }
}
