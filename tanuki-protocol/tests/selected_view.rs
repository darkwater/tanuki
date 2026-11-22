use std::collections::BTreeMap;

use tanuki_protocol::{
    ChangeView, ClientViewError, CurrentValueView, NodeView, ProvenanceView, SelectedView,
    SnapshotView, UpdateView, Value,
};

fn path(input: &str) -> tanuki_protocol::TopicPath {
    input.parse().unwrap()
}

fn current(value: i64, at: &str) -> CurrentValueView {
    CurrentValueView {
        value: Value::Integer(value),
        last_write: ProvenanceView {
            client: "lamp".parse().unwrap(),
            session: None,
            at: at.parse().unwrap(),
        },
        expires_at: None,
    }
}

#[test]
fn complete_update_is_applied_before_the_new_shape_is_observed() {
    let mut view = SelectedView::from_snapshot(SnapshotView {
        sequence: 1,
        nodes: BTreeMap::from([
            (
                "/lamp/hue".parse().unwrap(),
                NodeView::State {
                    current: current(10, "1970-01-01T00:00:01Z"),
                },
            ),
            (
                "/lamp/brightness".parse().unwrap(),
                NodeView::State {
                    current: current(20, "1970-01-01T00:00:01Z"),
                },
            ),
        ]),
    });

    view.apply(UpdateView {
        sequence: 3,
        changes: vec![
            ChangeView::Upsert {
                topic: "/lamp/hue".parse().unwrap(),
                node: NodeView::State {
                    current: current(120, "1970-01-01T00:00:03Z"),
                },
            },
            ChangeView::Upsert {
                topic: "/lamp/brightness".parse().unwrap(),
                node: NodeView::State {
                    current: current(70, "1970-01-01T00:00:03Z"),
                },
            },
        ],
    })
    .unwrap();

    assert_eq!(view.sequence(), 3);
    let NodeView::State { current: hue } = &view.nodes()[&path("/lamp/hue")] else {
        panic!("expected hue state")
    };
    let NodeView::State {
        current: brightness,
    } = &view.nodes()[&path("/lamp/brightness")]
    else {
        panic!("expected brightness state")
    };
    assert_eq!(hue.value, Value::Integer(120));
    assert_eq!(brightness.value, Value::Integer(70));
}

#[test]
fn removals_change_shape_and_occurrences_stay_out_of_retained_nodes() {
    let mut view = SelectedView::from_snapshot(SnapshotView {
        sequence: 1,
        nodes: BTreeMap::from([(
            "/old".parse().unwrap(),
            NodeView::State {
                current: current(1, "1970-01-01T00:00:01Z"),
            },
        )]),
    });
    let occurrence = ChangeView::Event {
        topic: "/doorbell/rang".parse().unwrap(),
        event: tanuki_protocol::OccurrenceView {
            value: Value::Bool(true),
            provenance: ProvenanceView {
                client: "doorbell".parse().unwrap(),
                session: None,
                at: "1970-01-01T00:00:02Z".parse().unwrap(),
            },
        },
    };
    let applied = view
        .apply(UpdateView {
            sequence: 2,
            changes: vec![
                ChangeView::Removed {
                    topic: "/old".parse().unwrap(),
                    previous: NodeView::State {
                        current: current(1, "1970-01-01T00:00:01Z"),
                    },
                },
                occurrence.clone(),
            ],
        })
        .unwrap();

    assert!(view.nodes().is_empty());
    assert_eq!(applied.occurrences(), &[occurrence]);
}

#[test]
fn stale_or_duplicate_update_is_rejected_without_mutation() {
    let mut view = SelectedView::from_snapshot(SnapshotView {
        sequence: 5,
        nodes: BTreeMap::new(),
    });
    let before = view.clone();
    let result = view.apply(UpdateView {
        sequence: 5,
        changes: vec![ChangeView::Upsert {
            topic: "/unexpected".parse().unwrap(),
            node: NodeView::State {
                current: current(1, "1970-01-01T00:00:05Z"),
            },
        }],
    });
    assert_eq!(
        result,
        Err(ClientViewError::OutOfOrder {
            current: 5,
            received: 5
        })
    );
    assert_eq!(view, before);
}
