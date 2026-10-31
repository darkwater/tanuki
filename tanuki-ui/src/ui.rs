//! Synchronous rendering and input; no network work is awaited while drawing.
use crate::{
    app::ConnectionConfig,
    format::{details, kind_label, preview, safe_text, value_text, value_type},
    model::{ConnectionStatus, Model},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use tanuki_protocol::{NodeView, TopicPath};

#[derive(Debug, PartialEq)]
pub enum Action {
    Continue,
    Quit,
    Reconnect,
}
#[derive(Default, PartialEq)]
enum Pane {
    #[default]
    Tree,
    Details,
}
#[derive(Default)]
pub struct ViewState {
    pane: Pane,
    editing: bool,
    help: bool,
    list: ListState,
    selected: Option<TopicPath>,
    top: usize,
    left: usize,
    page: usize,
}
impl ViewState {
    fn sync_selection(&mut self, m: &Model) {
        if self.selected.as_ref() != m.selected_path() {
            self.selected = m.selected_path().cloned();
            self.top = 0;
            self.left = 0;
        }
    }
    pub fn key(&mut self, key: KeyEvent, m: &mut Model) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::Continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit;
        }
        self.sync_selection(m);
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    m.set_filter(String::new());
                    self.editing = false;
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    let mut s = m.filter().to_owned();
                    s.pop();
                    m.set_filter(s);
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    let mut s = m.filter().to_owned();
                    s.push(c);
                    m.set_filter(s);
                }
                _ => {}
            }
            return Action::Continue;
        }
        if key.code == KeyCode::Char('q') {
            return Action::Quit;
        }
        if self.help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help = false;
            }
            return Action::Continue;
        }
        match key.code {
            KeyCode::Char('r') => return Action::Reconnect,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Esc => m.set_filter(String::new()),
            KeyCode::Tab => {
                self.pane = if self.pane == Pane::Tree {
                    Pane::Details
                } else {
                    Pane::Tree
                }
            }
            KeyCode::PageDown => self.top = self.top.saturating_add(self.page.max(1)),
            KeyCode::PageUp => self.top = self.top.saturating_sub(self.page.max(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.pane == Pane::Tree {
                    m.move_selection(1);
                } else {
                    self.top = self.top.saturating_add(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.pane == Pane::Tree {
                    m.move_selection(-1);
                } else {
                    self.top = self.top.saturating_sub(1);
                }
            }
            KeyCode::Home => {
                if self.pane == Pane::Tree {
                    m.move_selection(isize::MIN);
                } else {
                    self.left = 0;
                    self.top = 0;
                }
            }
            KeyCode::End => {
                if self.pane == Pane::Tree {
                    m.move_selection(isize::MAX);
                } else {
                    self.left = usize::MAX;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.pane == Pane::Details {
                    self.left = self.left.saturating_add(8);
                } else if let Some(path) = m.selected_path().cloned() {
                    if m.rows()
                        .iter()
                        .any(|r| r.path() == &path && r.is_expanded())
                    {
                        m.move_selection(1);
                    } else {
                        m.set_expanded(&path, true);
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.pane == Pane::Details {
                    self.left = self.left.saturating_sub(8);
                } else if let Some(path) = m.selected_path().cloned() {
                    if m.rows()
                        .iter()
                        .any(|r| r.path() == &path && r.is_expanded())
                    {
                        m.set_expanded(&path, false);
                    } else {
                        let parent = m
                            .rows()
                            .iter()
                            .rev()
                            .find(|r| r.path() != &path && r.path().is_prefix_of(&path))
                            .map(|r| r.path().clone());
                        if let Some(parent) = parent {
                            m.select(&parent);
                        }
                    }
                }
            }
            _ => {}
        }
        self.sync_selection(m);
        Action::Continue
    }

    pub fn draw(&mut self, f: &mut Frame, m: &Model, config: &ConnectionConfig) {
        self.sync_selection(m);
        let status = match m.status() {
            ConnectionStatus::Live => format!("LIVE  sequence {}", m.sequence()),
            ConnectionStatus::Connecting => "CONNECTING • STALE cache".into(),
            ConnectionStatus::Disconnected => "DISCONNECTED • STALE cache".into(),
            ConnectionStatus::Lagged => "LAGGED • STALE cache • r reconnect".into(),
            ConnectionStatus::Error(e) => format!("ERROR • STALE cache • {}", safe_text(e)),
        };
        let area = f.area();
        if area.width < 60 || area.height < 10 {
            f.render_widget(
                Paragraph::new(format!(
                    "Resize to at least 60x10\n{status}\nq quit • r reconnect"
                )),
                area,
            );
            return;
        }
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(format!(
                "Tanuki • read-only • {} • {}",
                safe_text(config.url()),
                safe_text(config.client().as_str())
            ))
            .style(Style::default().fg(
                if matches!(m.status(), ConnectionStatus::Live) {
                    Color::Green
                } else {
                    Color::Yellow
                },
            )),
            header,
        );
        let [tree, detail] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(body);
        let treeblock = Block::default()
            .borders(Borders::ALL)
            .title("Topics")
            .border_style(Style::default().fg(if self.pane == Pane::Tree {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        if let Some(message) = m.empty_message() {
            f.render_widget(Paragraph::new(message).block(treeblock), tree);
        } else {
            let items: Vec<_> = m
                .rows()
                .iter()
                .map(|row| {
                    let path = row.path();
                    let marker = if row.is_expanded() {
                        "−"
                    } else if row.has_children() {
                        "+"
                    } else {
                        " "
                    };
                    let label = path.segments().last().unwrap_or("");
                    let prefix = if path.is_system() { "[sys] " } else { "" };
                    let node = m.node(path);
                    let value = match node {
                        Some(NodeView::State { current })
                        | Some(NodeView::Desired {
                            current: Some(current),
                            ..
                        }) => value_text(current.value.as_inner()),
                        _ => String::new(),
                    };
                    let label = format!(
                        "{}{marker} {prefix}{}",
                        "  ".repeat(row.depth()),
                        safe_text(label)
                    );
                    let kind = node
                        .map(|n| format!("  {}/{} ", kind_label(n), value_type(n)))
                        .unwrap_or_default();
                    let available = usize::from(tree.width.saturating_sub(2))
                        .saturating_sub(Span::raw(format!("{label}{kind}")).width());
                    // A scalar occupies at most two terminal cells. Reserve space
                    // for wide glyphs so the row ellipsis remains visible.
                    let value = preview(&value, (available / 2).min(24));
                    ListItem::new(format!("{label}{kind}{value}")).style(Style::default().fg(
                        if path.is_system() {
                            Color::Cyan
                        } else {
                            Color::White
                        },
                    ))
                })
                .collect();
            self.list.select(
                m.rows()
                    .iter()
                    .position(|r| Some(r.path()) == m.selected_path()),
            );
            f.render_stateful_widget(
                List::new(items).block(treeblock).highlight_style(
                    Style::default()
                        .bg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                tree,
                &mut self.list,
            );
        }
        let detailblock =
            Block::bordered().border_style(Style::default().fg(if self.pane == Pane::Details {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let inner = detailblock.inner(detail);
        self.page = usize::from(inner.height).max(1);
        let lines = m
            .selected_path()
            .map(|p| details(p, m.selected_node()))
            .unwrap_or_else(|| vec!["Select a topic".into()]);
        self.top = self.top.min(lines.len().saturating_sub(self.page));
        let width = usize::from(inner.width);
        let end = lines
            .iter()
            .map(|line| end_offset(line, width))
            .max()
            .unwrap_or(0);
        self.left = self.left.min(end);
        // Slice complete safe logical lines with usize offsets. Ratatui clips
        // wide/combining Unicode to cells; no u16 scroll limit truncates payloads.
        let visible = lines
            .iter()
            .skip(self.top)
            .take(self.page)
            .map(|line| line.chars().skip(self.left).take(width).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        let detailblock = detailblock.title(format!(
            "Details • row {} char {}",
            self.top + 1,
            self.left + 1
        ));
        f.render_widget(detailblock, detail);
        f.render_widget(Paragraph::new(visible), inner);
        let filter = if self.editing {
            "Filter (Enter accept, Esc clear)"
        } else {
            "Filter"
        };
        f.render_widget(Paragraph::new(format!("{status} • {filter}: {}\nq quit  r reconnect  ? help  / filter  Tab pane  ↑↓/jk move  ←→ expand/scroll  PgUp/PgDn scroll",safe_text(m.filter()))),footer);
        if self.help {
            f.render_widget(Clear, body);
            f.render_widget(Paragraph::new("Read-only inspector\n\n↑↓ or j/k: select topic; in details, scroll rows\n←→ or h/l: collapse/expand; in details, scroll columns\nTab: switch pane • Home/End: first/last topic or value columns\nPageUp/PageDown: scroll details • /: edit path filter\nEnter: accept filter • Escape: clear filter / close help\nr: reconnect once with fresh snapshot • q / Ctrl-C: exit\n?: close help\n\nInstant nodes have no retained payload or occurrence history.\nSchema information is unavailable from this server API.").block(Block::bordered().title("Help")),body);
        }
    }
}

// Find a suffix that fits the pane, reserving cells per scalar conservatively
// for combining/ZWJ sequences. This keeps the end of wide text reachable.
fn end_offset(line: &str, width: usize) -> usize {
    let mut used = 0;
    let mut suffix = 0;
    for ch in line.chars().rev() {
        let mut bytes = [0; 4];
        let cells = Span::raw(&*ch.encode_utf8(&mut bytes)).width();
        if used + cells > width {
            break;
        }
        used += cells;
        suffix += 1;
    }
    line.chars().count() - suffix
}
