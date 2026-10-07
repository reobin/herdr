//! The only place unified layout touches upstream internals. When an upstream
//! rename or refactor breaks unified layout, the fix belongs here.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use ratatui::{layout::Rect, style::Style, text::Span};

pub(super) use super::super::render::{put_segment, put_text, ShellRenderState};
pub(super) use super::super::{
    ClientEndpointId, ClientShellConfig, ClientShellEndpoint, ClientShellInput, ClientShellState,
    ShellHitMap, WorkspaceEntry,
};
pub(super) use crate::api::schema::AgentStatus;
pub(super) use crate::app::state::Palette;
use crate::config::AgentSidebarToken;
pub(super) use crate::config::AgentsSidebarConfig;
pub(super) use crate::protocol::{ClientShellSnapshot, ClientShellWorkspace};

pub(super) type Token = crate::ui::ResolvedToken;

pub(super) use crate::ui::ResolvedTokenKind;

pub(super) fn unified_layout(config: &ClientShellConfig) -> bool {
    config.sidebar_layout == crate::config::SidebarLayout::Unified
}

pub(super) fn palette(config: &ClientShellConfig) -> &Palette {
    &config.palette
}

pub(super) fn agents_config(config: &ClientShellConfig) -> &AgentsSidebarConfig {
    &config.agents
}

pub(super) fn mouse_capture(config: &ClientShellConfig) -> bool {
    config.mouse_capture
}

/// Unified status glyphs: every state has its own shape (working spins,
/// blocked is a diamond, done a check, idle a dotted ring, unknown a middle dot).
/// Classic layout keeps its own dots/symbols mapping untouched; this
/// override only drives unified pane rows through the call below.
///
/// The working glyph animates through braille frames driven by the render
/// clock. The client loop already wakes roughly every 100ms, so the
/// spinner advances without extra timer plumbing.
pub(super) fn status_icon(status: AgentStatus, _config: &ClientShellConfig) -> &'static str {
    match status {
        AgentStatus::Working => working_spinner_frame(std::time::Instant::now()),
        AgentStatus::Blocked => "◆",
        AgentStatus::Done => "✓",
        AgentStatus::Idle => "◌",
        AgentStatus::Unknown => "·",
    }
}

/// Braille spinner frames for the working glyph, sampled in order. Busy and
/// unmistakable at a glance; the tradeoff is vertical metrics: braille cells
/// render full-height next to the centered idle ring, so state switches can
/// visibly shift the glyph by a pixel or two.
const WORKING_SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// One frame per client tick keeps the cadence even: the loop wakes roughly
/// every 100ms, so each repaint advances exactly one frame for a 1.0s cycle.
/// An off-multiple interval (e.g. 120ms) aliases into stalls and skips.
const SPINNER_FRAME_MS: u128 = 100;

fn spinner_epoch() -> std::time::Instant {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
}

fn working_spinner_frame(now: std::time::Instant) -> &'static str {
    let elapsed = now.saturating_duration_since(spinner_epoch());
    WORKING_SPINNER_FRAMES
        [(elapsed.as_millis() / SPINNER_FRAME_MS) as usize % WORKING_SPINNER_FRAMES.len()]
}

/// Unified status colors: peach working, red blocked, green done, grey idle
/// and unknown. Classic layout keeps its own mapping untouched; this
/// override only drives unified pane rows through the calls below. Done
/// takes green by the usual success convention while idle drops to tertiary,
/// so the resting default never outshines active work.
pub(super) fn status_color(status: AgentStatus, palette: &Palette) -> ratatui::style::Color {
    match status {
        AgentStatus::Working => palette.peach,
        AgentStatus::Blocked => palette.red,
        AgentStatus::Done => palette.green,
        AgentStatus::Idle | AgentStatus::Unknown => palette.overlay0,
    }
}

/// Unified layout replaces the stock agent layout with a two-line pane row:
/// the session label and harness on the first line, the reported `$summary`
/// on the second. The detail line is reserved whenever the layout has a
/// `$summary`, so rows never shift when one arrives. A user layout keeps
/// working: its rows drive the first line, and any `$summary` occurrence
/// moves to the detail line with its configured style intact.
pub(super) fn pane_text_config(
    config: &AgentsSidebarConfig,
) -> std::borrow::Cow<'_, AgentsSidebarConfig> {
    use crate::config::AgentSidebarToken;
    if config.rows == AgentsSidebarConfig::default().rows {
        std::borrow::Cow::Owned(AgentsSidebarConfig {
            rows: vec![vec![
                AgentSidebarToken::Pane,
                AgentSidebarToken::Agent,
                AgentSidebarToken::Custom("summary".into()),
            ]],
            ..config.clone()
        })
    } else {
        std::borrow::Cow::Borrowed(config)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub(super) index: usize,
    pub(super) indented: bool,
    // Retained for upstream parity; the space-only layout no longer pipes
    // worktree siblings together.
    #[allow(dead_code)]
    pub(super) last_child: bool,
}

