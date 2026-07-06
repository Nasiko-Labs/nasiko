//! Integration tests for Role ordering and serde.

use nasiko_auth::Role;

// ─── Ordering ────────────────────────────────────────────────────────────────

#[test]
fn admin_is_greater_than_all_other_roles() {
    assert!(Role::Admin > Role::DepartmentManager);
    assert!(Role::Admin > Role::TeamLead);
    assert!(Role::Admin > Role::TeamMember);
    assert!(Role::Admin > Role::Member);
}

#[test]
fn department_manager_ordering() {
    assert!(Role::DepartmentManager > Role::TeamLead);
    assert!(Role::DepartmentManager > Role::TeamMember);
    assert!(Role::DepartmentManager > Role::Member);
    assert!(Role::DepartmentManager < Role::Admin);
}

#[test]
fn team_lead_ordering() {
    assert!(Role::TeamLead > Role::TeamMember);
    assert!(Role::TeamLead > Role::Member);
    assert!(Role::TeamLead < Role::DepartmentManager);
    assert!(Role::TeamLead < Role::Admin);
}

#[test]
fn team_member_ordering() {
    assert!(Role::TeamMember > Role::Member);
    assert!(Role::TeamMember < Role::TeamLead);
}

#[test]
fn member_is_smallest_role() {
    assert!(Role::Member < Role::TeamMember);
    assert!(Role::Member < Role::TeamLead);
    assert!(Role::Member < Role::DepartmentManager);
    assert!(Role::Member < Role::Admin);
}

#[test]
fn role_is_equal_to_itself() {
    assert_eq!(Role::Admin, Role::Admin);
    assert_eq!(Role::DepartmentManager, Role::DepartmentManager);
    assert_eq!(Role::TeamLead, Role::TeamLead);
    assert_eq!(Role::TeamMember, Role::TeamMember);
    assert_eq!(Role::Member, Role::Member);
}

#[test]
fn role_ge_and_le_comparisons() {
    assert!(Role::Admin >= Role::Admin);
    assert!(Role::Member <= Role::Member);
    assert!(Role::TeamLead >= Role::Member);
    assert!(Role::Member <= Role::TeamLead);
}

// ─── Serialization ───────────────────────────────────────────────────────────

#[test]
fn role_serializes_to_snake_case_strings() {
    let cases = [
        (Role::Admin, "\"admin\""),
        (Role::DepartmentManager, "\"department_manager\""),
        (Role::TeamLead, "\"team_lead\""),
        (Role::TeamMember, "\"team_member\""),
        (Role::Member, "\"member\""),
    ];
    for (role, expected_json) in cases {
        let serialized = serde_json::to_string(&role).unwrap();
        assert_eq!(
            serialized, expected_json,
            "role {:?} serialized incorrectly: got {serialized}",
            role
        );
    }
}

#[test]
fn role_deserializes_from_snake_case_strings() {
    let cases = [
        ("\"admin\"", Role::Admin),
        ("\"department_manager\"", Role::DepartmentManager),
        ("\"team_lead\"", Role::TeamLead),
        ("\"team_member\"", Role::TeamMember),
        ("\"member\"", Role::Member),
    ];
    for (json, expected) in cases {
        let role: Role = serde_json::from_str(json).unwrap();
        assert_eq!(role, expected, "failed to deserialize {json}");
    }
}

#[test]
fn role_serde_roundtrip() {
    let roles = [
        Role::Admin,
        Role::DepartmentManager,
        Role::TeamLead,
        Role::TeamMember,
        Role::Member,
    ];
    for role in roles {
        let json = serde_json::to_string(&role).unwrap();
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role, "serde roundtrip failed for {:?}", role);
    }
}

#[test]
fn role_deserialize_invalid_string_fails() {
    let result: Result<Role, _> = serde_json::from_str("\"superadmin\"");
    assert!(result.is_err(), "unknown role variant should fail to deserialize");
}

// ─── Clone + Debug ────────────────────────────────────────────────────────────

#[test]
fn role_clone_produces_equal_value() {
    let original = Role::TeamLead;
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn role_debug_output_is_non_empty() {
    assert!(!format!("{:?}", Role::Admin).is_empty());
}
