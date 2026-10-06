//! Unified sidebar layout: each workspace block lists its panes grouped under tab
//! headers, replacing the separate agent section. Upstream render paths call
//! in through one-statement hooks that return their input unchanged in
//! classic layout.

mod upstream;

use std::borrow::Cow;
use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use upstream::{
    put_segment, put_text, AgentFacts, AgentStatus, AgentsSidebarConfig, ClientEndpointId,
    ClientShellConfig, ClientShellEndpoint, ClientShellInput, ClientShellSnapshot,
    ClientShellState, ClientShellWorkspace, Entry, Palette, PaneFacts, ShellHitMap,
    ShellRenderState, TabFacts, Token, WorkspaceEntry, WorkspaceFacts,
};

/// Blank rows between workspace blocks. Children hug their parent; every
/// other block is separated by one row.
pub(super) const WORKSPACE_ROW_GAP: u16 = 1;

pub(super) struct WorkspacePaneRow<'a> {
    pub(super) pane_id: &'a str,
    pub(super) status: AgentStatus,
    pub(super) kind: &'a str,
    pub(super) label: Option<&'a str>,
    /// Agent text flattened to one line. Empty for shell panes and for
    /// layouts that resolve to nothing, which fall back to `kind · label`.
    pub(super) tokens: Vec<Token>,
    /// Reserved summary line for agent panes whose layout has a `$summary`.
    /// `Some` even without a reported summary, so rows never shift when one
    /// arrives; `None` for shell panes and layouts without `$summary`.
    pub(super) detail: Option<Vec<Token>>,
    pub(super) focused: bool,
    pub(super) branch: PaneBranch,
}

impl WorkspacePaneRow<'_> {
    fn display_height(&self) -> usize {
        1 + usize::from(self.detail.is_some())
    }
}

pub(super) struct WorkspacePaneSection<'a> {
    pub(super) tab_label: &'a str,
    pub(super) tab_focused: bool,
    pub(super) rows: Vec<WorkspacePaneRow<'a>>,
}

/// Tree glyph joining a pane row to its tab header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneBranch {
    Single,
    First,
    Middle,
    Last,
}

impl PaneBranch {
    pub(super) fn glyph(self) -> &'static str {
        match self {
            PaneBranch::Single | PaneBranch::Last => "└─",
            PaneBranch::First | PaneBranch::Middle => "├─",
        }
    }
}

/// Pane rows for every workspace in one snapshot, built once per frame so the
/// height and draw passes share one walk over panes, tabs, and agents. Unlike
/// the agent list this walks every pane, so plain shells show up too. Panes
/// group by tab in bar order, keeping snapshot order within a tab.
pub(super) struct WorkspacePanes<'a> {
    by_workspace: HashMap<&'a str, Vec<WorkspacePaneSection<'a>>>,
}