pub(super) fn entry(entry: &WorkspaceEntry) -> Entry {
    Entry {
        index: entry.index,
        indented: entry.indented,
        last_child: entry.last_child,
    }
}

#[derive(Clone, Copy)]
pub(super) struct PaneFacts<'a> {
    pub(super) pane_id: &'a str,
    pub(super) workspace_id: &'a str,
    pub(super) tab_id: &'a str,
    pub(super) label: Option<&'a str>,
    pub(super) focused: bool,
}

pub(super) fn panes(snapshot: &ClientShellSnapshot) -> impl Iterator<Item = PaneFacts<'_>> {
    snapshot.panes.iter().map(|pane| PaneFacts {
        pane_id: &pane.pane_id,
        workspace_id: &pane.workspace_id,
        tab_id: &pane.tab_id,
        label: pane.label.as_deref(),
        focused: pane.focused,
    })
}

#[derive(Clone, Copy)]
pub(super) struct TabFacts<'a> {
    pub(super) tab_id: &'a str,
    pub(super) label: &'a str,
    pub(super) focused: bool,
}

pub(super) fn tabs(snapshot: &ClientShellSnapshot) -> impl Iterator<Item = TabFacts<'_>> {
    snapshot.tabs.iter().map(|tab| TabFacts {
        tab_id: &tab.tab_id,
        label: &tab.label,
        focused: tab.focused,
    })
}

#[derive(Clone, Copy)]
pub(super) struct AgentFacts<'a> {
    pub(super) pane_id: &'a str,
    pub(super) status: AgentStatus,
    pub(super) kind: Option<&'a str>,
    pub(super) title: Option<&'a str>,
    agent: &'a crate::protocol::ClientShellAgent,
}

pub(super) fn agents(snapshot: &ClientShellSnapshot) -> impl Iterator<Item = AgentFacts<'_>> {
    snapshot.agents.iter().map(|agent| AgentFacts {
        pane_id: &agent.pane_id,
        status: agent.agent_status,
        kind: agent
            .display_agent
            .as_deref()
            .or(agent.agent.as_deref())
            .or(agent.name.as_deref())
            .or(agent.title.as_deref()),
        title: agent.title.as_deref(),
        agent,
    })
}

/// Splits `$summary` occurrences out of agent rows, unwrapping occurrence
/// styles. The first return holds the first-line rows, the second only the
/// summary rows.
fn partition_summary_rows(
    rows: &[Vec<AgentSidebarToken>],
) -> (Vec<Vec<AgentSidebarToken>>, Vec<Vec<AgentSidebarToken>>) {
    fn is_summary(token: &AgentSidebarToken) -> bool {
        match token {
            AgentSidebarToken::Custom(name) => name == "summary",
            AgentSidebarToken::Styled { token, .. } => is_summary(token),
            _ => false,
        }
    }
    let mut line = Vec::with_capacity(rows.len());
    let mut detail = Vec::with_capacity(rows.len());
    for row in rows {
        let (kept, summary): (Vec<_>, Vec<_>) =
            row.iter().cloned().partition(|token| !is_summary(token));
        if !kept.is_empty() {
            line.push(kept);
        }
        if !summary.is_empty() {
            detail.push(summary);
        }
    }
    (line, detail)
}

pub(super) struct AgentPaneText {
    pub(super) line: Vec<Token>,
    pub(super) detail: Option<Vec<Token>>,
}

