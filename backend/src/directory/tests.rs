use super::*;

fn organization() -> Uuid {
    Uuid::from_u128(1)
}

fn member_value() -> Value {
    serde_json::json!({ "id": Uuid::from_u128(2).to_string(), "organizationId": organization().to_string(), "displayName": "成员 😀" })
}

#[test]
fn platform_list_is_projected_to_minimal_normalized_business_members() {
    let value = serde_json::json!({ "records": [member_value()], "total": 1, "page": 1, "size": 20, "privateExtra": "discarded" });
    let mapped = serde_json::to_value(page(&value, organization(), 1, 20).expect("valid page"))
        .expect("serialize");
    assert_eq!(
        mapped,
        serde_json::json!({ "records": [{ "userId": Uuid::from_u128(2).to_string(), "organizationId": organization().to_string(), "displayName": "成员 😀" }], "total": 1, "page": 1, "size": 20 })
    );
}

#[test]
fn malformed_or_wrong_target_member_data_fails_closed() {
    for (field, value) in [
        ("id", serde_json::json!("not-a-uuid")),
        (
            "organizationId",
            serde_json::json!(Uuid::from_u128(3).to_string()),
        ),
        ("displayName", serde_json::json!(null)),
        ("displayName", serde_json::json!("\u{feff} \t")),
        ("displayName", serde_json::json!("x\u{0}y")),
        ("displayName", serde_json::json!("😀".repeat(101))),
    ] {
        let mut item = member_value();
        item[field] = value;
        assert_eq!(
            member(&item, organization())
                .expect_err("invalid member")
                .status(),
            503
        );
    }
}

#[test]
fn upstream_pagination_and_total_must_be_exact_bounded_integers() {
    let original =
        serde_json::json!({ "records": [member_value()], "total": 1, "page": 1, "size": 20 });
    for (field, value) in [
        ("page", serde_json::json!(2)),
        ("size", serde_json::json!(21)),
        ("total", serde_json::json!(-1)),
        ("total", serde_json::json!(1.5)),
        ("total", serde_json::json!(9_007_199_254_740_992_u64)),
        ("total", serde_json::json!("1")),
        ("records", serde_json::json!({})),
        ("records", Value::Array(vec![member_value(); 21])),
    ] {
        let mut malformed = original.clone();
        malformed[field] = value;
        assert!(page(&malformed, organization(), 1, 20).is_err(), "{field}");
    }
    let mut numeric = original;
    numeric["page"] = serde_json::json!(1.0);
    assert!(page(&numeric, organization(), 1, 20).is_ok());
}

#[test]
fn member_witness_binds_every_identity_action_and_target_without_renewing_ttl() {
    let start = Instant::now();
    let deadline = start + std::time::Duration::from_secs(3);
    let member = VerifiedMember {
        actor: Uuid::from_u128(1),
        tenant: Uuid::from_u128(2),
        current_organization: Uuid::from_u128(3),
        organization: Uuid::from_u128(4),
        user: Uuid::from_u128(5),
        action: Action::Update,
        display_name: "verified".into(),
        deadline,
    };
    let (actor, tenant, context, org, user) = (
        member.actor,
        member.tenant,
        member.current_organization,
        member.organization,
        member.user,
    );
    assert!(member.matches(actor, tenant, context, org, user, Action::Update));
    let other = Uuid::from_u128(99);
    for (actor, tenant, context, org, user, action) in [
        (other, tenant, context, org, user, Action::Update),
        (actor, other, context, org, user, Action::Update),
        (actor, tenant, other, org, user, Action::Update),
        (actor, tenant, context, other, user, Action::Update),
        (actor, tenant, context, org, other, Action::Update),
        (actor, tenant, context, org, user, Action::Create),
    ] {
        assert!(!member.matches(actor, tenant, context, org, user, action));
    }
    assert!(member.assert_deadline(start).is_ok());
    assert!(member
        .assert_deadline(deadline - std::time::Duration::from_nanos(1))
        .is_ok());
    assert_eq!(
        member
            .assert_deadline(deadline)
            .expect_err("exact TTL")
            .code(),
        "MEMBER_VALIDATION_EXPIRED"
    );
    assert!(member
        .assert_deadline(deadline + std::time::Duration::from_secs(1))
        .is_err());
    assert_eq!(
        member.deadline, deadline,
        "observing a witness must never renew it"
    );
}