impl<'a> WorkspacePanes<'a> {
    pub(super) fn new(
        snapshot: &'a ClientShellSnapshot,
        agents_config: &AgentsSidebarConfig,
    ) -> Self {
        let agents_config = upstream::pane_text_config(agents_config);
        let agents = upstream::agents(snapshot)
            .map(|agent| (agent.pane_id, agent))
            .collect::<HashMap<_, _>>();
        let tabs = upstream::tabs(snapshot)
            .enumerate()
            .map(|(position, tab)| (tab.tab_id, (position, tab)))
            .collect::<HashMap<_, _>>();
        let tab_position = |pane: &PaneFacts<'_>| {
            tabs.get(pane.tab_id)
                .map_or(usize::MAX, |(position, _)| *position)
        };
        let mut panes_by_workspace: HashMap<&str, Vec<PaneFacts<'a>>> = HashMap::new();
        for pane in upstream::panes(snapshot) {
            panes_by_workspace
                .entry(pane.workspace_id)
                .or_default()
                .push(pane);
        }
        let by_workspace = panes_by_workspace
            .into_iter()
            .map(|(workspace_id, mut panes)| {
                panes.sort_by_key(|pane| tab_position(pane));
                let mut sections: Vec<WorkspacePaneSection<'a>> = Vec::new();
                for (index, pane) in panes.iter().enumerate() {
                    let first = index == 0 || panes[index - 1].tab_id != pane.tab_id;
                    let last = index + 1 == panes.len() || panes[index + 1].tab_id != pane.tab_id;
                    let branch = match (first, last) {
                        (true, true) => PaneBranch::Single,
                        (true, false) => PaneBranch::First,
                        (false, true) => PaneBranch::Last,
                        (false, false) => PaneBranch::Middle,
                    };
                    let row = pane_row(pane, agents.get(pane.pane_id), &agents_config, branch);
                    match sections.last_mut() {
                        Some(section) if !first => section.rows.push(row),
                        _ => {
                            let tab: Option<&TabFacts<'a>> =
                                tabs.get(pane.tab_id).map(|(_, tab)| tab);
                            sections.push(WorkspacePaneSection {
                                tab_label: tab.map_or(pane.tab_id, |tab| tab.label),
                                tab_focused: tab.is_some_and(|tab| tab.focused),
                                rows: vec![row],
                            });
                        }
                    }
                }
                (workspace_id, sections)
            })
            .collect();
        Self { by_workspace }
    }

    pub(super) fn sections(&self, workspace_id: &str) -> &[WorkspacePaneSection<'a>] {
        self.by_workspace
            .get(workspace_id)
            .map_or(&[], Vec::as_slice)
    }

    /// One header row per tab group, even a lone tab, plus one row per pane.
    pub(super) fn display_height(&self, workspace_id: &str) -> usize {
        sections_height(self.sections(workspace_id))
    }

    #[cfg(test)]
    pub(super) fn into_sections(mut self, workspace_id: &str) -> Vec<WorkspacePaneSection<'a>> {
        self.by_workspace.remove(workspace_id).unwrap_or_default()
    }
}

fn sections_height(sections: &[WorkspacePaneSection<'_>]) -> usize {
    sections
        .iter()
        .map(|section| {
            section
                .rows
                .iter()
                .map(WorkspacePaneRow::display_height)
                .sum::<usize>()
                + 1
        })
        .sum()
}

fn pane_row<'a>(
    pane: &PaneFacts<'a>,
    agent: Option<&AgentFacts<'a>>,
    agents_config: &AgentsSidebarConfig,
    branch: PaneBranch,
) -> WorkspacePaneRow<'a> {
    let pane_label = pane.label.filter(|label| !label.is_empty());
    // A pane with no detected agent was never classified, so its status is
    // unknown rather than idle. Shell rows show the callsign, falling back to
    // the constant kind.
    let Some(agent) = agent else {
        return WorkspacePaneRow {
            pane_id: pane.pane_id,
            status: AgentStatus::Unknown,
            kind: "shell",
            label: pane_label,
            tokens: Vec::new(),
            detail: None,
            focused: pane.focused,
            branch,
        };
    };
    let label = agent.title.or(pane_label);
    let text = upstream::agent_tokens(agent, label, agents_config);
    WorkspacePaneRow {
        pane_id: pane.pane_id,
        status: agent.status,
        kind: agent.kind.unwrap_or("shell"),
        label,
        tokens: text.line,
        detail: text.detail,
        focused: pane.focused,
        branch,
    }
}

/// Workspace header rows: the repo name (plus ` · label` when renamed),
/// then branch and ahead/behind when known, for every workspace including
/// linked worktree children.
pub(super) fn header_rows(workspace: &ClientShellWorkspace, _indented: bool) -> Vec<Vec<Token>> {
    let facts = upstream::workspace(workspace);
    let mut rows = vec![vec![upstream::workspace_token(
        header_label(&facts, _indented).into_owned(),
    )]];
    let mut details = Vec::new();
    if let Some(branch) = facts.branch {
        details.push(upstream::branch_token(branch));
    }
    if let Some((ahead, behind)) = facts
        .ahead_behind
        .filter(|(ahead, behind)| *ahead > 0 || *behind > 0)
    {
        details.push(upstream::git_status_token(ahead, behind));
    }
    if !details.is_empty() {
        rows.push(details);
    }
    rows
}

