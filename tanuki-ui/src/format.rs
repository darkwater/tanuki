//! Terminal-safe display text. Raw protocol values remain unchanged.
use std::fmt::Write;

use tanuki_protocol::{
    ClaimReleaseView, ClaimView, CurrentValueView, NodeView, ProvenanceView, TopicPath, Value,
    encode,
};

/// Escape control characters, including C1 terminal controls, and directional
/// formatting marks that could disguise paths or attribution. Preserve Unicode.
pub fn safe_text(input: &str) -> String {
    let mut output = String::new();
    for ch in input.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control()
                || matches!(ch, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                write!(output, "\\u{{{:x}}}", ch as u32).expect("String writing cannot fail");
            }
            ch => output.push(ch),
        }
    }
    output
}

pub fn value_text(value: &Value) -> String {
    if let Value::Bytes(bytes) = value {
        let mut output = format!("bytes[{}]:", bytes.len());
        for byte in bytes {
            write!(output, " {byte:02x}").expect("String writing cannot fail");
        }
        output
    } else {
        // Reuse the lossless protocol tags for nested bytes and semantic time.
        safe_text(&encode(value).to_string())
    }
}

/// A Unicode-scalar budget, including the ellipsis. The terminal renderer must
/// still clip to display-cell width (wide characters can occupy multiple cells).
pub fn preview(text: &str, max_chars: usize) -> String {
    let safe = safe_text(text);
    if safe.chars().count() <= max_chars {
        return safe;
    }
    if max_chars == 0 {
        return String::new();
    }
    safe.chars().take(max_chars - 1).chain(['…']).collect()
}

pub fn kind_label(node: &NodeView) -> &'static str {
    match node {
        NodeView::State { .. } => "state",
        NodeView::Desired { .. } => "desired",
        NodeView::Event { .. } => "event",
        NodeView::Command { .. } => "command",
    }
}

fn current(node: &NodeView) -> Option<&CurrentValueView> {
    match node {
        NodeView::State { current }
        | NodeView::Desired {
            current: Some(current),
            ..
        } => Some(current),
        _ => None,
    }
}

pub fn value_type(node: &NodeView) -> &'static str {
    let Some(current) = current(node) else {
        return if matches!(node, NodeView::Desired { .. }) {
            "absent"
        } else {
            "none (instant)"
        };
    };
    match current.value.as_inner() {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Bytes(_) => "bytes",
        Value::List(_) => "list",
        Value::Map(_) => "map",
        Value::Timestamp(_) => "timestamp",
        Value::Duration(_) => "duration",
    }
}

/// Complete, untruncated logical detail lines, ready for later pane scrolling.
/// A prefix has a path but no node, which differs from an absent desired payload.
pub fn details(path: &TopicPath, node: Option<&NodeView>) -> Vec<String> {
    let mut lines = vec![format!("Path: {}", safe_text(&path.to_string()))];
    if path.is_system() {
        lines.push("System topic".into());
    }
    let Some(node) = node else {
        lines.push("No node at this path (prefix or absent topic)".into());
        return lines;
    };
    lines.push(format!("Kind: {}", kind_label(node)));
    lines.push(format!("Value type: {}", value_type(node)));
    if let Some(current) = current(node) {
        lines.push(format!("Value: {}", value_text(current.value.as_inner())));
        provenance(&mut lines, &current.last_write);
        lines.push(format!(
            "Expires: {}",
            current
                .expires_at
                .as_deref()
                .map(safe_text)
                .unwrap_or_else(|| "none".into())
        ));
    } else if matches!(node, NodeView::Desired { .. }) {
        lines.push("No desired payload".into());
    } else {
        lines.push("No retained payload (instant node; no occurrence history)".into());
    }
    match node {
        NodeView::Event { last_publisher } => provenance(&mut lines, last_publisher),
        NodeView::Desired {
            claim: input_claim, ..
        }
        | NodeView::Command {
            claim: input_claim, ..
        } => claim(&mut lines, input_claim.as_ref()),
        NodeView::State { .. } => {}
    }
    lines.push("Schema: not available from this server API".into());
    lines
}

fn provenance(lines: &mut Vec<String>, provenance: &ProvenanceView) {
    lines.push(format!(
        "Writer/publisher: {}",
        safe_text(&provenance.client)
    ));
    lines.push(format!(
        "Session: {}",
        provenance
            .session
            .map_or_else(|| "stateless".into(), |id| id.to_string())
    ));
    lines.push(format!("Updated: {}", safe_text(&provenance.at)));
}

fn claim(lines: &mut Vec<String>, claim: Option<&ClaimView>) {
    if let Some(claim) = claim {
        lines.push(format!(
            "Claim: {} by {} (session {})",
            claim.id,
            safe_text(&claim.owner),
            claim.session
        ));
        lines.push(format!(
            "Claim release: {}",
            match &claim.release {
                ClaimReleaseView::Immediate => "immediate".into(),
                ClaimReleaseView::After { duration } => format!("after {}", safe_text(duration)),
            }
        ));
    } else {
        lines.push("Claim: unclaimed".into());
    }
}
