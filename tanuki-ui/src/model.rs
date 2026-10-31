use std::collections::{BTreeMap, BTreeSet};

use tanuki_protocol::{
    ChangeView, ClientViewError, NodeView, PathParseError, SelectedView, SnapshotView, TopicPath,
    UpdateView,
};
use thiserror::Error;

/// Display state only; connection lifecycle is owned by the future application loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionStatus {
    Connecting,
    Live,
    Disconnected,
    Error(String),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid topic in inspector input: {0}")]
    Path(#[from] PathParseError),
    #[error(transparent)]
    Sequence(#[from] ClientViewError),
}

/// A navigation row, which may be a real node, an ancestor prefix, or both.
/// Obtain the real node from `Model::node`; prefixes never fabricate node data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeRow {
    path: TopicPath,
    depth: usize,
    has_children: bool,
    expanded: bool,
}

impl TreeRow {
    pub fn path(&self) -> &TopicPath {
        &self.path
    }
    pub fn depth(&self) -> usize {
        self.depth
    }
    pub fn has_children(&self) -> bool {
        self.has_children
    }
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }
}

pub struct Model {
    view: SelectedView,
    selected: Option<TopicPath>,
    expanded: BTreeSet<TopicPath>,
    filter: String,
    status: ConnectionStatus,
    rows: Vec<TreeRow>,
}

impl Model {
    pub fn from_snapshot(snapshot: SnapshotView) -> Result<Self, ModelError> {
        for path in snapshot.nodes.keys() {
            TopicPath::parse(path)?;
        }
        let mut model = Self {
            view: SelectedView::from_snapshot(snapshot),
            selected: None,
            expanded: BTreeSet::new(),
            filter: String::new(),
            status: ConnectionStatus::Connecting,
            rows: Vec::new(),
        };
        model.rebuild();
        Ok(model)
    }

    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub fn empty_message(&self) -> Option<&'static str> {
        if self.view.nodes().is_empty() {
            Some("No topics")
        } else if self.rows.is_empty() {
            Some("No matching topics")
        } else {
            None
        }
    }

    pub fn selected_path(&self) -> Option<&TopicPath> {
        self.selected.as_ref()
    }

    pub fn node(&self, path: &TopicPath) -> Option<&NodeView> {
        self.view.nodes().get(&path.to_string())
    }

    pub fn selected_node(&self) -> Option<&NodeView> {
        self.selected.as_ref().and_then(|path| self.node(path))
    }

    /// Hidden and absent rows cannot become the selection.
    pub fn select(&mut self, path: &TopicPath) {
        if self.rows.iter().any(|row| &row.path == path) {
            self.selected = Some(path.clone());
        }
    }

    pub fn move_selection(&mut self, offset: isize) {
        if self.rows.is_empty() {
            return;
        }
        let index = self
            .selected_index()
            .saturating_add_signed(offset)
            .min(self.rows.len() - 1);
        self.selected = Some(self.rows[index].path.clone());
    }

    pub fn set_expanded(&mut self, path: &TopicPath, expanded: bool) {
        if expanded {
            self.expanded.insert(path.clone());
        } else {
            self.expanded.remove(path);
        }
        self.rebuild();
    }

    /// Case-sensitive substring matching on full paths; matching ancestors are visible.
    /// Filtering temporarily reveals descendants without changing saved expansion.
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn apply(&mut self, update: UpdateView) -> Result<(), ModelError> {
        // Validate external path DTOs before any mutation, including occurrence paths.
        for change in &update.changes {
            let (ChangeView::Upsert { topic, .. }
            | ChangeView::Removed { topic, .. }
            | ChangeView::Event { topic, .. }
            | ChangeView::Command { topic, .. }) = change;
            TopicPath::parse(topic)?;
        }
        // SelectedView applies the entire batch before rows/details can be read.
        // Occurrences deliberately do not become retained values or history.
        self.view.apply(update)?;
        self.rebuild();
        Ok(())
    }

    pub fn sequence(&self) -> u64 {
        self.view.sequence()
    }

    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        self.status = status;
    }

    fn selected_index(&self) -> usize {
        self.rows
            .iter()
            .position(|row| Some(&row.path) == self.selected.as_ref())
            .unwrap_or(0)
    }

    fn rebuild(&mut self) {
        let previous_index = self.selected_index();
        let mut tree = BTreeMap::<TopicPath, bool>::new();
        let mut included = BTreeSet::new();
        for name in self.view.nodes().keys() {
            let matches = name.contains(&self.filter);
            let mut prefix = String::new();
            let segments: Vec<_> = name[1..].split('/').collect();
            for (index, segment) in segments.iter().enumerate() {
                prefix.push('/');
                prefix.push_str(segment);
                // Input paths were validated at the owning boundary. Every nonempty
                // segment prefix of a valid TopicPath is itself a valid TopicPath.
                let path = TopicPath::parse(&prefix).expect("validated topic prefix");
                let children = index + 1 < segments.len();
                tree.entry(path.clone())
                    .and_modify(|has_children| *has_children |= children)
                    .or_insert(children);
                if matches {
                    included.insert(path);
                }
            }
        }
        self.expanded.retain(|path| tree.get(path) == Some(&true));
        let searching = !self.filter.is_empty();
        let previous_rows = std::mem::take(&mut self.rows);
        let mut collapsed: Option<TopicPath> = None;
        // TopicPath orders by segments: a prefix and all descendants are contiguous.
        for (path, has_children) in &tree {
            if !included.contains(path) {
                continue;
            }
            if !searching
                && collapsed
                    .as_ref()
                    .is_some_and(|prefix| prefix.is_prefix_of(path))
            {
                continue;
            }
            let expanded = *has_children && (searching || self.expanded.contains(path));
            collapsed = if *has_children && !expanded {
                Some(path.clone())
            } else {
                None
            };
            self.rows.push(TreeRow {
                path: path.clone(),
                depth: path.segments().len() - 1,
                has_children: *has_children,
                expanded,
            });
        }
        if self
            .rows
            .iter()
            .any(|row| Some(&row.path) == self.selected.as_ref())
        {
            return;
        }
        let visible: BTreeSet<_> = self.rows.iter().map(|row| &row.path).collect();
        let nearest = previous_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| visible.contains(&row.path))
            .min_by_key(|(index, _)| index.abs_diff(previous_index))
            .map(|(_, row)| row.path.clone());
        // Prefer the closest visible ancestor, then the nearest surviving old row
        // (ties choose the preceding row), then the first new row.
        self.selected = self
            .selected
            .as_ref()
            .and_then(|selected| {
                self.rows
                    .iter()
                    .rev()
                    .find(|row| row.path.is_prefix_of(selected))
                    .map(|row| row.path.clone())
            })
            .or(nearest)
            .or_else(|| self.rows.first().map(|row| row.path.clone()));
    }
}
