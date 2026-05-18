//! View for `fl pub <subcommand>`. The variant of `PubEvent` selects the layout.

use crate::theme::Theme;
use crate::view::View;
use fl_core::{KeyEvent as FlKey, LogLevel, OutdatedRow, PubDepKind, PubEvent, PubTreeNode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::time::Duration;

pub enum PubMode {
    GetOrUpgrade {
        added: Vec<String>,
        removed: Vec<String>,
        modified: Vec<(String, String, String)>,
    },
    Outdated {
        rows: Vec<OutdatedRow>,
    },
    Deps {
        tree: Option<PubTreeNode>,
    },
}

pub struct PubView {
    pub title: String,
    pub mode: PubMode,
    pub log: Vec<(LogLevel, String)>,
    pub done: bool,
    pub success: bool,
    pub quitting: bool,
}

impl PubView {
    pub fn for_get_or_upgrade(label: &str) -> Self {
        Self {
            title: label.into(),
            mode: PubMode::GetOrUpgrade {
                added: Vec::new(),
                removed: Vec::new(),
                modified: Vec::new(),
            },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
    pub fn for_outdated() -> Self {
        Self {
            title: "outdated".into(),
            mode: PubMode::Outdated { rows: Vec::new() },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
    pub fn for_deps() -> Self {
        Self {
            title: "deps".into(),
            mode: PubMode::Deps { tree: None },
            log: Vec::new(),
            done: false,
            success: false,
            quitting: false,
        }
    }
}

impl View for PubView {
    type Input = PubEvent;

    fn apply(&mut self, input: Self::Input) {
        match input {
            PubEvent::Resolving => {}
            PubEvent::Got { added, removed, modified } => {
                if let PubMode::GetOrUpgrade { added: a, removed: r, modified: m } = &mut self.mode {
                    *a = added;
                    *r = removed;
                    *m = modified;
                }
            }
            PubEvent::Outdated { rows } => {
                if let PubMode::Outdated { rows: target } = &mut self.mode {
                    *target = rows;
                }
            }
            PubEvent::Deps { tree } => {
                if let PubMode::Deps { tree: t } = &mut self.mode {
                    *t = Some(tree);
                }
            }
            PubEvent::Log { level, message } => {
                self.log.push((level, message));
            }
            PubEvent::Done { success } => {
                self.done = true;
                self.success = success;
                self.quitting = true;
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(area);

        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent).bg(theme.bg))
            .style(theme.base());
        let inner = header_block.inner(layout[0]);
        header_block.render(layout[0], buf);
        Paragraph::new(Line::styled(format!(" fl pub ── {} ", self.title), theme.header())).render(inner, buf);

        let body_block = Block::default().borders(Borders::ALL).border_style(theme.dimmed()).style(theme.base());
        let inner = body_block.inner(layout[1]);
        body_block.render(layout[1], buf);

        let lines = match &self.mode {
            PubMode::GetOrUpgrade { added, removed, modified } => {
                let mut out = Vec::new();
                for a in added {
                    out.push(Line::styled(format!("+ {a}"), Style::default().fg(theme.success).bg(theme.bg)));
                }
                for r in removed {
                    out.push(Line::styled(format!("- {r}"), Style::default().fg(theme.error).bg(theme.bg)));
                }
                for (name, was, new) in modified {
                    out.push(Line::styled(
                        format!("> {name}  {was} → {new}"),
                        Style::default().fg(theme.warn).bg(theme.bg),
                    ));
                }
                out
            }
            PubMode::Outdated { rows } => {
                let mut out = vec![Line::styled(
                    format!("{:<28} {:<10} {:<10} {:<10} {:<10}", "Package", "Current", "Upgradable", "Resolvable", "Latest"),
                    theme.header(),
                )];
                for row in rows {
                    out.push(Line::from(vec![
                        ratatui::text::Span::styled(format!("{:<28} ", row.package), theme.base()),
                        ratatui::text::Span::styled(format!("{:<10} ", row.current), theme.dimmed()),
                        ratatui::text::Span::styled(format!("{:<10} ", row.upgradable), Style::default().fg(theme.warn).bg(theme.bg)),
                        ratatui::text::Span::styled(format!("{:<10} ", row.resolvable), Style::default().fg(theme.cyan).bg(theme.bg)),
                        ratatui::text::Span::styled(format!("{:<10}", row.latest), Style::default().fg(theme.success).bg(theme.bg)),
                    ]));
                }
                out
            }
            PubMode::Deps { tree } => {
                let mut out = Vec::new();
                if let Some(t) = tree {
                    write_tree(t, 0, theme, &mut out);
                }
                out
            }
        };
        Paragraph::new(lines).render(inner, buf);
    }

    fn handle_key(&mut self, key: FlKey) -> Option<Self::Input> {
        if matches!(key, FlKey::Char('q') | FlKey::Ctrl('c')) {
            self.quitting = true;
        }
        None
    }
    fn tick(&mut self, _dt: Duration) {}
    fn quitting(&self) -> bool { self.quitting }
}

fn write_tree(node: &PubTreeNode, depth: usize, theme: &Theme, out: &mut Vec<Line<'static>>) {
    let indent = "  ".repeat(depth);
    let color = match node.kind {
        PubDepKind::Direct => theme.accent,
        PubDepKind::Dev => theme.cyan,
        PubDepKind::Transitive => theme.dim,
    };
    out.push(Line::styled(
        format!("{indent}{} {}", node.name, node.version),
        Style::default().fg(color).bg(theme.bg),
    ));
    for child in &node.children {
        write_tree(child, depth + 1, theme, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn got_event_populates_lists() {
        let mut v = PubView::for_get_or_upgrade("get");
        v.apply(PubEvent::Got {
            added: vec!["a".into()],
            removed: vec!["b".into()],
            modified: vec![("c".into(), "1.0".into(), "2.0".into())],
        });
        match v.mode {
            PubMode::GetOrUpgrade { added, removed, modified } => {
                assert_eq!(added, vec!["a"]);
                assert_eq!(removed, vec!["b"]);
                assert_eq!(modified.len(), 1);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn outdated_event_replaces_rows() {
        let mut v = PubView::for_outdated();
        v.apply(PubEvent::Outdated {
            rows: vec![OutdatedRow {
                package: "http".into(),
                current: "0.13.5".into(),
                upgradable: "0.13.6".into(),
                resolvable: "0.14.0".into(),
                latest: "1.2.0".into(),
            }],
        });
        if let PubMode::Outdated { rows } = v.mode { assert_eq!(rows.len(), 1); } else { panic!() }
    }

    #[test]
    fn deps_event_sets_tree() {
        let mut v = PubView::for_deps();
        v.apply(PubEvent::Deps {
            tree: PubTreeNode {
                name: "myapp".into(),
                version: "1.0".into(),
                kind: PubDepKind::Direct,
                children: vec![],
            },
        });
        if let PubMode::Deps { tree } = v.mode { assert!(tree.is_some()); } else { panic!() }
    }

    #[test]
    fn done_sets_quitting() {
        let mut v = PubView::for_get_or_upgrade("get");
        v.apply(PubEvent::Done { success: true });
        assert!(v.quitting);
    }
}
