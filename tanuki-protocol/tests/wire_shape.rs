//! Typed view fields (paths, names, timestamps, durations, kinds, codes) keep
//! the exact wire text the earlier string fields produced.
use serde_json::json;
use tanuki_protocol::*;

fn provenance() -> ProvenanceView {
    ProvenanceView {
        client: "lamp controller".parse().unwrap(),
        session: Some(3),
        at: "2023-11-14T22:13:20Z".parse().unwrap(),
    }
}

#[test]
fn typed_view_fields_serialize_to_the_established_json() {
    let message = ServerMessage::Snapshot {
        request_id: RequestId::new("h".into()),
        sequence: 7,
        nodes: [(
            "/lamp/desired".parse().unwrap(),
            NodeView::Desired {
                definition: InputDefinitionView {},
                claim: Some(ClaimView {
                    id: 2,
                    owner: "lamp controller".parse().unwrap(),
                    session: 3,
                    release: ClaimReleaseView::After {
                        duration: "PT1M30S".parse().unwrap(),
                    },
                }),
                current: Some(CurrentValueView {
                    value: WireValue::new(Value::Integer(70)),
                    last_write: provenance(),
                    expires_at: Some("2023-11-14T22:23:20.5Z".parse().unwrap()),
                }),
            },
        )]
        .into(),
        warnings: vec![DiagnosticView::NodeKindChanged {
            topic: "/lamp/desired".parse().unwrap(),
            previous: NodeKind::State,
            replacement: NodeKind::Desired,
        }],
    };
    let expected = json!({
        "type": "snapshot",
        "request_id": "h",
        "sequence": 7,
        "nodes": {"/lamp/desired": {
            "kind": "desired",
            "definition": {},
            "claim": {"id": 2, "owner": "lamp controller", "session": 3,
                      "release": {"mode": "after", "duration": "PT1M30S"}},
            "current": {"value": 70,
                        "last_write": {"client": "lamp controller", "session": 3,
                                       "at": "2023-11-14T22:13:20Z"},
                        "expires_at": "2023-11-14T22:23:20.5Z"}
        }},
        "warnings": [{"code": "node_kind_changed", "topic": "/lamp/desired",
                      "previous": "state", "replacement": "desired"}]
    });
    assert_eq!(serde_json::to_value(&message).unwrap(), expected);
    let decoded: ServerMessage = serde_json::from_value(expected).unwrap();
    assert_eq!(decoded, message);
    let packed = rmp_serde::to_vec_named(&message).unwrap();
    assert_eq!(
        decode_messagepack::<ServerMessage>(&packed).unwrap(),
        message
    );
}

#[test]
fn error_codes_are_snake_case_and_unknown_codes_still_decode() {
    let error = ErrorView {
        code: ErrorCode::SessionRequired,
        message: "claiming needs a managed session".into(),
    };
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"code": "session_required", "message": "claiming needs a managed session"})
    );
    let future: ErrorView =
        serde_json::from_value(json!({"code": "added_later", "message": "new"})).unwrap();
    assert_eq!(future.code, ErrorCode::Unknown);
}