/// First row is always the repo name (the worktree repo, the local git repo
/// resolving through linked checkouts, or the directory name outside repos),
/// plus the custom label when one is set: `repo` or `repo · label`. The repo
/// leads so sibling worktrees of one repo share one name even when checked
/// out at different paths.
fn header_label<'w>(facts: &WorkspaceFacts<'w>, _indented: bool) -> Cow<'w, str> {
    let repo: Option<Cow<'w, str>> = facts
        .repo
        .map(Cow::Borrowed)
        .or_else(|| upstream::repo_name_for_cwd(facts.cwd).map(Cow::Owned))
        .or_else(|| cwd_dir_name(facts.cwd).map(Cow::Borrowed));
    match (repo, facts.custom_label) {
        (Some(repo), true) => Cow::Owned(format!("{repo} · {}", facts.label)),
        (Some(repo), false) => repo,
        (None, _) => Cow::Borrowed(facts.label),
    }
}

fn cwd_dir_name(cwd: &str) -> Option<&str> {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

/// Collapsed unified layout lists workspaces only: they fill the rail except the
/// last row, which the sidebar toggle owns.
pub(super) fn collapsed(
    config: &ClientShellConfig,
    area: Rect,
    sections: (Rect, Option<u16>, Rect),
) -> (Rect, Option<u16>, Rect) {
    if !upstream::unified_layout(config) {
        return sections;
    }
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return (Rect::default(), None, Rect::default());
    }
    (
        Rect::new(
            content.x,
            content.y,
            content.width,
            content.height.saturating_sub(1),
        ),
        None,
        Rect::default(),
    )
}

pub(super) fn handle_click(
    shell: &mut ClientShellState,
    point: (u16, u16),
    outcome: &mut ClientShellInput,
) -> bool {
    let Some((endpoint_id, pane_id)) = upstream::pane_hit_at(shell, point) else {
        return false;
    };
    upstream::focus_pane(shell, endpoint_id, pane_id, outcome);
    true
}

struct Machine<'a> {
    id: &'a ClientEndpointId,
    snapshot: Option<&'a ClientShellSnapshot>,
    panes: Option<WorkspacePanes<'a>>,
}

struct Active<'a> {
    config: &'a ClientShellConfig,
    active_id: &'a ClientEndpointId,
    machines: Vec<Machine<'a>>,
    /// `endpoint_sidebar` row order; empty for the single-machine sidebar.
    rows: Vec<Option<(usize, WorkspaceEntry)>>,
}

/// Per-frame unified layout state for one expanded sidebar render. Inert in
/// classic layout: every hook returns its input unchanged.
pub(super) struct SidebarPanes<'a> {
    active: Option<Active<'a>>,
}

impl<'a> SidebarPanes<'a> {
    pub(super) fn local(
        snapshot: &'a ClientShellSnapshot,
        config: &'a ClientShellConfig,
        active_id: &'a ClientEndpointId,
    ) -> Self {
        if !upstream::unified_layout(config) {
            return Self { active: None };
        }
        let machine = Machine {
            id: active_id,
            snapshot: Some(snapshot),
            panes: Some(WorkspacePanes::new(
                snapshot,
                upstream::agents_config(config),
            )),
        };
        Self {
            active: Some(Active {
                config,
                active_id,
                machines: vec![machine],
                rows: Vec::new(),
            }),
        }
    }

    pub(super) fn machines<'s: 'a>(
        config: &'a ClientShellConfig,
        state: &ShellRenderState<'s>,
    ) -> Self {
        if !upstream::unified_layout(config) {
            return Self { active: None };
        }
        let layout = upstream::machine_layout(state);
        let agents_config = upstream::agents_config(config);
        let machines = layout
            .machines
            .into_iter()
            .map(|machine| Machine {
                id: machine.id,
                snapshot: machine.snapshot,
                panes: machine
                    .snapshot
                    .map(|snapshot| WorkspacePanes::new(snapshot, agents_config)),
            })
            .collect();
        Self {
            active: Some(Active {
                config,
                active_id: state.active_endpoint_id,
                machines,
                rows: layout.rows,
            }),
        }
    }

