use tanuki::domain::{PathParseError, Selection, Selector, TopicPath};

fn topic(path: &str) -> TopicPath {
    TopicPath::parse(path).expect("valid topic fixture")
}

#[test]
fn topic_paths_enforce_boundaries_through_every_public_conversion() {
    assert_eq!(
        TopicPath::parse("battery/phone"),
        Err(PathParseError::NotAbsolute)
    );
    assert_eq!(TopicPath::parse("/"), Err(PathParseError::Root));
    assert!(matches!(
        TopicPath::parse("/battery/*"),
        Err(PathParseError::ReservedCharacter { character: '*', .. })
    ));
    assert!(matches!(
        TopicPath::parse_ordinary("/$connections/laptop"),
        Err(PathParseError::ReservedSystemNamespace { .. })
    ));

    let decoded = serde_json::from_str::<TopicPath>(r#""/battery/{phone}""#);
    assert!(decoded.is_err(), "deserialization must use path validation");

    let system = TopicPath::parse("/$connections/laptop").expect("system path is representable");
    assert!(system.is_system());
    assert_eq!(system.to_string(), "/$connections/laptop");

    for reserved in ['*', '?', '[', ']', '{', '}', '\\'] {
        let candidate = format!("/battery/phone{reserved}");
        assert!(matches!(
            TopicPath::parse(&candidate),
            Err(PathParseError::ReservedCharacter { character, .. }) if character == reserved
        ));
    }
}

#[test]
fn selector_matcher_covers_exact_single_recursive_choice_union_and_empty() {
    let cases = [
        ("/battery/phone", "/battery/phone", true),
        ("/battery/phone", "/battery/laptop", false),
        ("/battery/*", "/battery/phone", true),
        ("/battery/*", "/battery/phone/level", false),
        ("/devices/**", "/devices", true),
        ("/devices/**", "/devices/tv/power", true),
        ("/devices/**/power", "/devices/power", true),
        ("/devices/**/power", "/devices/tv/room/power", true),
        ("/devices/{tv,lamp}/power", "/devices/lamp/power", true),
        ("/devices/{tv,lamp}/power", "/devices/phone/power", false),
    ];

    for (selector, candidate, expected) in cases {
        let selector = Selector::parse(selector).expect("valid selector fixture");
        assert_eq!(
            selector.matches(&topic(candidate)),
            expected,
            "selector {selector:?} against {candidate}"
        );
    }

    let union = Selection::new(vec![
        Selector::parse("/battery/*").unwrap(),
        Selector::parse("/tv/**").unwrap(),
    ]);
    assert!(union.matches(&topic("/tv/power")));
    assert!(!union.matches(&topic("/lamp/power")));
    assert!(!Selection::default().matches(&topic("/battery/phone")));
    assert!(
        !Selector::parse("/")
            .unwrap()
            .matches(&topic("/battery/phone"))
    );
}

#[test]
fn malformed_selector_syntax_is_rejected_instead_of_guessed() {
    for invalid in [
        "battery/*",
        "/battery/",
        "/battery/phone*",
        "/battery/{phone}",
        "/battery/{phone,}",
        "/battery/{phone,{laptop}}",
        "/battery/?",
    ] {
        assert!(
            Selector::parse(invalid).is_err(),
            "selector should be rejected: {invalid}"
        );
    }

    let selector = Selector::parse("/devices/{tv,lamp}/**").unwrap();
    let encoded = serde_json::to_string(&selector).unwrap();
    let decoded: Selector = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, selector);
}

#[test]
fn maps_remain_values_instead_of_implicit_child_topics() {
    let parent = topic("/desktop/state");
    let child_selector = Selector::parse("/desktop/state/clipboard").unwrap();

    assert!(!child_selector.matches(&parent));
}
