use tanuki_protocol::*;

#[test]
fn requests_round_trip_both_codecs_without_losing_tags_or_limits() {
    let hello = ClientMessage::Hello {
        request_id: RequestId::new("hello".into()),
        client: ClientName::parse("native client").unwrap(),
        selectors: vec![Selector::parse("/lamp/{hue,brightness}").unwrap()],
        session: SessionMode::Managed,
    };
    let requests = [
        hello,
        ClientMessage::Hello {
            request_id: RequestId::new("tab".into()),
            client: ClientName::parse("dashboard").unwrap(),
            selectors: vec![],
            session: SessionMode::Stateless,
        },
        ClientMessage::Select {
            request_id: RequestId::new("select".into()),
            selectors: vec![Selector::parse("/battery/*").unwrap()],
        },
        ClientMessage::Write {
            request_id: RequestId::new("write".into()),
            operations: vec![
                WireOperation::PublishState {
                    topic: TopicPath::parse("/lamp/hue").unwrap(),
                    value: Value::Integer(i64::MAX),
                    expiry: None,
                },
                WireOperation::PublishEvent {
                    topic: TopicPath::parse("/lamp/event").unwrap(),
                    value: Value::Bytes(vec![0, 255]),
                },
                WireOperation::DefineInput {
                    topic: TopicPath::parse("/lamp/desired").unwrap(),
                    kind: WireInputKind::Desired,
                },
                WireOperation::ClaimInput {
                    topic: TopicPath::parse("/lamp/desired").unwrap(),
                    release: WireRelease::After {
                        duration: "PT5S".parse().unwrap(),
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
        let mut bytes = rmp_serde::to_vec_named(&request).unwrap();
        assert_eq!(
            decode_messagepack::<ClientMessage>(&bytes).unwrap(),
            request
        );
        bytes.push(0xc0);
        assert!(decode_messagepack::<ClientMessage>(&bytes).is_err());
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
        matches!(&operations[0],WireOperation::SubmitDesired { value, .. } if value == &Value::Null)
    );
    let missing = r#"{"op":"submit_desired","topic":"/lamp/desired"}"#;
    assert!(serde_json::from_str::<WireOperation>(missing).is_err());
    assert!(serde_json::from_str::<NodeView>(r#"{"kind":"state","current":null}"#).is_err());
    assert!(matches!(
        serde_json::from_str::<ServerMessage>(
            r#"{"result":{"sequence":1,"warnings":[]},"request_id":"w","type":"reply"}"#
        )
        .unwrap(),
        ServerMessage::Reply {
            result: CommitView { sequence: 1, .. },
            ..
        }
    ));
    // A reply always carries a commit.
    assert!(
        serde_json::from_str::<ServerMessage>(r#"{"result":null,"request_id":"w","type":"reply"}"#)
            .is_err()
    );
}

#[test]
fn a_hello_without_session_mode_is_managed() {
    let hello: ClientMessage = serde_json::from_str(
        r#"{"type":"hello","request_id":"h","client":"laptop","selectors":[]}"#,
    )
    .unwrap();
    assert!(matches!(
        hello,
        ClientMessage::Hello {
            session: SessionMode::Managed,
            ..
        }
    ));
}

#[test]
fn server_messages_with_invalid_topics_fail_to_decode() {
    for message in [
        r#"{"type":"snapshot","request_id":"h","sequence":1,"warnings":[],"nodes":{"invalid":{"kind":"event","last_publisher":{"client":"x","session":null,"at":"2026-01-01T00:00:00Z"}}}}"#,
        r#"{"type":"update","sequence":2,"changes":[{"type":"removed","topic":"/a/*","previous":{"kind":"event","last_publisher":{"client":"x","session":null,"at":"2026-01-01T00:00:00Z"}}}]}"#,
    ] {
        assert!(
            serde_json::from_str::<ServerMessage>(message).is_err(),
            "{message}"
        );
    }
}

#[test]
fn an_exact_selector_matches_only_its_topic() {
    let topic = TopicPath::parse("/lamp/hue").unwrap();
    let selector = Selector::from(&topic);
    assert_eq!(selector, Selector::parse("/lamp/hue").unwrap());
    assert!(selector.matches(&topic));
    assert!(!selector.matches(&TopicPath::parse("/lamp/hue/extra").unwrap()));
    assert!(!selector.matches(&TopicPath::parse("/lamp").unwrap()));
}