    /// The workspace section takes the whole sidebar: no agent section and no
    /// draggable divider.
    pub(super) fn areas(
        &self,
        area: Rect,
        workspace_area: Rect,
        detail_area: Rect,
        hits: &mut ShellHitMap,
    ) -> (Rect, Rect) {
        if self.active.is_none() {
            return (workspace_area, detail_area);
        }
        upstream::clear_section_divider(hits);
        let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
        if content.is_empty() {
            (Rect::default(), Rect::default())
        } else {
            (content, Rect::default())
        }
    }

    /// Without an agent section the footer shares its row with the sidebar
    /// toggle, so the menu launcher ends before the toggle cell.
    pub(super) fn footer(&self, workspace_area: Rect, area: Rect) -> Rect {
        if self.active.is_none() {
            return workspace_area;
        }
        let right = area.right().saturating_sub(3).max(workspace_area.x);
        Rect::new(
            workspace_area.x,
            workspace_area.y,
            right.saturating_sub(workspace_area.x),
            workspace_area.height,
        )
    }

    pub(super) fn entry_heights(&self, entries: &[WorkspaceEntry], heights: Vec<u16>) -> Vec<u16> {
        let Some(active) = &self.active else {
            return heights;
        };
        entries
            .iter()
            .map(|entry| active.workspace_height(0, entry))
            .collect()
    }

    pub(super) fn entry_gaps(&self, entries: &[WorkspaceEntry], gaps: Vec<u16>) -> Vec<u16> {
        if self.active.is_none() {
            return gaps;
        }
        (0..entries.len())
            .map(|index| gap_before(entries.get(index + 1)))
            .collect()
    }

    pub(super) fn gap(&self, gap: u16, next: Option<&WorkspaceEntry>) -> u16 {
        if self.active.is_none() {
            return gap;
        }
        gap_before(next)
    }

    pub(super) fn row_heights(&self, heights: Vec<u16>) -> Vec<u16> {
        let Some(active) = &self.active else {
            return heights;
        };
        debug_assert_eq!(active.rows.len(), heights.len(), "machine rows drifted");
        active
            .rows
            .iter()
            .map(|row| {
                row.map_or(1, |(machine, entry)| {
                    active.workspace_height(machine, &entry)
                })
            })
            .collect()
    }

    pub(super) fn row_gaps(&self, gaps: Vec<u16>) -> Vec<u16> {
        let Some(active) = &self.active else {
            return gaps;
        };
        debug_assert_eq!(active.rows.len(), gaps.len(), "machine rows drifted");
        active
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| match (row, active.rows.get(index + 1)) {
                (Some((machine, _)), Some(Some((next_machine, next))))
                    if machine == next_machine =>
                {
                    gap_before(Some(next))
                }
                _ => 0,
            })
            .collect()
    }

    /// Hands upstream an empty header so it only paints the block background;
    /// `render` draws the header and pane rows.
    pub(super) fn block(
        &self,
        workspace: &ClientShellWorkspace,
        entry: &WorkspaceEntry,
        rows: Vec<Vec<Token>>,
        height: u16,
        body: Rect,
    ) -> (Vec<Vec<Token>>, u16) {
        match &self.active {
            None => (rows, height),
            Some(active) => (
                Vec::new(),
                active.block_height(0, workspace, entry).min(body.height),
            ),
        }
    }

    pub(super) fn machine_block(
        &self,
        endpoint: &ClientShellEndpoint,
        workspace: &ClientShellWorkspace,
        entry: &WorkspaceEntry,
        rows: Vec<Vec<Token>>,
        height: u16,
        body: Rect,
    ) -> (Vec<Vec<Token>>, u16) {
        match &self.active {
            None => (rows, height),
            Some(active) => (
                Vec::new(),
                active
                    .block_height(active.machine_index(endpoint), workspace, entry)
                    .min(body.height),
            ),
        }
    }

    pub(super) fn render(
        &self,
        buffer: &mut Buffer,
        rect: Rect,
        entry: &WorkspaceEntry,
        workspace: &ClientShellWorkspace,
        selected: bool,
        dragged: bool,
        hits: &mut ShellHitMap,
    ) {
        if let Some(active) = &self.active {
            let block = Block {
                machine: 0,
                entry,
                workspace,
            };
            active.render_block(buffer, rect, block, true, selected, dragged, hits);
        }
    }

    pub(super) fn render_machine(
        &self,
        buffer: &mut Buffer,
        rect: Rect,
        endpoint: &ClientShellEndpoint,
        entry: &WorkspaceEntry,
        workspace: &ClientShellWorkspace,
        selected: bool,
        hits: &mut ShellHitMap,
    ) {
        let Some(active) = &self.active else {
            return;
        };
        let machine = active.machine_index(endpoint);
        let show_focus = active
            .machines
            .get(machine)
            .is_some_and(|machine| machine.id == active.active_id);
        let block = Block {
            machine,
            entry,
            workspace,
        };
        active.render_block(buffer, rect, block, show_focus, selected, false, hits);
    }
}

