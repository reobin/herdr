//! Unified sidebar layout: each workspace block lists its panes grouped under tab
//! headers, replacing the separate agent section. Upstream render paths call
//! in through one-statement hooks that return their input unchanged in
//! classic layout.

mod upstream;

use std::collections::{HashMap, HashSet};

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

/// Label cells kept before the elapsed slot shows. Below this the first row
/// keeps its full width and the elapsed time gives up.
const MIN_ELAPSED_LABEL_WIDTH: u16 = 5;

/// Heartbeat for the working spinner and status elapsed times. Timer ticks
/// only recompose when a tick requests a repaint, so an idle shell would
/// freeze the animation between unrelated activity. The client Timer arm
/// ORs this into its repaint decision; true while unified layout has a
/// working agent to animate, a fresh stamp ticking through seconds, or a
/// settled stamp within a second past its minute flip. Settled rows
/// between flips correctly stay quiet; the next flip wakes them.
pub(crate) fn unified_sidebar_needs_repaint(shell: &ClientShellState) -> bool {
    if !upstream::unified_layout(&shell.config) {
        return false;
    }
    shell.snapshot.as_ref().is_some_and(|snapshot| {
        if snapshot
            .agents
            .iter()
            .any(|agent| agent.agent_status == AgentStatus::Working)
        {
            return true;
        }
        let now = now_ms();
        snapshot
            .agents
            .iter()
            .filter_map(|agent| agent.status_since_ms)
            .any(|since| {
                let age = now.saturating_sub(since);
                age < 60_000 || age % 60_000 < 1_000
            })
    })
}

pub(super) struct WorkspacePaneRow<'a> {
    pub(super) pane_id: &'a str,
    pub(super) status: AgentStatus,
    pub(super) kind: &'a str,
    pub(super) label: Option<&'a str>,
    /// Wall-clock millis the agent entered its current status, restamped on
    /// every transition. `None` for shell rows and older endpoints that
    /// never reported it.
    pub(super) status_since_ms: Option<u64>,
    /// Agent text flattened to one line. Empty for shell panes and for
    /// layouts that resolve to nothing, which fall back to `kind / label`.
    pub(super) tokens: Vec<Token>,
    /// Reserved summary line for agent panes whose layout has a `$summary`.
    /// `Some` even without a reported summary, so rows never shift when one
    /// arrives; `None` for shell panes and layouts without `$summary`.
    pub(super) detail: Option<Vec<Token>>,
    pub(super) focused: bool,
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
                    let row = pane_row(pane, agents.get(pane.pane_id), &agents_config);
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

    /// Tab headers show only when a workspace holds more than one tab.
    /// A lone tab skips its header, so the block reads workspace name,
    /// branch, then directly the spine and panes. Height counts one row
    /// per pane (plus reserved detail rows) and one header row per tab
    /// group when headers show.
    pub(super) fn display_height(&self, workspace_id: &str) -> usize {
        sections_height(self.sections(workspace_id))
    }

    #[cfg(test)]
    pub(super) fn into_sections(mut self, workspace_id: &str) -> Vec<WorkspacePaneSection<'a>> {
        self.by_workspace.remove(workspace_id).unwrap_or_default()
    }
}

fn sections_height(sections: &[WorkspacePaneSection<'_>]) -> usize {
    let hide_tabs = sections.len() == 1;
    sections
        .iter()
        .map(|section| {
            section
                .rows
                .iter()
                .map(WorkspacePaneRow::display_height)
                .sum::<usize>()
                + usize::from(!hide_tabs)
        })
        .sum()
}

fn pane_row<'a>(
    pane: &PaneFacts<'a>,
    agent: Option<&AgentFacts<'a>>,
    agents_config: &AgentsSidebarConfig,
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
            status_since_ms: None,
            tokens: Vec::new(),
            detail: None,
            focused: pane.focused,
        };
    };
    let label = agent.title.or(pane_label);
    let text = upstream::agent_tokens(agent, label, agents_config);
    WorkspacePaneRow {
        pane_id: pane.pane_id,
        status: agent.status,
        kind: agent.kind.unwrap_or("shell"),
        label,
        status_since_ms: agent.status_since_ms,
        tokens: text.line,
        detail: text.detail,
        focused: pane.focused,
    }
}

