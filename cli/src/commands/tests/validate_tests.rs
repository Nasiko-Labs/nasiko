use super::*;
use serde_json::json;

// ─── missing_card_fields ─────────────────────────────────────────────────────

/// The 0.2.x shape: everything at the card root, no `supportedInterfaces`.
fn legacy_card() -> serde_json::Value {
    json!({
        "name": "currency-agent",
        "description": "Converts currencies",
        "version": "1.0.0",
        "url": "http://localhost:8000/",
        "protocolVersion": "0.2.9",
        "preferredTransport": "JSONRPC",
        "capabilities": { "streaming": true },
        "skills": [{ "id": "convert", "name": "Convert", "description": "Converts" }],
    })
}

/// The A2A 1.0 shape, as every current SDK serialises it: the three relocated
/// fields live in `supportedInterfaces[]` and appear nowhere at the root.
fn a2a_1_0_card() -> serde_json::Value {
    json!({
        "name": "currency-agent",
        "description": "Converts currencies",
        "version": "1.0.0",
        "capabilities": { "streaming": true },
        "skills": [{ "id": "convert", "name": "Convert", "description": "Converts" }],
        "supportedInterfaces": [{
            "url": "http://localhost:8000/",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0",
        }],
    })
}

#[test]
fn a_legacy_card_with_every_field_at_the_root_still_validates() {
    assert!(missing_card_fields(&legacy_card()).is_empty());
}

#[test]
fn an_a2a_1_0_card_validates_from_supported_interfaces_alone() {
    assert!(
        missing_card_fields(&a2a_1_0_card()).is_empty(),
        "a spec-correct A2A 1.0 card must not be rejected"
    );
}

#[test]
fn a_card_carrying_both_placements_validates() {
    let mut card = a2a_1_0_card();
    card["url"] = json!("http://localhost:8000/");
    card["protocolVersion"] = json!("1.0");
    card["preferredTransport"] = json!("JSONRPC");

    assert!(missing_card_fields(&card).is_empty());
}

#[test]
fn a_card_with_neither_placement_reports_all_three_relocated_fields() {
    let mut card = a2a_1_0_card();
    card.as_object_mut()
        .expect("card literal is an object")
        .remove("supportedInterfaces");

    assert_eq!(
        missing_card_fields(&card),
        vec!["url", "protocolVersion", "preferredTransport"]
    );
}

#[test]
fn an_empty_supported_interfaces_array_is_not_a_placement() {
    let mut card = a2a_1_0_card();
    card["supportedInterfaces"] = json!([]);

    assert_eq!(
        missing_card_fields(&card),
        vec!["url", "protocolVersion", "preferredTransport"]
    );
}

#[test]
fn the_transport_is_read_as_protocol_binding_not_preferred_transport() {
    // A 1.0 interface that misspells the transport as the 0.2.x name must not
    // satisfy `preferredTransport`, or the rename is silently undone.
    let mut card = a2a_1_0_card();
    card["supportedInterfaces"] = json!([{
        "url": "http://localhost:8000/",
        "preferredTransport": "JSONRPC",
        "protocolVersion": "1.0",
    }]);

    assert_eq!(missing_card_fields(&card), vec!["preferredTransport"]);
}

#[test]
fn fields_the_spec_never_moved_are_not_satisfied_by_an_interface() {
    let mut card = a2a_1_0_card();
    card.as_object_mut()
        .expect("card literal is an object")
        .remove("name");
    card["supportedInterfaces"] = json!([{
        "name": "not the card's name",
        "url": "http://localhost:8000/",
        "protocolBinding": "JSONRPC",
        "protocolVersion": "1.0",
    }]);

    assert_eq!(missing_card_fields(&card), vec!["name"]);
}

#[test]
fn missing_fields_are_reported_in_declaration_order() {
    let card = json!({ "description": "Converts currencies" });

    assert_eq!(
        missing_card_fields(&card),
        vec![
            "name",
            "url",
            "version",
            "capabilities",
            "skills",
            "protocolVersion",
            "preferredTransport"
        ]
    );
}

#[test]
fn only_the_first_interface_is_consulted() {
    let mut card = a2a_1_0_card();
    card["supportedInterfaces"] = json!([
        { "url": "http://localhost:8000/" },
        { "protocolBinding": "JSONRPC", "protocolVersion": "1.0" },
    ]);

    assert_eq!(
        missing_card_fields(&card),
        vec!["protocolVersion", "preferredTransport"],
        "the preferred interface is the first entry, not a union of all of them"
    );
}
