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
fn selector_intersection_detects_shared_possible_topics() {
    let cases = [
        ("/battery/*", "/battery/phone", true),
        ("/battery/*", "/battery/*/level", false),
        ("/**/status", "/devices/**", true),
        ("/a/**/z", "/a/b/{y,z}", true),
        ("/a/{b,c}", "/a/{c,d}", true),
        ("/a/{b,c}", "/a/{d,e}", false),
        ("/**", "/$connections/*", true),
        ("/", "/**", false),
    ];

    for (left, right, expected) in cases {
        let left = Selector::parse(left).unwrap();
        let right = Selector::parse(right).unwrap();
        assert_eq!(left.intersects(&right), expected, "{left} vs {right}");
        assert_eq!(
            right.intersects(&left),
            expected,
            "symmetry: {right} vs {left}"
        );
    }
}

#[test]
fn nonintersecting_selectors_never_match_the_same_topic_in_a_small_corpus() {
    let selectors = [
        "/a",
        "/a/*",
        "/a/**",
        "/b/{x,y}",
        "/**/z",
        "/$connections/*",
    ]
    .map(|value| Selector::parse(value).unwrap());
    let topics = [
        "/a",
        "/a/x",
        "/a/x/z",
        "/b/x",
        "/b/y",
        "/b/y/z",
        "/c/z",
        "/$connections/client",
    ]
    .map(topic);

    for left in &selectors {
        for right in &selectors {
            if !left.intersects(right) {
                assert!(
                    topics
                        .iter()
                        .all(|topic| !(left.matches(topic) && right.matches(topic)))
                );
            }
        }
    }
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