/// Short status elapsed for the right edge of agent first rows: `7s` under
/// a minute, then `3m`, `12m`, `1h05m`. The sub-minute seconds keep fresh
/// rows lively; the repaint heartbeat below wakes the client every tick, so
/// the seconds text tracks it for free.
fn format_elapsed(status_since_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(status_since_ms) / 1_000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3_600, secs % 3_600 / 60)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Workspace header rows: the repo name (plus ` / label` when renamed),
/// then branch and ahead/behind when known, for every workspace including
/// linked worktree children. The renamed first row carries two tokens so
/// both sides render primary with a quiet slash divider.
pub(super) fn header_rows(workspace: &ClientShellWorkspace, _indented: bool) -> Vec<Vec<Token>> {
    let facts = upstream::workspace(workspace);
    let (repo, name) = header_names(&facts);
    let mut rows = match (repo, name) {
        (Some(repo), name) => vec![vec![
            upstream::branch_token(&repo),
            upstream::workspace_token(name),
        ]],
        (None, name) => vec![vec![upstream::workspace_token(name)]],
    };
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

/// Split header name into its repo context and display name. Renamed
/// workspaces show both (`repo / label`); everything else shows one name:
/// the worktree repo, the local git repo resolving through linked
/// checkouts, the directory name outside repos, or the label with no repo.
/// The repo leads so sibling worktrees of one repo share one context even
/// when checked out at different paths.
fn header_names(facts: &WorkspaceFacts<'_>) -> (Option<String>, String) {
    let repo: Option<String> = facts
        .repo
        .map(str::to_owned)
        .or_else(|| upstream::repo_name_for_cwd(facts.cwd))
        .or_else(|| cwd_dir_name(facts.cwd).map(str::to_owned));
    if facts.custom_label {
        match repo {
            Some(repo) => (Some(repo), facts.label.to_owned()),
            None => (None, facts.label.to_owned()),
        }
    } else {
        (None, repo.unwrap_or_else(|| facts.label.to_owned()))
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
    if let Some((endpoint_id, workspace_id)) = upstream::collapse_toggle_at(shell, point) {
        upstream::toggle_workspace_collapsed(shell, &endpoint_id, &workspace_id, outcome);
        return true;
    }
    let Some((endpoint_id, pane_id)) = upstream::pane_hit_at(shell, point) else {
        return false;
    };
    upstream::focus_pane(shell, endpoint_id, pane_id, outcome);
    true
}

pub(super) fn handle_prefix_binding(
    shell: &mut ClientShellState,
    binding: &crate::input::KeybindMatch,
    outcome: &mut ClientShellInput,
) -> bool {
    upstream::handle_prefix_binding(shell, binding, outcome)
}

#[cfg(test)]
pub(super) fn workspace_collapse_key(workspace_id: &str) -> String {
    upstream::workspace_collapse_key(workspace_id)
}

/// Parking signal: blocked outranks working outranks done, and anything
/// settled outranks idle/shell rows. The header count takes the worst
/// color so a parked workspace still reports its hottest pane.
fn status_severity(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Working => 3,
        AgentStatus::Done => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

struct Machine<'a> {
    id: &'a ClientEndpointId,
    snapshot: Option<&'a ClientShellSnapshot>,
    panes: Option<WorkspacePanes<'a>>,
    collapsed: Option<&'a HashSet<String>>,
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
    pub(super) fn local<'s: 'a>(
        snapshot: &'a ClientShellSnapshot,
        config: &'a ClientShellConfig,
        active_id: &'a ClientEndpointId,
        collapsed: &'s HashSet<String>,
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
            collapsed: Some(collapsed),
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
                collapsed: machine.collapsed,
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

    fn collapsed_set(&self, machine: usize) -> Option<&HashSet<String>> {
        self.machines
            .get(machine)
            .and_then(|machine| machine.collapsed)
    }

    fn parked(&self, machine: usize, workspace_id: &str) -> bool {
        upstream::workspace_collapsed(self.collapsed_set(machine), workspace_id)
    }

    /// Pane count and hottest status for the header count. Sections
    /// already hold one row per pane, so this needs no second walk over
    /// the snapshot.
    fn pane_summary(&self, machine: usize, workspace_id: &str) -> (usize, AgentStatus) {
        let mut count = 0;
        let mut worst = AgentStatus::Unknown;
        for section in self.sections(machine, workspace_id) {
            for row in &section.rows {
                count += 1;
                if status_severity(row.status) > status_severity(worst) {
                    worst = row.status;
                }
            }
        }
        (count, worst)
    }

    fn block_height(
        &self,
        machine: usize,
        workspace: &ClientShellWorkspace,
        entry: &WorkspaceEntry,
    ) -> u16 {
        let workspace_id = upstream::workspace(workspace).workspace_id;
        if self.parked(machine, workspace_id) {
            return 1;
        }
        let header = header_rows(workspace, upstream::entry(entry).indented).len();
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
        let sections = self.sections(block.machine, facts.workspace_id);
        // The count carries the hottest pane status: tertiary while the
        // workspace rests, signal-colored while anything needs attention.
        // Bold follows the same emphasis as the name.
        let (pane_count, worst) = self.pane_summary(block.machine, facts.workspace_id);
        let emphasized = (show_focus && facts.focused) || selected || dragged;
        let count = upstream::collapse_count_text(pane_count);
        let count_style = Style::default()
            .fg(upstream::status_color(worst, palette))
            .add_modifier(if emphasized {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let count = Some((count.as_str(), count_style));
        if self.parked(block.machine, facts.workspace_id) {
            // One row only: the name plus the count. Branch, tabs, and
            // panes all give up their rows; the count keeps the pane
            // total visible and stays clickable to unpark.
            render_header(
                buffer,
                rect,
                &entry,
                &rows[..rows.len().min(1)],
                show_focus && facts.focused,
                selected,
                dragged,
                palette,
                count,
            );
            return;
        }
        render_header(
            buffer,
            rect,
            &entry,
            &rows,
            show_focus && facts.focused,
            selected,
            dragged,
            palette,
            count,
        );
        let Some(endpoint_id) = self.machines.get(block.machine).map(|machine| machine.id) else {
            return;
        };
        let start_y = rect.y.saturating_add(rows.len() as u16).min(rect.bottom());
        let rows_area = Rect::new(rect.x, start_y, rect.width, rect.bottom() - start_y);
        // Summaries lift for the navigate cursor; workspace or tab focus
        // alone never promotes another pane's summary.
        let target = PaneRowsTarget {
            entry: &entry,
            endpoint_id,
            show_focus,
            summary_lift: selected,
        };
        render_pane_rows(buffer, rows_area, sections, target, self.config, hits);
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

/// Renamed header rows carry `[Branch(repo), Workspace(label)]` so the
/// repo renders tertiary and the label primary. Anything else renders
/// through the generic token spans.
fn is_slash_header(row: &[Token]) -> bool {
    matches!(
        row,
        [first, second]
            if matches!(first.kind, upstream::ResolvedTokenKind::Branch(_))
                && matches!(second.kind, upstream::ResolvedTokenKind::Workspace(_))
    )
}

/// First row of a renamed workspace: `repo / label` with both sides in
/// the header style (primary, bold on focus or cursor) and a quiet
/// tertiary divider.
fn render_slash_header(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: usize,
    row: &[Token],
    workspace_style: Style,
    secondary_style: Style,
) {
    let (repo, label) = match row {
        [first, second] => match (&first.kind, &second.kind) {
            (
                upstream::ResolvedTokenKind::Branch(repo),
                upstream::ResolvedTokenKind::Workspace(label),
            ) => (repo.as_str(), label.as_str()),
            _ => return,
        },
        _ => return,
    };
    let separator = " / ";
    let separator_width = slash_header_width(separator);
    let repo_width = slash_header_width(repo);
    let label_width = slash_header_width(label);
    if width < 1 + separator_width + 1 {
        let text = slash_header_truncate(label, width);
        Paragraph::new(Line::from(vec![Span::styled(text, workspace_style)]))
            .render(Rect::new(x, y, width as u16, 1), buffer);
        return;
    }
    let mut repo_budget = 1usize;
    let mut label_budget = 1usize;
    let mut remaining = width.saturating_sub(separator_width).saturating_sub(2);
    while remaining > 0 {
        let mut grew = false;
        if repo_budget < repo_width {
            repo_budget += 1;
            remaining -= 1;
            grew = true;
        }
        if remaining == 0 {
            break;
        }
        if label_budget < label_width {
            label_budget += 1;
            remaining -= 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let spans = vec![
        Span::styled(slash_header_truncate(repo, repo_budget), workspace_style),
        Span::styled(separator.to_owned(), secondary_style),
        Span::styled(slash_header_truncate(label, label_budget), workspace_style),
    ];
    Paragraph::new(Line::from(spans)).render(Rect::new(x, y, width as u16, 1), buffer);
}

fn slash_header_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(text)
}

fn slash_header_truncate(text: &str, max_width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_owned();
    }
    let mut output = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width.saturating_sub(1) {
            break;
        }
        output.push(ch);
        width += ch_width;
    }
    format!("{output}…")
}

/// Mirrors upstream's header text styling. The name sits one cell
/// in so it lines up with the upstream `spaces` title. Worktree children
/// indent by the same offset, with spaces.
fn render_header(
    buffer: &mut Buffer,
    area: Rect,
    entry: &Entry,
    rows: &[Vec<Token>],
    focused: bool,
    selected: bool,
    dragged: bool,
    palette: &Palette,
    count: Option<(&str, Style)>,
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
    // The count hugs the right edge of the first row, ending two cells
    // before the block edge; the name truncates around it. Same geometry
    // as `collapse_toggle_rect`, which hit-tests the count.
    let count_width = count.map_or(0, |(text, _)| slash_header_width(text));
    for (row_index, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_index as u16);
        if y >= area.bottom() {
            break;
        }
        let x = area.x.saturating_add(if entry.indented { 7 } else { 1 });
        let width = area.right().saturating_sub(2).saturating_sub(x);
        let width = if row_index == 0 && count_width > 0 {
            width.saturating_sub(count_width as u16 + 1)
        } else {
            width
        };
        if row_index == 0 && is_slash_header(row) {
            render_slash_header(
                buffer,
                x,
                y,
                usize::from(width),
                row,
                workspace_style,
                secondary_style,
            );
        } else {
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
        if row_index == 0 {
            if let Some((text, style)) = count {
                let full = area.right().saturating_sub(2).saturating_sub(x);
                let start = x.saturating_add(full.saturating_sub(count_width as u16));
                put_text(buffer, start, y, count_width as u16, text, style);
            }
        }
    }
}

/// Agent pane rows join tokens with ` / ` where upstream uses ` · `,
/// matching the workspace `repo / label` header. Shell rows carry no
/// separator, so the swap only ever touches agent rows.
fn pane_token_spans(
    tokens: &[Token],
    status_style: Style,
    workspace_style: Style,
    secondary_style: Style,
    custom_style: Style,
    palette: &Palette,
    max_width: usize,
) -> Vec<Span<'static>> {
    upstream::token_spans(
        tokens,
        status_style,
        workspace_style,
        secondary_style,
        custom_style,
        palette,
        max_width,
    )
    .into_iter()
    .map(|span| {
        if span.content == " · " {
            Span::styled(" / ".to_owned(), span.style)
        } else {
            span
        }
    })
    .collect()
}

/// Draws tab headers and pane rows top-down in `rect`, recording one click
/// target per visible pane row. Tab headers align with the workspace header
/// and pane rows indent two spaces under their tab, with summaries under
/// their pane. A lone tab skips its header, so its pane rows follow the
/// workspace header directly with the spine still hanging at the tab
/// column. A quiet spine hangs below each tab name across the full
/// height of its pane rows. The spine follows its tab: primary while the
/// tab holds focus, quiet otherwise.
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
    let now = now_ms();
    // Tabs align with the workspace header; panes indent two spaces under
    // their tab with a quiet spine hanging below the tab name across the
    // full height of the tab's pane rows. A lone tab skips its header so
    // the block reads workspace name, branch, then directly spine + panes.
    let hide_tabs = sections.len() == 1;
    let base_x = rect.x.saturating_add(if entry.indented { 7 } else { 1 });
    let tab_x = base_x;
    let pane_x = tab_x.saturating_add(2);
    let mut y = rect.y;
    for section in sections.iter() {
        if y >= rect.bottom() {
            return;
        }
        // The selected tab goes primary without bold; the cursor never
        // lifts tabs, and unfocused ones sit with the branch line. The
        // spine follows its tab: primary while the tab holds focus, quiet
        // otherwise, never bold either way.
        let spine_style = if section.tab_focused && show_focus {
            Style::default().fg(palette.text)
        } else {
            Style::default().fg(palette.overlay0)
        };
        if !hide_tabs {
            let tab_style = if section.tab_focused && show_focus {
                Style::default().fg(palette.text)
            } else {
                Style::default().fg(palette.overlay0)
            };
            put_text(
                buffer,
                tab_x,
                y,
                right.saturating_sub(tab_x),
                section.tab_label,
                tab_style,
            );
            y = y.saturating_add(1);
        }
        for pane_row in section.rows.iter() {
            if y >= rect.bottom() {
                return;
            }
            // The spine hangs below the tab name across every pane row,
            // so each tab reads as one block.
            put_segment(buffer, base_x, y, right, "│", spine_style);
            // Signal rows carry their glyph hue in the title: working peach,
            // done green, blocked red (bold when focused). Idle and unknown
            // stay as today: primary bold when focused, tertiary otherwise.
            // The cursor never changes row color.
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
            // Shell panes were never classified, so they carry a dim `$`
            // where an agent shows its status glyph. The label keeps the
            // row hue.
            let x = if pane_row.status == AgentStatus::Unknown {
                put_segment(
                    buffer,
                    pane_x,
                    y,
                    right,
                    "$",
                    Style::default().fg(palette.overlay0),
                )
            } else {
                put_segment(
                    buffer,
                    pane_x,
                    y,
                    right,
                    upstream::status_icon(pane_row.status, config),
                    Style::default().fg(upstream::status_color(pane_row.status, palette)),
                )
            };
            // Agents hang their status elapsed off the right edge of the
            // first row only. The label truncates first; the slot gives up
            // entirely when the row cannot spare it.
            let first_width = right.saturating_sub(x);
            let elapsed = pane_row
                .status_since_ms
                .map(|since| format_elapsed(since, now));
            let (label_end, elapsed) = match elapsed {
                Some(text) if first_width > text.len() as u16 + MIN_ELAPSED_LABEL_WIDTH => {
                    (right.saturating_sub(text.len() as u16 + 1), Some(text))
                }
                _ => (right, None),
            };
            if pane_row.tokens.is_empty() {
                // A shell row's label is its callsign, so the kind prefix
                // would only repeat what the `$` marker says. Agent
                // rows that resolve to nothing keep `kind / label`.
                let text = match (pane_row.label, pane_row.kind) {
                    (Some(label), "shell") => format!(" {label}"),
                    (Some(label), kind) => format!(" {kind} / {label}"),
                    (None, kind) => format!(" {kind}"),
                };
                put_text(buffer, x, y, label_end.saturating_sub(x), &text, text_style);
            } else {
                let width = label_end.saturating_sub(x);
                let mut spans = vec![Span::raw(" ")];
                spans.extend(pane_token_spans(
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
            if let Some(text) = elapsed {
                // Quiet tertiary like the branch line: the elapsed time
                // informs without outshining the row title.
                put_text(
                    buffer,
                    right.saturating_sub(text.len() as u16),
                    y,
                    text.len() as u16,
                    &text,
                    Style::default().fg(palette.overlay0),
                );
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
                // The spine runs through summary rows too, so the line
                // spans the tab's full height.
                put_segment(buffer, base_x, y, right, "│", spine_style);
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

#[cfg(test)]
mod tests {
    use super::format_elapsed;

    #[test]
    fn elapsed_formats_seconds_minutes_and_hours() {
        assert_eq!(format_elapsed(0, 0), "0s");
        assert_eq!(format_elapsed(0, 7_000), "7s");
        assert_eq!(format_elapsed(0, 59_999), "59s");
        assert_eq!(format_elapsed(0, 60_000), "1m");
        assert_eq!(format_elapsed(0, 3 * 60_000), "3m");
        assert_eq!(format_elapsed(0, 59 * 60_000 + 59_999), "59m");
        assert_eq!(format_elapsed(0, 60 * 60_000), "1h00m");
        assert_eq!(format_elapsed(0, 83 * 60_000), "1h23m");
        assert_eq!(format_elapsed(5_000, 3_000), "0s");
    }
}
