use tanuki_protocol::*;

#[test]
fn requests_round_trip_both_codecs_without_losing_tags_or_limits() {
    let hello = ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse("native client").unwrap(),
        selectors: vec![Selector::parse("/lamp/{hue,brightness}").unwrap()],
    };
    let requests = [
        hello,
        ClientMessage::Write {
            request_id: RequestId::new("write".into()),
            operations: vec![
                WireOperation::PublishState {
                    topic: TopicPath::parse("/lamp/hue").unwrap(),
                    value: JsonValue::new(Value::Integer(i64::MAX)),
                    expiry: None,
                },
                WireOperation::PublishEvent {
                    topic: TopicPath::parse("/lamp/event").unwrap(),
                    value: JsonValue::new(Value::Bytes(vec![0, 255])),
                },
                WireOperation::DefineInput {
                    topic: TopicPath::parse("/lamp/desired").unwrap(),
                    kind: WireInputKind::Desired,
                },
                WireOperation::ClaimInput {
                    topic: TopicPath::parse("/lamp/desired").unwrap(),
                    release: WireRelease::After {
                        duration: "PT5S".into(),
                    },
                },
            ],
        },
    ];
    for request in requests {
        assert_eq!(
            serde_json::from_slice::<ClientMessage>(&serde_json::to_vec(&request).unwrap())
                .unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientMessage>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }
    let bad = r#"{"type":"hello","request_id":"h","client":"bad/name","selectors":[]}"#;
    assert!(serde_json::from_str::<ClientMessage>(bad).is_err());
    let bad =
        r#"{"type":"write","request_id":"w","operations":[{"op":"remove_node","topic":"/bad/*"}]}"#;
    assert!(serde_json::from_str::<ClientMessage>(bad).is_err());
}

#[test]
fn null_payload_presence_and_discriminator_order_are_preserved() {
    let request = r#"{"operations":[{"value":null,"topic":"/lamp/desired","op":"submit_desired"}],"request_id":"w","type":"write"}"#;
    let ClientMessage::Write { operations, .. } = serde_json::from_str(request).unwrap() else {
        panic!()
    };
    assert!(
        matches!(&operations[0],WireOperation::SubmitDesired { value, .. } if value.as_inner() == &Value::Null)
    );
    let missing = r#"{"op":"submit_desired","topic":"/lamp/desired"}"#;
    assert!(serde_json::from_str::<WireOperation>(missing).is_err());
    assert!(serde_json::from_str::<NodeView>(r#"{"kind":"state","current":null}"#).is_err());
    assert!(matches!(
        serde_json::from_str::<ServerMessage>(r#"{"result":null,"request_id":"w","type":"reply"}"#)
            .unwrap(),
        ServerMessage::Reply {
            result: serde_json::Value::Null,
            ..
        }
    ));
}
