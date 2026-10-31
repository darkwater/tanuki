use std::collections::BTreeMap;
use tanuki_protocol::{
    ChangeView, CurrentValueView, InputDefinitionView, JsonValue, NodeView, OccurrenceView,
    ProvenanceView, SnapshotView, TopicPath, UpdateView, Value,
};
use tanuki_ui::{format::*, model::*};

fn path(s: &str) -> TopicPath {
    TopicPath::parse(s).unwrap()
}
fn provenance(client: &str) -> ProvenanceView {
    ProvenanceView {
        client: client.into(),
        session: Some(1),
        at: "2026-10-31T12:00:00Z".into(),
    }
}
fn state(value: Value) -> NodeView {
    NodeView::State {
        current: CurrentValueView {
            value: JsonValue::new(value),
            last_write: provenance("writer"),
            expires_at: None,
        },
    }
}
fn model(nodes: Vec<(&str, NodeView)>) -> Model {
    Model::from_snapshot(SnapshotView {
        sequence: 10,
        nodes: nodes.into_iter().map(|(p, n)| (p.into(), n)).collect(),
    })
    .unwrap()
}
fn upsert(p: &str, node: NodeView) -> ChangeView {
    ChangeView::Upsert {
        topic: p.into(),
        node,
    }
}
fn paths(m: &Model) -> Vec<String> {
    m.rows().iter().map(|r| r.path().to_string()).collect()
}
fn detail(m: &Model) -> String {
    details(m.selected_path().unwrap(), m.selected_node()).join("\n")
}

#[test]
fn empty_snapshot_is_explicit() {
    let m = model(vec![]);
    assert_eq!(m.empty_message(), Some("No topics"));
    assert!(m.rows().is_empty());
    assert!(m.selected_path().is_none());
    assert_eq!(m.sequence(), 10);
}

#[test]
fn node_and_its_descendant_are_separately_inspectable() {
    let mut m = model(vec![
        ("/lamp", state(Value::Null)),
        ("/lamp/hue", state(Value::Integer(42))),
    ]);
    assert_eq!(paths(&m), ["/lamp"]);
    m.set_expanded(&path("/lamp"), true);
    assert_eq!(paths(&m), ["/lamp", "/lamp/hue"]);
    m.select(&path("/lamp"));
    assert!(detail(&m).contains("null"));
    m.select(&path("/lamp/hue"));
    assert!(detail(&m).contains("42"));
    assert_eq!(m.rows()[1].depth(), 1);
}

#[test]
fn four_kinds_and_payload_states_are_distinct() {
    let nodes = [
        state(Value::Null),
        NodeView::Desired {
            definition: InputDefinitionView {},
            claim: None,
            current: None,
        },
        NodeView::Event {
            last_publisher: provenance("event producer"),
        },
        NodeView::Command {
            definition: InputDefinitionView {},
            claim: None,
        },
    ];
    assert_eq!(
        nodes.each_ref().map(kind_label),
        ["state", "desired", "event", "command"]
    );
    assert_eq!(
        nodes.each_ref().map(value_type),
        ["null", "absent", "none (instant)", "none (instant)"]
    );
    assert!(
        details(&path("/missing"), None)
            .join("\n")
            .contains("No node")
    );
    assert!(
        details(&path("/desired"), Some(&nodes[1]))
            .join("\n")
            .contains("No desired payload")
    );
    for n in &nodes[2..] {
        assert!(
            details(&path("/instant"), Some(n))
                .join("\n")
                .contains("No retained payload")
        );
    }
    assert!(
        details(&path("/null"), Some(&nodes[0]))
            .join("\n")
            .contains("not available from this server API")
    );
}

#[test]
fn whole_batches_remove_stale_details_and_keep_occurrences_separate() {
    let a = state(Value::Integer(1));
    let mut m = model(vec![("/a", a.clone()), ("/b", a.clone())]);
    m.apply(UpdateView {
        sequence: 12,
        changes: vec![
            upsert("/a", state(Value::Integer(2))),
            upsert("/b", state(Value::Integer(3))),
            ChangeView::Event {
                topic: "/pulse".into(),
                event: OccurrenceView {
                    value: JsonValue::new(Value::Integer(99)),
                    provenance: provenance("sender"),
                },
            },
        ],
    })
    .unwrap();
    assert!(detail(&m).contains("2"));
    m.select(&path("/b"));
    assert!(detail(&m).contains("3"));
    assert!(!paths(&m).contains(&"/pulse".to_string()));
    m.apply(UpdateView {
        sequence: 13,
        changes: vec![ChangeView::Removed {
            topic: "/b".into(),
            previous: a,
        }],
    })
    .unwrap();
    assert_eq!(m.selected_path(), Some(&path("/a")));
    assert!(!detail(&m).contains("/b"));
    m.apply(UpdateView {
        sequence: 14,
        changes: vec![ChangeView::Removed {
            topic: "/a".into(),
            previous: state(Value::Null),
        }],
    })
    .unwrap();
    assert!(m.selected_node().is_none());
    assert!(m.selected_path().is_none());
}

