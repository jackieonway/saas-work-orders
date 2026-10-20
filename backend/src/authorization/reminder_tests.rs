use serde_json::{json, Value};
use uuid::Uuid;

use super::{policy, ReminderAuthorized, Scope};

fn fixture() -> (i64, Value, Value) {
    let now = policy::now_millis();
    let (user, tenant, org, app) = (
        Uuid::from_u128(1),
        Uuid::from_u128(2),
        Uuid::from_u128(3),
        Uuid::from_u128(4),
    );
    let intro = json!({"active":true,"aud":"work-orders","sub":user.to_string(),"tenant_id":tenant.to_string(),"organization_id":org.to_string(),"token_type":"Bearer","exp":now/1000+120});
    let context = json!({"contractVersion":"1.0","appCode":"work-orders","contextVersion":"a".repeat(64),
        "expiresAt":chrono::DateTime::from_timestamp_millis(now+20_000).unwrap().to_rfc3339(),
        "user":{"id":user.to_string(),"username":"actor"},"tenant":{"id":tenant.to_string()},
        "organization":{"id":org.to_string(),"name":"Organization"},"application":{"id":app.to_string()},
        "organizationScopeIds":[org.to_string(),Uuid::from_u128(5).to_string()],
        "permissions":["ticket:read","notification:publish"],"dataGrants":[
            {"resource":"ticket","action":"read","tenantId":tenant.to_string(),"subjectUserId":user.to_string(),"scope":"SELF","organizationIds":[]},
            {"resource":"notification","action":"publish","tenantId":tenant.to_string(),"subjectUserId":user.to_string(),"scope":"TENANT","organizationIds":[]}]});
    (now, intro, context)
}

fn authorize(
    intro: &Value,
    context: &Value,
    now: i64,
) -> Result<ReminderAuthorized, crate::error::ApiError> {
    policy::validate_reminder(&policy::validate_intro(intro, now)?, context, now)
}

#[test]
fn reminder_requires_both_operations_and_independent_identity_bound_grants() {
    let (now, intro, original) = fixture();
    for permission in ["ticket:read", "notification:publish"] {
        let mut context = original.clone();
        context["permissions"]
            .as_array_mut()
            .unwrap()
            .retain(|item| item != permission);
        assert_eq!(
            authorize(&intro, &context, now).err().unwrap().code(),
            "OPERATION_DENIED"
        );
    }
    for index in [0, 1] {
        let mut context = original.clone();
        context["dataGrants"].as_array_mut().unwrap().remove(index);
        assert_eq!(
            authorize(&intro, &context, now).err().unwrap().code(),
            "DATA_DENIED"
        );
    }
    for field in ["subjectUserId", "tenantId"] {
        let mut context = original.clone();
        context["dataGrants"][1][field] = json!(Uuid::from_u128(99).to_string());
        assert_eq!(
            authorize(&intro, &context, now).err().unwrap().code(),
            "DATA_DENIED"
        );
    }
    let mut context = original;
    let duplicate = context["dataGrants"][1].clone();
    context["dataGrants"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert_eq!(
        authorize(&intro, &context, now).err().unwrap().code(),
        "DATA_DENIED"
    );
}

#[test]
fn notification_scopes_intersect_hard_ceiling_without_expanding_ticket_self() {
    let (now, intro, original) = fixture();
    let (actor, current, child, peer) = (
        Uuid::from_u128(1),
        Uuid::from_u128(3),
        Uuid::from_u128(5),
        Uuid::from_u128(6),
    );
    for (scope, ids, child_allowed, other_allowed) in [
        ("SELF", json!([]), false, false),
        ("ORG", json!([current.to_string()]), false, true),
        (
            "ORG_SUBTREE",
            json!([current.to_string(), child.to_string(), peer.to_string()]),
            true,
            true,
        ),
        ("TENANT", json!([]), true, true),
    ] {
        let mut context = original.clone();
        context["dataGrants"][1]["scope"] = json!(scope);
        context["dataGrants"][1]["organizationIds"] = ids;
        let auth = authorize(&intro, &context, now).unwrap();
        assert_eq!(auth.read().scope(), Scope::SelfOnly);
        assert_eq!(auth.application_id(), Uuid::from_u128(4));
        assert!(auth.assert_recipient(current, actor).is_ok());
        assert_eq!(
            auth.assert_recipient(current, Uuid::from_u128(99)).is_ok(),
            other_allowed
        );
        assert_eq!(auth.assert_recipient(child, actor).is_ok(), child_allowed);
        assert!(auth.assert_recipient(peer, actor).is_err());
    }
}

#[test]
fn notification_missing_ceiling_fails_closed_and_explicit_null_is_preserved() {
    let (now, intro, mut context) = fixture();
    context
        .as_object_mut()
        .unwrap()
        .remove("organizationScopeIds");
    assert!(authorize(&intro, &context, now)
        .unwrap()
        .assert_recipient(Uuid::from_u128(5), Uuid::from_u128(1))
        .is_err());
    context["organizationScopeIds"] = Value::Null;
    assert!(authorize(&intro, &context, now)
        .unwrap()
        .assert_recipient(Uuid::from_u128(6), Uuid::from_u128(99))
        .is_ok());
}
