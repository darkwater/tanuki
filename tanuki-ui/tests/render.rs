use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use tanuki_protocol::*;
use tanuki_ui::{
    app::ConnectionConfig,
    model::{ConnectionStatus, Model},
    ui::{Action, ViewState},
};

fn path(p: &str) -> TopicPath {
    TopicPath::parse(p).unwrap()
}
fn state(value: Value) -> NodeView {
    NodeView::State {
        current: CurrentValueView {
            value: JsonValue::new(value),
            last_write: ProvenanceView {
                client: "writer\x1b[2J".into(),
                session: Some(7),
                at: "2026-10-31T00:00:00Z".into(),
            },
            expires_at: Some("2026-11-01T00:00:00Z".into()),
        },
    }
}
fn model(nodes: Vec<(&str, NodeView)>) -> Model {
    let mut m = Model::from_snapshot(SnapshotView {
        sequence: 4,
        nodes: nodes.into_iter().map(|(p, n)| (p.into(), n)).collect(),
    })
    .unwrap();
    m.set_status(ConnectionStatus::Live);
    m
}
fn key(ui: &mut ViewState, m: &mut Model, code: KeyCode) -> Action {
    ui.key(KeyEvent::new(code, KeyModifiers::NONE), m)
}
fn screen(ui: &mut ViewState, m: &Model, w: u16, h: u16) -> (String, Terminal<TestBackend>) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui.draw(f, m, &ConnectionConfig::default()))
        .unwrap();
    let mut text = String::new();
    // Wide glyphs occupy a symbol cell plus continuation cells. Reconstruct
    // visible text without counting those continuation cells as literal spaces.
    for row in t.backend().buffer().content.chunks(usize::from(w).max(1)) {
        let mut x = 0;
        while x < row.len() {
            let symbol = row[x].symbol();
            text.push_str(symbol);
            x += ratatui::text::Span::raw(symbol).width().max(1);
        }
        text.push('\n');
    }
    (text, t)
}
#[test]
fn empty_view_status_and_key_hints_are_explicit() {
    let mut ui = ViewState::default();
    let m = model(vec![]);
    let (text, _) = screen(&mut ui, &m, 120, 24);
    assert!(text.lines().nth(22).unwrap().contains("LIVE"));
    for expected in [
        "No topics",
        "Select a topic",
        "LIVE",
        "q quit",
        "r reconnect",
        "? help",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}
#[test]
fn selection_tree_metadata_and_sanitization_reach_the_screen() {
    let mut ui = ViewState::default();
    let mut m = model(vec![
        ("/lamp", state(Value::Null)),
        ("/lamp/hue", state(Value::Integer(42))),
    ]);
    key(&mut ui, &mut m, KeyCode::Right);
    key(&mut ui, &mut m, KeyCode::Down);
    let (text, t) = screen(&mut ui, &m, 160, 24);
    for expected in [
        "/lamp/hue",
        "state",
        "integer",
        "42",
        "writer\\u{1b}[2J",
        "2026-11-01",
        "not available from this server API",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        t.backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.bg == Color::Blue)
    );
    assert!(!text.contains('\x1b'));
}
#[test]
fn filtering_navigation_help_and_exit_controls_work() {
    let mut ui = ViewState::default();
    let mut m = model(vec![
        ("/a/leaf", state(Value::Null)),
        ("/z", state(Value::Null)),
    ]);
    key(&mut ui, &mut m, KeyCode::Char('/'));
    for c in "leaf".chars() {
        key(&mut ui, &mut m, KeyCode::Char(c));
    }
    assert_eq!(m.filter(), "leaf");
    key(&mut ui, &mut m, KeyCode::Enter);
    key(&mut ui, &mut m, KeyCode::Down);
    assert_eq!(m.selected_path(), Some(&path("/a/leaf")));
    key(&mut ui, &mut m, KeyCode::Esc);
    assert!(m.filter().is_empty());
    key(&mut ui, &mut m, KeyCode::Char('?'));
    let (text, _) = screen(&mut ui, &m, 120, 24);
    assert!(text.contains("Read-only inspector"));
    assert!(text.contains("PageUp"));
    key(&mut ui, &mut m, KeyCode::Esc);
    assert_eq!(key(&mut ui, &mut m, KeyCode::Char('r')), Action::Reconnect);
    assert_eq!(key(&mut ui, &mut m, KeyCode::Char('q')), Action::Quit);
    assert_eq!(
        ui.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut m
        ),
        Action::Quit
    );
}
#[test]
fn tiny_layouts_and_all_stale_states_are_safe() {
    let mut ui = ViewState::default();
    let mut m = model(vec![]);
    for (w, h) in [(0, 0), (1, 1), (8, 2), (30, 4), (59, 9)] {
        let (text, _) = screen(&mut ui, &m, w, h);
        if w == 30 {
            assert!(text.contains("Resize"));
        }
    }
    for status in [
        ConnectionStatus::Disconnected,
        ConnectionStatus::Connecting,
        ConnectionStatus::Lagged,
        ConnectionStatus::Error("bad\x1b[2J".into()),
    ] {
        m.set_status(status);
        let (text, _) = screen(&mut ui, &m, 120, 24);
        assert!(text.contains("STALE"));
        assert!(!text.contains('\x1b'));
    }
}
#[test]
fn long_unicode_values_have_short_rows_and_scrollable_complete_details() {
    let mut ui = ViewState::default();
    let mut m = model(vec![(
        "/text",
        state(Value::String(format!("日本語{}TAIL", "語".repeat(400)))),
    )]);
    let (first, _) = screen(&mut ui, &m, 120, 18);
    assert!(first.contains("日本語"), "{first}");
    assert!(!first.contains("TAIL"));
    key(&mut ui, &mut m, KeyCode::Tab);
    key(&mut ui, &mut m, KeyCode::End);
    let (last, _) = screen(&mut ui, &m, 120, 18);
    assert!(last.contains("TAIL"));
    key(&mut ui, &mut m, KeyCode::Home);
    key(&mut ui, &mut m, KeyCode::PageDown);
    let (scrolled, _) = screen(&mut ui, &m, 120, 10);
    assert!(scrolled.contains("Schema"));
}
#[test]
fn instant_and_system_nodes_never_show_occurrences_as_retained_state() {
    let mut ui = ViewState::default();
    let m = model(vec![(
        "/$diagnostics/example",
        NodeView::Event {
            last_publisher: ProvenanceView {
                client: "server".into(),
                session: None,
                at: "now".into(),
            },
        },
    )]);
    let mut m = m;
    m.set_filter("example".into());
    m.select(&path("/$diagnostics/example"));
    let (text, _) = screen(&mut ui, &m, 160, 24);
    for e in ["[sys]", "event", "No retained payload", "System topic"] {
        assert!(text.contains(e), "missing {e}");
    }
}
