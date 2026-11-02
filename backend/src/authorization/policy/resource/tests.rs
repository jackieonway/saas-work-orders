use super::*;
use serde_json::json;

const ISSUER: &str = "https://auth.saas.test";
const USER: &str = "11111111-1111-7111-8111-111111111111";
const TENANT: &str = "22222222-2222-7222-8222-222222222222";
const ORG: &str = "33333333-3333-7333-8333-333333333333";
const APP: &str = "44444444-4444-7444-8444-444444444444";
const REF: &str = "55555555-5555-7555-8555-555555555555";

fn user(now: i64) -> Value {
    let end = chrono::DateTime::from_timestamp_millis(now + 20_000)
        .unwrap()
        .to_rfc3339();
    json!({"contractVersion":"resource-authorization/1","issuer":ISSUER,"tokenUse":"access_token","subjectMode":"USER",
      "resource":"work-orders","action":"read","targetApplicationId":APP,"audience":"work-orders","expiresAt":end,
      "clientId":REF,"sourceApplicationId":REF,
      "principal":{"userId":USER,"membershipId":REF,"memberType":"EMPLOYEE","identityVersion":"1","accountVersion":"1","tenantId":TENANT,"organizationId":ORG},
      "introspection":{"active":true,"scope":"tickets:approved-read","client_id":REF,"token_type":"Bearer","token_use":"access_token","iss":ISSUER,"aud":"work-orders","exp":now/1000+120,"sub":"pairwise-opaque-subject-with-no-platform-UUID"},
      "context":{"contractVersion":"1.0","appCode":"work-orders","contextVersion":"a".repeat(64),"expiresAt":end,
        "user":{"id":USER,"username":"reader"},"tenant":{"id":TENANT},"organization":{"id":ORG,"name":"Organization"},"application":{"id":APP},
        "permissions":["ticket:read"],"dataGrants":[{"resource":"ticket","action":"read","tenantId":TENANT,"subjectUserId":USER,"scope":"ORG","organizationIds":[ORG]}]}})
}

fn parse(value: &Value, now: i64) -> Result<Authorized, ApiError> {
    validate_authorization(
        value,
        Action::Read,
        now,
        Some(ISSUER),
        Some("tickets:approved-read"),
    )
}

#[test]
fn opaque_subject_uses_private_principal_and_exact_deployed_scope_binding() {
    let now = now_millis();
    let base = user(now);
    let auth = parse(&base, now).unwrap();
    assert_eq!(auth.user_id().unwrap().to_string(), USER);
    assert!(validate_authorization(
        &base,
        Action::Read,
        now,
        None,
        Some("tickets:approved-read")
    )
    .is_err());
    assert!(validate_authorization(&base, Action::Read, now, Some(ISSUER), None).is_err());
    assert!(validate_authorization(
        &base,
        Action::Read,
        now,
        Some(ISSUER),
        Some("work-orders:read")
    )
    .is_err());
    for (field, value) in [
        ("tokenUse", json!("id_token")),
        ("resource", json!("stores")),
        ("audience", json!("stores")),
        ("issuer", json!("https://other.test")),
        ("targetApplicationId", json!(REF)),
    ] {
        let mut changed = base.clone();
        changed[field] = value;
        assert!(parse(&changed, now).is_err(), "{field}");
    }
    for (field, value) in [
        ("userId", json!(REF)),
        ("memberType", json!("CUSTOMER")),
        ("identityVersion", json!("0")),
        ("accountVersion", json!("01")),
    ] {
        let mut changed = base.clone();
        changed["principal"][field] = value;
        assert!(parse(&changed, now).is_err(), "{field}");
    }
}

fn machine(now: i64) -> Value {
    let end = chrono::DateTime::from_timestamp_millis(now + 20_000)
        .unwrap()
        .to_rfc3339();
    json!({"contractVersion":"resource-authorization/1","issuer":ISSUER,"tokenUse":"access_token","subjectMode":"MACHINE",
      "clientId":REF,"sourceApplicationId":REF,"targetApplicationId":APP,"resource":"work-orders","action":"read","audience":"work-orders","scope":["tickets:approved-read"],"credentialVersion":"1","expiresAt":end,
      "delegation":{"delegationId":REF,"version":"1","targetApplicationId":APP,"tenantId":TENANT,"organizationId":ORG,"resource":"ticket","action":"read","scope":"ORG","organizationIds":[ORG],"expiresAt":end}})
}

