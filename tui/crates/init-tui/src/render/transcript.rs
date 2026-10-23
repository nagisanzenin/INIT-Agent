//! The transcript view (Ctrl-O, alternate screen; docs/tui.md §3.6): every block of this process,
//! a selection, and per-block expansion. Tool calls expand to the full call, the summary, the times
//! and an optional `detail`; thinking expands to its text.

use super::live::{RowOpts, block_rows, text_rows};
use super::md::MdState;
use super::{Line, Style};
use crate::model::{Kind, Transcript};
use std::time::Instant;

#[derive(Debug, Default, Clone, Copy)]
pub struct ViewState {
    /// Absolute block index of the selection.
    pub sel: usize,
    /// Absolute block index at the top of the screen.
    pub top: usize,
}

fn rows_of(t: &Transcript, abs: usize, width: usize, verbose: bool, now: Instant) -> Vec<Line> {
    let Some(b) = t.get(abs) else { return Vec::new() };
    if let Kind::Text { lines, .. } = &b.kind {
        let mut n = lines.len();
        while n > 0 && lines[n - 1].trim().is_empty() {
            n -= 1;
        }
        let rows = text_rows(lines, 0, n, MdState::default(), width.saturating_sub(2)).0;
        if !b.expanded && rows.len() > 6 {
            let mut r: Vec<Line> = rows[..5].to_vec();
            r.push(Line::from_span(format!("… {} more rows (Enter expands)", rows.len() - 5), Style::dim()));
            return r;
        }
        return rows;
    }
    let o = RowOpts { verbose, expanded: b.expanded, spin: 0, first: true };
    let mut rows = block_rows(&b.kind, width.saturating_sub(2), now, &o);
    if b.expanded && b.at_ms > 0 && !matches!(b.kind, Kind::Tool(_)) {
        rows.push(Line::from_span(format!("    at {}", crate::time::utc_hms(b.at_ms)), Style::dim()));
    }
    rows
}

/// Renders the view; adjusts `vs.top` so the selection is visible.
pub fn render(t: &Transcript, vs: &mut ViewState, width: usize, height: usize, verbose: bool, now: Instant) -> Vec<Line> {
    let mut out = vec![
        Line::from_span(" transcript · j/k move · Enter toggle · e expand all · c collapse all · g/G top/bottom · q back", Style::PLAIN.with_reverse()).pad(width, Style::PLAIN.with_reverse()).truncate(width),
    ];
    let body = height.saturating_sub(1);
    if t.is_empty() {
        out.push(Line::from_span("  (nothing yet)", Style::dim()));
        return out;
    }
    let first = t.base;
    let last = t.len() - 1;
    vs.sel = vs.sel.clamp(first, last);
    vs.top = vs.top.clamp(first, last);
    if vs.sel < vs.top {
        vs.top = vs.sel;
    }
    // move the top down until the selection's rows fit
    loop {
        let mut used = 0;
        let mut fits = false;
        for i in vs.top..=vs.sel {
            used += rows_of(t, i, width, verbose, now).len().max(1);
        }
        if used <= body || vs.top == vs.sel {
            fits = true;
        }
        if fits {
            break;
        }
        vs.top += 1;
    }
    let mut i = vs.top;
    while out.len() < height && i <= last {
        let rows = rows_of(t, i, width, verbose, now);
        let rows = if rows.is_empty() { vec![Line::new()] } else { rows };
        for (k, r) in rows.into_iter().enumerate() {
            if out.len() >= height {
                break;
            }
            let marker = if i == vs.sel && k == 0 { "▶ " } else { "  " };
            let mut l = Line::from_span(marker, Style::bold());
            l.extend(r);
            out.push(l.truncate(width));
        }
        i += 1;
    }
    out
}

pub fn page(height: usize) -> usize {
    height.saturating_sub(2).max(1)
}