/// Resolves the agent rows into the first line plus the summary line, which
/// is `None` when the layout has no `$summary`. The status icon, machine, workspace, and tab tokens drop out of the
/// first line because the status column and enclosing workspace
/// and tab headers already show them.
pub(super) fn agent_tokens(
    agent: &AgentFacts<'_>,
    pane: Option<&str>,
    config: &AgentsSidebarConfig,
) -> AgentPaneText {
    use crate::ui::ResolvedTokenKind;
    use std::collections::BTreeMap;
    let raw = agent.agent;
    let custom_tokens = raw.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = raw
        .state_labels
        .iter()
        .find(|(state, _)| state == super::super::status_text(agent.status))
        .map_or_else(
            || default_state_text(agent.status),
            |(_, label)| label.as_str(),
        );
    let canonical_agent = raw
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let (line_rows, detail_rows) = partition_summary_rows(config.rows_for_agent(canonical_agent));
    let resolve = |rows: Vec<Vec<AgentSidebarToken>>| {
        crate::ui::sidebar_agent_rows(
            &AgentsSidebarConfig {
                rows,
                rows_by_agent: BTreeMap::new(),
                row_gap: 0,
            },
            crate::ui::AgentTokenContext {
                machine: None,
                workspace: "",
                tab: None,
                pane,
                agent_label: agent.kind,
                terminal_title: raw.terminal_title.as_deref(),
                terminal_title_stripped: raw.terminal_title_stripped.as_deref(),
                canonical_agent: None,
                tokens: &custom_tokens,
            },
            state_text,
        )
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
    };
    AgentPaneText {
        line: resolve(line_rows)
            .into_iter()
            .filter(|token| {
                !matches!(
                    token.kind,
                    ResolvedTokenKind::StateIcon
                        | ResolvedTokenKind::Machine(_)
                        | ResolvedTokenKind::Workspace(_)
                        | ResolvedTokenKind::Tab(_)
                )
            })
            .collect(),
        detail: (!detail_rows.is_empty()).then(|| {
            let has_summary = custom_tokens
                .get("summary")
                .is_some_and(|summary| !summary.trim().is_empty());
            if has_summary {
                return resolve(detail_rows);
            }
            // No reporter fills `$summary` today (claude, opencode, pi),
            // so fall back to the terminal title the agent sets itself.
            if let Some(stripped) = raw
                .terminal_title_stripped
                .as_deref()
                .filter(|title| !title.trim().is_empty())
            {
                return vec![unstyled(ResolvedTokenKind::Custom(stripped.to_owned()))];
            }
            resolve(detail_rows)
        }),
    }
}

/// Mirrors the private `sidebar_status_text` in `agent_sidebar.rs`.
fn default_state_text(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}

pub(super) struct WorkspaceFacts<'a> {
    pub(super) workspace_id: &'a str,
    pub(super) label: &'a str,
    pub(super) custom_label: bool,
    pub(super) branch: Option<&'a str>,
    pub(super) ahead_behind: Option<(usize, usize)>,
    pub(super) repo: Option<&'a str>,
    pub(super) cwd: &'a str,
    pub(super) focused: bool,
}

pub(super) fn workspace(workspace: &ClientShellWorkspace) -> WorkspaceFacts<'_> {
    WorkspaceFacts {
        workspace_id: &workspace.workspace_id,
        label: &workspace.label,
        custom_label: workspace.custom_label,
        branch: workspace.branch.as_deref(),
        ahead_behind: workspace.git_ahead_behind,
        repo: workspace
            .worktree
            .as_ref()
            .map(|worktree| worktree.label.as_str()),
        cwd: &workspace.new_workspace_cwd,
        focused: workspace.focused,
    }
}

pub(super) fn workspace_at(
    snapshot: &ClientShellSnapshot,
    index: usize,
) -> Option<&ClientShellWorkspace> {
    snapshot.workspaces.get(index)
}

fn repo_name_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Base repo name for a workspace cwd, resolving through linked worktree
/// checkouts to the common repo (e.g. `ordering-web` for a
/// `con-4403-...` checkout). Local filesystem only; remote paths miss and
/// callers fall back to the directory name. Cached per cwd, since headers
/// render every frame and the mapping is stable for the process lifetime.
pub(super) fn repo_name_for_cwd(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    if let Some(cached) = repo_name_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cwd).cloned())
    {
        return cached;
    }
    let repo = crate::workspace::git_space_metadata(std::path::Path::new(cwd))
        .map(|space| space.repo_name);
    if let Ok(mut cache) = repo_name_cache().lock() {
        cache.insert(cwd.to_owned(), repo.clone());
    }
    repo
}

pub(super) fn workspace_token(name: String) -> Token {
    unstyled(crate::ui::ResolvedTokenKind::Workspace(name))
}

pub(super) fn branch_token(branch: &str) -> Token {
    unstyled(crate::ui::ResolvedTokenKind::Branch(branch.to_owned()))
}

pub(super) fn git_status_token(ahead: usize, behind: usize) -> Token {
    unstyled(crate::ui::ResolvedTokenKind::GitStatus { ahead, behind })
}

fn unstyled(kind: crate::ui::ResolvedTokenKind) -> Token {
    Token {
        kind,
        style: crate::config::SidebarTokenStyle::default(),
    }
}

/// Spans for tokens that never include a state icon.
pub(super) fn token_spans(
    tokens: &[Token],
    status_style: Style,
    workspace_style: Style,
    secondary_style: Style,
    custom_style: Style,
    palette: &Palette,
    max_width: usize,
) -> Vec<Span<'static>> {
    crate::ui::resolved_token_spans(
        tokens,
        ("", Style::default()),
        status_style,
        workspace_style,
        secondary_style,
        custom_style,
        palette,
        max_width,
    )
}

pub(super) fn clear_section_divider(hits: &mut ShellHitMap) {
    hits.sidebar_section_divider = Rect::default();
}