#[test]
fn filtering_collapse_and_removal_preserve_valid_selection() {
    let mut m = model(vec![
        ("/lamp/hue", state(Value::Null)),
        ("/lamp/brightness", state(Value::Null)),
        ("/z", state(Value::Null)),
    ]);
    m.set_filter("hue".into());
    assert_eq!(paths(&m), ["/lamp", "/lamp/hue"]);
    m.select(&path("/lamp/hue"));
    m.set_filter(String::new());
    assert_eq!(m.selected_path(), Some(&path("/lamp")));
    assert!(m.selected_node().is_none());
    m.set_expanded(&path("/lamp"), true);
    m.select(&path("/lamp/hue"));
    m.set_expanded(&path("/lamp"), false);
    assert_eq!(m.selected_path(), Some(&path("/lamp")));
    m.set_filter("unmatched".into());
    assert_eq!(m.empty_message(), Some("No matching topics"));
    assert!(m.selected_path().is_none());
}

#[test]
fn metadata_updates_gaps_errors_and_staleness_do_not_corrupt_state() {
    let mut m = model(vec![("/a", state(Value::Null))]);
    let changed = NodeView::State {
        current: CurrentValueView {
            value: JsonValue::new(Value::Null),
            last_write: provenance("new writer\x1b[2J"),
            expires_at: Some("2026-11-01T00:00:00Z".into()),
        },
    };
    m.apply(UpdateView {
        sequence: 100,
        changes: vec![upsert("/a", changed)],
    })
    .unwrap();
    assert!(detail(&m).contains("new writer\\u{1b}[2J"));
    assert!(detail(&m).contains("2026-11-01"));
    assert_eq!(m.sequence(), 100);
    let before = detail(&m);
    assert!(
        m.apply(UpdateView {
            sequence: 99,
            changes: vec![upsert("/bad", state(Value::Null))]
        })
        .is_err()
    );
    assert_eq!(detail(&m), before);
    assert!(
        m.apply(UpdateView {
            sequence: 101,
            changes: vec![
                upsert("/ok", state(Value::Null)),
                upsert("bad", state(Value::Null))
            ]
        })
        .is_err()
    );
    assert_eq!(paths(&m), ["/a"]);
    assert_eq!(m.sequence(), 100);
    m.set_status(ConnectionStatus::Disconnected);
    assert_eq!(m.status(), &ConnectionStatus::Disconnected);
    assert_eq!(detail(&m), before);
}

#[test]
fn values_are_readable_safely_shortened_and_fully_available_in_details() {
    assert_eq!(
        value_text(&Value::Bytes(vec![0, 255, 27])),
        "bytes[3]: 00 ff 1b"
    );
    let timestamp = serde_json::from_value::<JsonValue>(
        serde_json::json!({"$timestamp":"2026-10-31T12:00:00Z"}),
    )
    .unwrap()
    .into_inner();
    let duration = serde_json::from_value::<JsonValue>(serde_json::json!({"$duration":"PT1.5S"}))
        .unwrap()
        .into_inner();
    assert!(value_text(&timestamp).contains("2026-10-31T12:00:00Z"));
    assert!(value_text(&duration).contains("PT1.5S"));
    assert_eq!(
        safe_text("a\x1b[31m\n\r\t\x07\u{009b}\u{202e}"),
        "a\\u{1b}[31m\\n\\r\\t\\u{7}\\u{9b}\\u{202e}"
    );
    assert_eq!(preview("日本語abc", 4), "日本語…");
    assert_eq!(preview("abc", 0), "");
    assert_eq!(preview("abc", 1), "…");
    let long = "x".repeat(300);
    let m = model(vec![("/a", state(Value::String(long.clone())))]);
    assert!(detail(&m).contains(&long));
    assert_eq!(
        preview(&value_text(&Value::String(long)), 20)
            .chars()
            .count(),
        20
    );
    let v = Value::Map(BTreeMap::from([(
        "\x1b".into(),
        Value::String("\x1b[2J".into()),
    )]));
    assert!(!value_text(&v).chars().any(char::is_control));
    assert_eq!(preview("\x1b[2J", 30), "\\u{1b}[2J");
}