struct Block<'w> {
    machine: usize,
    entry: &'w WorkspaceEntry,
    workspace: &'w ClientShellWorkspace,
}

fn gap_before(next: Option<&WorkspaceEntry>) -> u16 {
    next.map_or(0, |next| {
        u16::from(!upstream::entry(next).indented) * WORKSPACE_ROW_GAP
    })
}

impl<'a> Active<'a> {
    fn machine_index(&self, endpoint: &ClientShellEndpoint) -> usize {
        let id = upstream::endpoint_id(endpoint);
        self.machines
            .iter()
            .position(|machine| machine.id == id)
            .unwrap_or(usize::MAX)
    }

    fn sections(&self, machine: usize, workspace_id: &str) -> &[WorkspacePaneSection<'a>] {
        self.machines
            .get(machine)
            .and_then(|machine| machine.panes.as_ref())
            .map_or(&[], |panes| panes.sections(workspace_id))
    }

    fn workspace_height(&self, machine: usize, entry: &WorkspaceEntry) -> u16 {
        self.machines
            .get(machine)
            .and_then(|machine| machine.snapshot)
            .and_then(|snapshot| upstream::workspace_at(snapshot, upstream::entry(entry).index))
            .map_or(1, |workspace| self.block_height(machine, workspace, entry))
    }

    fn block_height(
        &self,
        machine: usize,
        workspace: &ClientShellWorkspace,
        entry: &WorkspaceEntry,
    ) -> u16 {
        let header = header_rows(workspace, upstream::entry(entry).indented).len();
        let workspace_id = upstream::workspace(workspace).workspace_id;
        let panes = self
            .machines
            .get(machine)
            .and_then(|machine| machine.panes.as_ref())
            .map_or(0, |panes| panes.display_height(workspace_id));
        (header + panes).clamp(1, usize::from(u16::MAX)) as u16
    }

    fn render_block(
        &self,
        buffer: &mut Buffer,
        rect: Rect,
        block: Block<'_>,
        show_focus: bool,
        selected: bool,
        dragged: bool,
        hits: &mut ShellHitMap,
    ) {
        let entry = upstream::entry(block.entry);
        let facts = upstream::workspace(block.workspace);
        let rows = header_rows(block.workspace, entry.indented);
        let palette = upstream::palette(self.config);
        render_header(
            buffer,
            rect,
            &entry,
            &rows,
            show_focus && facts.focused,
            selected,
            dragged,
            palette,
        );
        let Some(endpoint_id) = self.machines.get(block.machine).map(|machine| machine.id) else {
            return;
        };
        let start_y = rect.y.saturating_add(rows.len() as u16).min(rect.bottom());
        let rows_area = Rect::new(rect.x, start_y, rect.width, rect.bottom() - start_y);
        let sections = self.sections(block.machine, facts.workspace_id);
        // Summaries lift for the navigate cursor; workspace or tab focus
        // alone never promotes another pane's summary.
        let target = PaneRowsTarget {
            entry: &entry,
            endpoint_id,
            show_focus,
            summary_lift: selected,
        };
        render_pane_rows(buffer, rows_area, sections, target, self.config, hits);
        render_group_bar(
            buffer,
            rect,
            (show_focus && facts.focused) || selected || dragged,
            palette,
        );
    }
}

struct PaneRowsTarget<'t> {
    entry: &'t Entry,
    endpoint_id: &'t ClientEndpointId,
    /// False for machines other than the active one, which never show focus.
    show_focus: bool,
    /// Whether the navigate cursor lifts summaries to secondary.
    /// Workspace or tab focus alone never does; only the focused pane's
    /// own summary steps up with it.
    summary_lift: bool,
}