pub(super) fn push_pane_hit(
    hits: &mut ShellHitMap,
    rect: Rect,
    endpoint_id: &ClientEndpointId,
    pane_id: &str,
) {
    hits.workspace_panes
        .push((rect, endpoint_id.clone(), pane_id.to_owned()));
}

pub(super) fn pane_hit_at(
    shell: &ClientShellState,
    point: (u16, u16),
) -> Option<(ClientEndpointId, String)> {
    shell
        .hits
        .workspace_panes
        .iter()
        .find(|(rect, _, _)| super::super::contains(*rect, point))
        .map(|(_, endpoint_id, pane_id)| (endpoint_id.clone(), pane_id.clone()))
}

/// Same routing as upstream's agent rows: the aggregate sidebar goes through
/// endpoint activation, the single-machine sidebar focuses directly.
pub(super) fn focus_pane(
    shell: &mut ClientShellState,
    endpoint_id: ClientEndpointId,
    pane_id: String,
    outcome: &mut ClientShellInput,
) {
    if shell.multi_endpoint_active() {
        shell.focus_or_activate(
            endpoint_id,
            super::super::ClientEndpointFocusTarget::Pane(pane_id),
            outcome,
        );
    } else {
        shell.push_endpoint_method(
            crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget { pane_id }),
            outcome,
        );
    }
}

pub(super) fn endpoint_id(endpoint: &ClientShellEndpoint) -> &ClientEndpointId {
    &endpoint.endpoint_id
}

pub(super) struct MachineFacts<'a> {
    pub(super) id: &'a ClientEndpointId,
    pub(super) snapshot: Option<&'a ClientShellSnapshot>,
}

/// The machine list and its sidebar rows in `endpoint_sidebar::render_expanded`
/// order: `None` is a machine row, `Some((machine, entry))` a workspace row.
pub(super) struct MachineLayout<'a> {
    pub(super) machines: Vec<MachineFacts<'a>>,
    pub(super) rows: Vec<Option<(usize, WorkspaceEntry)>>,
}

pub(super) fn machine_layout<'a>(state: &ShellRenderState<'a>) -> MachineLayout<'a> {
    let empty_collapsed_groups = HashSet::new();
    let endpoints: &'a [ClientShellEndpoint] = state.endpoints;
    let mut machines = Vec::with_capacity(endpoints.len());
    let mut rows = Vec::new();
    for (index, endpoint) in endpoints.iter().enumerate() {
        rows.push(None);
        let collapsed = state.collapsed_endpoints.contains(&endpoint.endpoint_id);
        let snapshot = endpoint.snapshot.as_deref().filter(|_| !collapsed);
        if let Some(snapshot) = snapshot {
            let collapsed_groups = if endpoint.endpoint_id.is_local() {
                Some(state.collapsed_groups)
            } else {
                state.remote_collapsed_groups.get(&endpoint.endpoint_id)
            }
            .unwrap_or(&empty_collapsed_groups);
            rows.extend(
                super::super::sidebar::workspace_entries(snapshot, collapsed_groups)
                    .into_iter()
                    .map(|entry| Some((index, entry))),
            );
        }
        machines.push(MachineFacts {
            id: &endpoint.endpoint_id,
            snapshot,
        });
    }
    MachineLayout { machines, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ClientShellConfig {
        ClientShellConfig::from_config(&crate::config::Config::default())
    }

    #[test]
    fn settled_statuses_have_distinct_static_glyphs() {
        let config = config();
        assert_eq!(status_icon(AgentStatus::Blocked, &config), "\u{25c6}");
        assert_eq!(status_icon(AgentStatus::Done, &config), "\u{2713}");
        assert_eq!(status_icon(AgentStatus::Idle, &config), "\u{25cc}");
        assert_eq!(status_icon(AgentStatus::Unknown, &config), "\u{b7}");
    }

    #[test]
    fn working_glyph_spins_through_braille_frames() {
        let config = config();
        assert!(WORKING_SPINNER_FRAMES.contains(&status_icon(AgentStatus::Working, &config)));
        let start = spinner_epoch();
        assert_eq!(working_spinner_frame(start), WORKING_SPINNER_FRAMES[0]);
        assert_eq!(
            working_spinner_frame(
                start + std::time::Duration::from_millis(SPINNER_FRAME_MS as u64)
            ),
            WORKING_SPINNER_FRAMES[1]
        );
        let cycle = std::time::Duration::from_millis(
            (SPINNER_FRAME_MS as u64) * (WORKING_SPINNER_FRAMES.len() as u64),
        );
        assert_eq!(
            working_spinner_frame(start + cycle),
            WORKING_SPINNER_FRAMES[0]
        );
    }
}
