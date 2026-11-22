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
                claim: Some(ClaimView {
                    id: 2,
                    owner: "lamp controller".parse().unwrap(),
                    session: 3,
                    release: ClaimReleaseView::After {
                        duration: "PT1M30S".parse().unwrap(),
                    },
                }),
                current: Some(CurrentValueView {
                    value: Value::Integer(70),
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

#[test]
fn write_replies_are_typed_commits() {
    let message = ServerMessage::Reply {
        request_id: RequestId::from("w1"),
        result: CommitView {
            sequence: 43,
            warnings: vec![DiagnosticView::NodeKindChanged {
                topic: "/lamp".parse().unwrap(),
                previous: NodeKind::State,
                replacement: NodeKind::Desired,
            }],
        },
    };
    let expected = json!({
        "type": "reply",
        "request_id": "w1",
        "result": {"sequence": 43, "warnings": [{"code": "node_kind_changed", "topic": "/lamp",
                                                 "previous": "state", "replacement": "desired"}]}
    });
    assert_eq!(serde_json::to_value(&message).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<ServerMessage>(expected).unwrap(),
        message
    );
}

#[test]
fn durations_have_typed_constructors_with_the_wire_text() {
    use std::time::Duration;
    assert_eq!(
        WireExpiry::after(Duration::from_secs(90)),
        WireExpiry::Set {
            duration: "PT1M30S".into()
        }
    );
    assert_eq!(
        WireRelease::after(Duration::from_millis(1500)),
        WireRelease::After {
            duration: "PT1.5S".into()
        }
    );
    assert_eq!(RequestId::from("a"), RequestId::new("a".into()));
    assert_eq!(RequestId::from(String::from("b")).as_str(), "b");
}