/// Mirrors upstream's header text styling. Rows after the name sit one cell
/// in, under the name, because headers carry no status icon. Content starts
/// one cell past the group bar, leaving a blank column between the bar and
/// the text.
fn render_header(
    buffer: &mut Buffer,
    area: Rect,
    entry: &Entry,
    rows: &[Vec<Token>],
    focused: bool,
    selected: bool,
    dragged: bool,
    palette: &Palette,
) {
    // The name stays primary in every state; weight alone marks focus and
    // the navigate cursor. The branch line stays tertiary in every state.
    let emphasized = focused || selected || dragged;
    let workspace_style = Style::default()
        .fg(palette.text)
        .add_modifier(if emphasized {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let secondary_style = Style::default().fg(palette.overlay0);
    for (row_index, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_index as u16);
        if y >= area.bottom() {
            break;
        }
        let x = if entry.indented {
            let prefix = match (row_index == 0, entry.last_child) {
                (true, true) => "   └─ ",
                (true, false) => "   ├─ ",
                (false, true) => "      ",
                (false, false) => "   │  ",
            };
            put_segment(
                buffer,
                area.x.saturating_add(1),
                y,
                area.right(),
                prefix,
                Style::default().fg(palette.overlay0),
            )
        } else {
            area.x.saturating_add(2)
        };
        let width = area.right().saturating_sub(2).saturating_sub(x);
        let spans = upstream::token_spans(
            row,
            Style::default(),
            workspace_style,
            secondary_style,
            Style::default().fg(palette.overlay1),
            palette,
            usize::from(width),
        );
        Paragraph::new(Line::from(spans)).render(Rect::new(x, y, width, 1), buffer);
    }
}

/// Draws tab headers and pane rows top-down in `rect`, recording one click
/// target per visible pane row.
fn render_pane_rows(
    buffer: &mut Buffer,
    rect: Rect,
    sections: &[WorkspacePaneSection<'_>],
    target: PaneRowsTarget<'_>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let PaneRowsTarget {
        entry,
        endpoint_id,
        show_focus,
        summary_lift,
    } = target;
    let palette = upstream::palette(config);
    let emphasis = |focused: bool| {
        if focused && show_focus {
            Modifier::BOLD
        } else {
            Modifier::empty()
        }
    };
    let record_hits = upstream::mouse_capture(config);
    let right = rect.right().saturating_sub(2);
    let left = rect.x.saturating_add(if entry.indented { 7 } else { 2 });
    let mut y = rect.y;
    for section in sections {
        if y >= rect.bottom() {
            return;
        }
        render_sibling_pipe(buffer, rect, y, entry, palette);
        // The selected tab goes primary without bold; the cursor never
        // lifts tabs, and unfocused ones sit with the branch line.
        let tab_style = if section.tab_focused && show_focus {
            Style::default().fg(palette.text)
        } else {
            Style::default().fg(palette.overlay0)
        };
        put_text(
            buffer,
            left,
            y,
            right.saturating_sub(left),
            section.tab_label,
            tab_style,
        );
        y = y.saturating_add(1);
        for pane_row in &section.rows {
            if y >= rect.bottom() {
                return;
            }
            render_sibling_pipe(buffer, rect, y, entry, palette);
            // Signal rows carry their dot hue in the title and the tree
            // pipe: working yellow, done teal-green, blocked red (bold when
            // focused). Idle and unknown stay as today: primary bold when
            // focused, tertiary otherwise. The cursor never changes row color.
            let text_style = match pane_row.status {
                AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done => Style::default()
                    .fg(upstream::status_color(pane_row.status, palette))
                    .add_modifier(emphasis(pane_row.focused)),
                _ => {
                    if pane_row.focused && show_focus {
                        Style::default()
                            .fg(palette.text)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(palette.overlay0)
                    }
                }
            };
            let mut x = put_segment(buffer, left, y, right, pane_row.branch.glyph(), text_style);
            // Unknown carries no signal (detection never ran), so it leaves a
            // blank instead of gluing a second dot onto the branch.
            x = if pane_row.status == AgentStatus::Unknown {
                put_segment(buffer, x, y, right, " ", Style::default())
            } else {
                put_segment(
                    buffer,
                    x,
                    y,
                    right,
                    upstream::status_icon(pane_row.status, config),
                    Style::default().fg(upstream::status_color(pane_row.status, palette)),
                )
            };
            if pane_row.tokens.is_empty() {
                // A shell row's label is its callsign, so the kind prefix
                // would only repeat what the blank status column says. Agent
                // rows that resolve to nothing keep `kind · label`.
                let text = match (pane_row.label, pane_row.kind) {
                    (Some(label), "shell") => format!(" {label}"),
                    (Some(label), kind) => format!(" {kind} · {label}"),
                    (None, kind) => format!(" {kind}"),
                };
                put_text(buffer, x, y, right.saturating_sub(x), &text, text_style);
            } else {
                let width = right.saturating_sub(x);
                let mut spans = vec![Span::raw(" ")];
                spans.extend(upstream::token_spans(
                    &pane_row.tokens,
                    Style::default().fg(upstream::status_color(pane_row.status, palette)),
                    text_style,
                    text_style,
                    text_style,
                    palette,
                    usize::from(width.saturating_sub(1)),
                ));
                Paragraph::new(Line::from(spans)).render(Rect::new(x, y, width, 1), buffer);
            }
            if record_hits {
                upstream::push_pane_hit(
                    hits,
                    Rect::new(rect.x, y, rect.width, 1),
                    endpoint_id,
                    pane_row.pane_id,
                );
            }
            y = y.saturating_add(1);
            if let Some(detail) = &pane_row.detail {
                if y >= rect.bottom() {
                    return;
                }
                render_sibling_pipe(buffer, rect, y, entry, palette);
                if matches!(pane_row.branch, PaneBranch::First | PaneBranch::Middle) {
                    put_segment(buffer, left, y, right, "│ ", text_style);
                }
                if !detail.is_empty() {
                    let width = right.saturating_sub(x);
                    let mut spans = vec![Span::raw(" ")];
                    // Summaries stay tertiary unless their own pane holds
                    // focus or the workspace carries the cursor, never bold.
                    let detail_style = if (pane_row.focused && show_focus) || summary_lift {
                        Style::default().fg(palette.subtext0)
                    } else {
                        Style::default().fg(palette.overlay0)
                    };
                    spans.extend(upstream::token_spans(
                        detail,
                        Style::default().fg(upstream::status_color(pane_row.status, palette)),
                        detail_style,
                        detail_style,
                        detail_style,
                        palette,
                        usize::from(width.saturating_sub(1)),
                    ));
                    Paragraph::new(Line::from(spans)).render(Rect::new(x, y, width, 1), buffer);
                }
                if record_hits {
                    upstream::push_pane_hit(
                        hits,
                        Rect::new(rect.x, y, rect.width, 1),
                        endpoint_id,
                        pane_row.pane_id,
                    );
                }
                y = y.saturating_add(1);
            }
        }
    }
}

/// Left bar marking each workspace block as one group. Bold once the
/// workspace is focused or carries the navigate cursor, matching the
/// header emphasis.
fn render_group_bar(buffer: &mut Buffer, rect: Rect, highlighted: bool, palette: &Palette) {
    if rect.is_empty() {
        return;
    }
    for y in rect.y..rect.bottom() {
        let bg = buffer[(rect.x, y)].bg;
        put_text(
            buffer,
            rect.x,
            y,
            1,
            "│",
            Style::default()
                .fg(if highlighted {
                    palette.text
                } else {
                    palette.overlay0
                })
                .bg(bg)
                .add_modifier(if highlighted {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
    }
}

/// Continues the worktree tree line past a child that has later siblings, as
/// its own header row does. Offset one cell for the group bar gap.
fn render_sibling_pipe(buffer: &mut Buffer, rect: Rect, y: u16, entry: &Entry, palette: &Palette) {
    if entry.indented && !entry.last_child {
        put_segment(
            buffer,
            rect.x.saturating_add(1),
            y,
            rect.right(),
            "   │",
            Style::default().fg(palette.overlay0),
        );
    }
}