#[test]
fn selection_and_expansion_survive_insertions_and_real_node_removal() {
    let parent = state(Value::Integer(9));
    let mut m = model(vec![
        ("/lamp", parent.clone()),
        ("/lamp/hue", state(Value::Null)),
        ("/lamp/hue-extra/x", state(Value::Null)),
    ]);
    m.set_expanded(&path("/lamp"), true);
    m.select(&path("/lamp/hue"));
    m.apply(UpdateView {
        sequence: 11,
        changes: vec![
            upsert("/a", state(Value::Null)),
            upsert("/lamp/brightness", state(Value::Null)),
        ],
    })
    .unwrap();
    assert_eq!(m.selected_path(), Some(&path("/lamp/hue")));
    assert_eq!(
        paths(&m),
        [
            "/a",
            "/lamp",
            "/lamp/brightness",
            "/lamp/hue",
            "/lamp/hue-extra"
        ]
    );
    m.select(&path("/lamp"));
    m.apply(UpdateView {
        sequence: 12,
        changes: vec![ChangeView::Removed {
            topic: "/lamp".into(),
            previous: parent,
        }],
    })
    .unwrap();
    assert_eq!(m.selected_path(), Some(&path("/lamp")));
    assert!(m.selected_node().is_none());
    assert!(detail(&m).contains("No node"));
    assert!(paths(&m).contains(&"/lamp/hue".to_string()));
    m.set_filter("hue".into());
    m.set_filter(String::new());
    assert!(
        m.rows()
            .iter()
            .find(|r| r.path() == &path("/lamp"))
            .unwrap()
            .is_expanded()
    );
}

#[test]
fn controls_in_paths_and_claims_are_displayed_without_mutating_dtos() {
    use tanuki_protocol::{ClaimReleaseView, ClaimView};
    let p = path("/$connections/observer\u{202e}");
    let claim = ClaimView {
        id: 5,
        owner: "owner\x1b[2J".into(),
        session: 7,
        release: ClaimReleaseView::After {
            duration: "PT5S".into(),
        },
    };
    let node = NodeView::Desired {
        definition: InputDefinitionView {},
        claim: Some(claim),
        current: Some(CurrentValueView {
            value: JsonValue::new(Value::Null),
            last_write: provenance("writer"),
            expires_at: None,
        }),
    };
    let before = node.clone();
    let lines = details(&p, Some(&node));
    assert!(lines.iter().any(|s| s == "System topic"));
    let text = lines.join("\n");
    assert!(text.contains("observer\\u{202e}"));
    assert!(text.contains("owner\\u{1b}[2J"));
    assert!(text.contains("after PT5S"));
    assert_eq!(value_type(&node), "null");
    assert_eq!(node, before);
    assert!(lines.iter().all(|s| !s.chars().any(char::is_control)));
}

#[test]
fn navigation_clamps_and_invalid_snapshots_fail_at_the_boundary() {
    let mut m = model(vec![("/a", state(Value::Null)), ("/b", state(Value::Null))]);
    m.move_selection(isize::MAX);
    assert_eq!(m.selected_path(), Some(&path("/b")));
    m.move_selection(isize::MIN);
    assert_eq!(m.selected_path(), Some(&path("/a")));
    m.select(&path("/invisible"));
    assert_eq!(m.selected_path(), Some(&path("/a")));
    let mut empty = model(vec![]);
    empty.move_selection(1);
    assert!(empty.selected_path().is_none());
    assert!(
        Model::from_snapshot(SnapshotView {
            sequence: 1,
            nodes: BTreeMap::from([("/".into(), state(Value::Null))])
        })
        .is_err()
    );
}

#[test]
fn removal_selects_the_nearest_surviving_old_row() {
    let mut m = model(
        ["/a", "/b", "/c", "/d", "/e", "/f"]
            .map(|p| (p, state(Value::Null)))
            .to_vec(),
    );
    m.select(&path("/d"));
    m.apply(UpdateView {
        sequence: 11,
        changes: ["/a", "/b", "/d", "/e"]
            .map(|p| ChangeView::Removed {
                topic: p.into(),
                previous: state(Value::Null),
            })
            .to_vec(),
    })
    .unwrap();
    assert_eq!(m.selected_path(), Some(&path("/c")));
}

#[test]
fn replacement_baselines_can_restart_sequences_and_preserve_surviving_navigation() {
    let mut m = model(vec![
        ("/lamp/hue", state(Value::Integer(1))),
        ("/old", state(Value::Null)),
    ]);
    m.set_expanded(&path("/lamp"), true);
    m.select(&path("/lamp/hue"));
    m.set_filter("lamp".into());
    m.replace_snapshot(SnapshotView {
        sequence: 0,
        nodes: [("/lamp/hue".into(), state(Value::Integer(2)))].into(),
    })
    .unwrap();
    assert_eq!(m.sequence(), 0);
    assert_eq!(m.selected_path(), Some(&path("/lamp/hue")));
    assert_eq!(m.filter(), "lamp");
    assert!(m.node(&path("/old")).is_none());
    assert!(detail(&m).contains("Value: 2"));
    assert!(
        m.replace_snapshot(SnapshotView {
            sequence: 9,
            nodes: [("invalid".into(), state(Value::Null))].into()
        })
        .is_err()
    );
    assert_eq!(m.sequence(), 0);
    m.set_filter(String::new());
    assert!(
        m.rows()
            .iter()
            .find(|r| r.path() == &path("/lamp"))
            .unwrap()
            .is_expanded()
    );
}