#[test]
fn machine_has_no_person_and_only_current_explicit_non_self_scope() {
    let now = now_millis();
    let base = machine(now);
    let auth = parse(&base, now).unwrap();
    assert!(auth.user_id().is_err());
    assert!(auth.organization_id().is_err());
    assert!(auth.require_person().is_err());
    assert_eq!(auth.filter_organization().unwrap().to_string(), ORG);
    for field in [
        "principal",
        "context",
        "sub",
        "introspection",
        "user",
        "member",
    ] {
        let mut changed = base.clone();
        changed[field] = Value::Null;
        assert!(parse(&changed, now).is_err(), "{field}");
    }
    for action in [Action::Create, Action::Update, Action::Delete] {
        assert!(validate_authorization(
            &base,
            action,
            now,
            Some(ISSUER),
            Some("tickets:approved-read")
        )
        .is_err());
    }
    let mut changed = base.clone();
    changed["delegation"]["scope"] = json!("SELF");
    assert!(parse(&changed, now).is_err());
    let mut changed = base.clone();
    changed["delegation"]["organizationIds"] = json!([ORG, ORG]);
    assert!(parse(&changed, now).is_err());
    let mut changed = base.clone();
    changed["delegation"]["tenantId"] = json!(Uuid::nil().to_string());
    assert!(parse(&changed, now).is_err());
    let mut tenant = base;
    tenant["delegation"]["scope"] = json!("TENANT");
    tenant["delegation"]["organizationId"] = Value::Null;
    tenant["delegation"]["organizationIds"] = json!([]);
    let auth = parse(&tenant, now).unwrap();
    assert_eq!(auth.scope(), Scope::Tenant);
    assert_eq!(auth.filter_organization(), None);
}

#[test]
fn original_authority_recheck_never_substitutes_a_changed_binding() {
    let now = now_millis();
    let base = user(now);
    let original = parse(&base, now).unwrap();
    let same = parse(&base, now).unwrap();
    assert!(original.same_authority(&same));
    let mut changed = base;
    changed["principal"]["identityVersion"] = json!("2");
    let latest = parse(&changed, now).unwrap();
    assert!(!original.same_authority(&latest));
    let mut machine = machine(now);
    machine["delegation"]["version"] = json!("2");
    assert!(!parse(&machine, now).unwrap().same_authority(&original));
}

#[test]
fn external_reminder_requires_separate_scope_permission_and_data_grant() {
    let now = now_millis();
    let mut value = user(now);
    value["introspection"]["scope"] =
        json!("tickets:approved-read work-orders:notification:publish");
    value["context"]["permissions"]
        .as_array_mut()
        .unwrap()
        .push(json!("notification:publish"));
    value["context"]["dataGrants"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "resource":"notification","action":"publish","tenantId":TENANT,
            "subjectUserId":USER,"scope":"ORG","organizationIds":[ORG]
        }));
    let authorize = |wire: &Value, publish_scope| {
        validate_reminder_authorization(
            wire,
            now,
            Some(ISSUER),
            Some("tickets:approved-read"),
            publish_scope,
        )
    };
    let auth = authorize(&value, Some("work-orders:notification:publish")).unwrap();
    assert_eq!(auth.source_binding().len(), 64);
    assert!(auth
        .assert_recipient(uuid(ORG).unwrap(), uuid(USER).unwrap())
        .is_ok());
    assert!(authorize(&value, None).is_err());
    assert!(authorize(&value, Some("unapproved:publish")).is_err());
    let mut missing = value.clone();
    missing["introspection"]["scope"] = json!("tickets:approved-read");
    assert!(authorize(&missing, Some("work-orders:notification:publish")).is_err());
    let mut missing = value.clone();
    missing["context"]["permissions"] = json!(["ticket:read"]);
    assert_eq!(
        authorize(&missing, Some("work-orders:notification:publish"))
            .err()
            .unwrap()
            .code(),
        "OPERATION_DENIED"
    );
    let mut wrong_subject = value;
    wrong_subject["context"]["dataGrants"][1]["subjectUserId"] = json!(REF);
    assert!(authorize(&wrong_subject, Some("work-orders:notification:publish")).is_err());
    assert_eq!(
        authorize(&machine(now), Some("work-orders:notification:publish"))
            .err()
            .unwrap()
            .code(),
        "OPERATION_DENIED"
    );
}
