use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crate::client::shell::workspace_panes::{
    header_rows, PaneBranch, WorkspacePaneRow, WorkspacePaneSection, WorkspacePanes,
};
use crate::config::{AgentSidebarToken, AgentsSidebarConfig, SidebarLayout};
use crate::ui::ResolvedTokenKind;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

const SIDEBAR_WIDTH: usize = 25;

fn panes_config() -> Config {
    let mut config = Config::default();
    config.ui.sidebar.layout = SidebarLayout::Unified;
    config
}

fn panes_state(config: &Config, snapshot: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state
}

/// The sidebar columns of each frame row, right-trimmed.
fn sidebar_rows(frame: &FrameData) -> Vec<String> {
    frame_rows(frame)
        .iter()
        .map(|row| {
            row.chars()
                .take(SIDEBAR_WIDTH)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn workspace_pane_sections<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace_id: &str,
) -> Vec<WorkspacePaneSection<'a>> {
    WorkspacePanes::new(snapshot, &AgentsSidebarConfig::default()).into_sections(workspace_id)
}

fn workspace_pane_rows<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace_id: &str,
) -> Vec<WorkspacePaneRow<'a>> {
    workspace_pane_sections(snapshot, workspace_id)
        .into_iter()
        .flat_map(|section| section.rows)
        .collect()
}

fn workspace_pane_display_height(snapshot: &ClientShellSnapshot, workspace_id: &str) -> usize {
    WorkspacePanes::new(snapshot, &AgentsSidebarConfig::default()).display_height(workspace_id)
}

fn agent(pane_id: &str) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: Some("pi".into()),
        agent: Some("pi".into()),
        title: Some("herdr".into()),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn shell_pane(pane_id: &str, workspace_id: &str) -> ClientShellPane {
    shell_pane_in_tab(pane_id, workspace_id, "tab_1")
}

fn shell_pane_in_tab(pane_id: &str, workspace_id: &str, tab_id: &str) -> ClientShellPane {
    ClientShellPane {
        pane_id: pane_id.into(),
        workspace_id: workspace_id.into(),
        tab_id: tab_id.into(),
        label: None,
        cwd: None,
        foreground_cwd: None,
        focused: false,
        right_click_passthrough: false,
    }
}

fn tab(tab_id: &str, workspace_id: &str, number: usize, label: &str) -> ClientShellTab {
    ClientShellTab {
        tab_id: tab_id.into(),
        workspace_id: workspace_id.into(),
        number,
        label: label.into(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    }
}

fn workspace(workspace_id: &str, number: usize, label: &str) -> ClientShellWorkspace {
    ClientShellWorkspace {
        workspace_id: workspace_id.into(),
        active_tab_id: format!("tab_{workspace_id}"),
        new_workspace_cwd: format!("/repo/{workspace_id}"),
        number,
        label: label.into(),
        custom_label: false,
        branch: None,
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: None,
        focused: false,
        agent_status: AgentStatus::Idle,
    }
}

/// `client-shell` with two shell panes in tab `1`, then `api` with one pi
/// agent in tab `editor`.
fn two_workspace_snapshot() -> ClientShellSnapshot {
    let mut snapshot = snapshot();
    snapshot.panes.push(shell_pane("pane_2", "ws_1"));
    snapshot.workspaces.push(workspace("ws_2", 2, "api"));
    snapshot.tabs.push(tab("tab_2", "ws_2", 1, "editor"));
    snapshot
        .panes
        .push(shell_pane_in_tab("pane_3", "ws_2", "tab_2"));
    let mut pi = agent("pane_3");
    pi.workspace_id = "ws_2".into();
    pi.tab_id = "tab_2".into();
    snapshot.agents.push(pi);
    snapshot
}

fn header_name(workspace: &ClientShellWorkspace, indented: bool) -> String {
    header_rows(workspace, indented)[0]
        .iter()
        .find_map(|token| match &token.kind {
            ResolvedTokenKind::Workspace(name) => Some(name.clone()),
            _ => None,
        })
        .expect("workspace token")
}

fn left_click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn panes_layout_renders_headers_gap_tree_and_pane_text_at_fixed_geometry() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows,
        vec![
            " spaces",
            "",
            " repo",
            " main",
            " 1",
            " ├─  shell",
            " └─  shell",
            "",
            " ws_2",
            " editor",
            " └─● herdr · pi",
            "",
            "",
            "",
            "",
            " new               menu «",
        ],
    );
    assert_eq!(state.hits.workspace_body, Rect::new(0, 2, 25, 13));
    assert_eq!(
        state
            .hits
            .workspaces
            .iter()
            .map(|hit| (hit.workspace_id.as_str(), hit.rect))
            .collect::<Vec<_>>(),
        vec![
            ("ws_1", Rect::new(0, 2, 25, 5)),
            ("ws_2", Rect::new(0, 8, 25, 4)),
        ],
    );
    assert_eq!(
        state.hits.workspace_panes,
        vec![
            (
                Rect::new(0, 5, 25, 1),
                ClientEndpointId::Local,
                "pane_1".to_owned()
            ),
            (
                Rect::new(0, 6, 25, 1),
                ClientEndpointId::Local,
                "pane_2".to_owned()
            ),
            (
                Rect::new(0, 10, 25, 1),
                ClientEndpointId::Local,
                "pane_3".to_owned()
            ),
            (
                Rect::new(0, 11, 25, 1),
                ClientEndpointId::Local,
                "pane_3".to_owned()
            ),
        ],
    );
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert_eq!(state.hits.global_launcher, Rect::new(17, 15, 6, 1));
    assert_eq!(state.hits.sidebar_toggle, Rect::new(24, 15, 1, 1));

    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let bold = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::BOLD);
    assert!(bold(1, 2), "focused workspace header is bold");
    assert!(!bold(1, 8), "unfocused workspace header is not bold");
    assert!(bold(1, 5), "focused pane branch is bold");
    assert!(!bold(1, 6), "unfocused pane branch is not bold");
    assert_eq!(
        buffer[(3, 10)].fg,
        state.config.palette.yellow,
        "working icon"
    );
}

#[test]
fn classic_mode_keeps_the_agent_section_and_records_no_pane_rows() {
    let mut state = panes_state(&Config::default(), two_workspace_snapshot());
    let frame = state.compose(100, 16).expect("classic sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(rows[2], " ○ client-shell");
    assert_eq!(rows[3], "   main");
    assert!(
        rows.iter().any(|row| row.starts_with(" agents")),
        "{rows:?}"
    );
    assert!(state.hits.workspace_panes.is_empty());
    assert_ne!(state.hits.sidebar_section_divider, Rect::default());
}

#[test]
fn pane_row_click_focuses_the_pane_instead_of_pressing_the_workspace() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.compose(100, 16).expect("panes sidebar");
    let outcome = left_click(&mut state, 3, 6);
    assert!(state.workspace_press.is_none());
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("pane row click should focus the pane");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
    ));
}

#[test]
fn header_click_still_presses_the_workspace() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.compose(100, 16).expect("panes sidebar");
    left_click(&mut state, 3, 8);
    assert_eq!(
        state
            .workspace_press
            .as_ref()
            .map(|press| press.workspace_id.as_str()),
        Some("ws_2")
    );
}

#[test]
fn collapsed_unified_layout_lists_workspaces_without_the_agent_list() {
    let mut snapshot = two_workspace_snapshot();
    snapshot.agents[0].focused = true;
    let mut classic = panes_state(&Config::default(), snapshot.clone());
    classic.sidebar_collapsed = true;
    classic.compose(100, 16).expect("collapsed classic");
    assert!(!classic.hits.agents.is_empty(), "classic lists agents");

    let mut state = panes_state(&panes_config(), snapshot);
    state.sidebar_collapsed = true;
    let frame = state.compose(100, 16).expect("collapsed panes");
    assert!(state.hits.agents.is_empty());
    assert!(state.hits.workspace_panes.is_empty());
    assert_eq!(state.hits.workspaces.len(), 2);
    let rows = frame_rows(&frame);
    assert!(
        rows.iter().all(|row| !row.starts_with('─')),
        "no section divider: {rows:?}"
    );
}

fn remote_profile() -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    }
}

fn state_with_remote(status: ClientEndpointStatus) -> (ClientShellState, ClientEndpointId) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&panes_config()));
    let profile = remote_profile();
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, status);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut remote = snapshot();
    remote.boot_id = "remote-boot".into();
    remote.workspaces[0].label = "remote-workspace".into();
    remote.panes[0].pane_id = "remote_pane".into();
    state.set_endpoint_snapshot(&endpoint_id, Box::new(remote));
    (state, endpoint_id)
}

#[test]
fn machine_sidebar_lists_pane_rows_under_each_machine() {
    let (mut state, remote) = state_with_remote(ClientEndpointStatus::Online);
    let frame = state.compose(100, 20).expect("machine sidebar");
    let rows = sidebar_rows(&frame);
    let hits = state.hits.workspace_panes.clone();
    assert_eq!(
        hits.iter()
            .map(|(_, endpoint, pane)| (endpoint.clone(), pane.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (ClientEndpointId::Local, "pane_1"),
            (remote.clone(), "remote_pane"),
        ],
    );
    for (rect, _, _) in &hits {
        assert_eq!(rows[rect.y as usize], "   └─  shell", "{rows:?}");
        assert_eq!(
            rows[rect.y as usize - 1],
            "   1",
            "tab header above: {rows:?}"
        );
    }
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert_eq!(state.hits.sidebar_toggle.x, 24);
    assert!(state.hits.global_launcher.right() < state.hits.sidebar_toggle.x);

    let (rect, _, _) = hits[1];
    let outcome = left_click(&mut state, rect.x + 4, rect.y);
    assert!(state.workspace_press.is_none());
    assert!(
        outcome.actions.iter().any(|action| matches!(
            action,
            ClientShellAction::ActivateEndpoint { endpoint_id, .. } if *endpoint_id == remote
        )),
        "remote pane click activates its machine"
    );
}

#[test]
fn offline_machine_dims_its_pane_rows() {
    let (mut state, remote) = state_with_remote(ClientEndpointStatus::Reconnecting);
    let frame = state.compose(100, 20).expect("machine sidebar");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let (rect, _, _) = state
        .hits
        .workspace_panes
        .iter()
        .find(|(_, endpoint, _)| *endpoint == remote)
        .expect("remote pane row");
    assert!(buffer[(rect.x + 3, rect.y)]
        .modifier
        .contains(Modifier::DIM));
}

#[test]
fn shell_pane_without_agent_reads_as_unknown_shell() {
    let snapshot = snapshot();
    let rows = workspace_pane_rows(&snapshot, "ws_1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "shell");
    assert_eq!(rows[0].status, AgentStatus::Unknown);
    assert_eq!(rows[0].label, None);
}

#[test]
fn shell_pane_prefers_pane_label_and_ignores_blank_ones() {
    let mut snapshot = snapshot();
    snapshot.panes[0].label = Some("neon".into());
    assert_eq!(
        workspace_pane_rows(&snapshot, "ws_1")[0].label,
        Some("neon")
    );
    snapshot.panes[0].label = Some(String::new());
    assert_eq!(workspace_pane_rows(&snapshot, "ws_1")[0].label, None);
}

#[test]
fn agent_pane_uses_display_agent_and_title() {
    let mut snapshot = snapshot();
    snapshot.agents.push(agent("pane_1"));
    let rows = workspace_pane_rows(&snapshot, "ws_1");
    assert_eq!(rows[0].kind, "pi");
    assert_eq!(rows[0].status, AgentStatus::Working);
    assert_eq!(rows[0].label, Some("herdr"));
}

#[test]
fn kind_prefers_harness_over_slug_agent_name() {
    let mut snapshot = snapshot();
    let mut fallback = agent("pane_1");
    fallback.display_agent = None;
    fallback.name = Some("con-4665-plans-ordering-web".into());
    fallback.agent = Some("claude".into());
    fallback.title = None;
    snapshot.agents.push(fallback);
    snapshot.panes[0].label = Some("Fix 2".into());
    let rows = workspace_pane_rows(&snapshot, "ws_1");
    assert_eq!(rows[0].kind, "claude");
    assert_eq!(rows[0].label, Some("Fix 2"));

    let mut without_harness = super::snapshot();
    let mut named = agent("pane_1");
    named.display_agent = None;
    named.agent = None;
    named.name = Some("reviewer".into());
    named.title = None;
    without_harness.agents.push(named);
    let rows = workspace_pane_rows(&without_harness, "ws_1");
    assert_eq!(rows[0].kind, "reviewer");
}

#[test]
fn panes_branch_within_a_tab_and_group_by_tab_bar_order() {
    let mut snapshot = snapshot();
    snapshot.tabs.push(tab("tab_2", "ws_1", 2, "servers"));
    snapshot
        .panes
        .push(shell_pane_in_tab("pane_2", "ws_1", "tab_2"));
    snapshot.panes.push(shell_pane("pane_3", "ws_1"));
    snapshot.panes.push(shell_pane("pane_4", "ws_1"));
    snapshot.panes.push(shell_pane("pane_9", "ws_2"));
    let rows = workspace_pane_rows(&snapshot, "ws_1");
    assert_eq!(
        rows.iter()
            .map(|row| (row.pane_id, row.branch))
            .collect::<Vec<_>>(),
        vec![
            ("pane_1", PaneBranch::First),
            ("pane_3", PaneBranch::Middle),
            ("pane_4", PaneBranch::Last),
            ("pane_2", PaneBranch::Single),
        ]
    );
    assert_eq!(PaneBranch::Single.glyph(), "└─");
    assert_eq!(PaneBranch::Middle.glyph(), "├─");
    let sections = workspace_pane_sections(&snapshot, "ws_1");
    assert_eq!(
        sections
            .iter()
            .map(|section| (section.tab_label, section.tab_focused, section.rows.len()))
            .collect::<Vec<_>>(),
        vec![("1", true, 3), ("servers", false, 1)]
    );
    assert_eq!(workspace_pane_display_height(&snapshot, "ws_1"), 6);
}

#[test]
fn renamed_workspace_headers_keep_their_automatic_name_as_context() {
    let mut top = snapshot().workspaces.remove(0);
    top.label = "review".into();
    top.custom_label = true;
    top.worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "herdr".into(),
        is_linked_worktree: false,
    });
    assert_eq!(header_name(&top, false), "herdr · review");

    let mut child = top.clone();
    child.branch = Some("worktree/feat".into());
    assert_eq!(header_name(&child, true), "herdr · review");

    let mut bare = top.clone();
    bare.branch = None;
    bare.worktree = None;
    bare.new_workspace_cwd = "/repo/scratch".into();
    assert_eq!(header_name(&bare, false), "scratch · review");

    bare.custom_label = false;
    assert_eq!(header_name(&bare, false), "scratch");
}

#[test]
fn header_details_show_only_known_git_values_and_never_on_children() {
    let mut workspace = snapshot().workspaces.remove(0);
    workspace.git_ahead_behind = Some((2, 1));
    let rows = header_rows(&workspace, false);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1].iter().map(|token| &token.kind).collect::<Vec<_>>(),
        vec![
            &ResolvedTokenKind::Branch("main".into()),
            &ResolvedTokenKind::GitStatus {
                ahead: 2,
                behind: 1
            },
        ]
    );
    assert_eq!(header_rows(&workspace, true).len(), 1);
    workspace.branch = None;
    workspace.git_ahead_behind = Some((0, 0));
    assert_eq!(header_rows(&workspace, false).len(), 1);
    assert!(header_rows(&workspace, false)
        .iter()
        .flatten()
        .all(|token| token.kind != ResolvedTokenKind::StateIcon));
}

fn pane_row_text(config: &Config) -> String {
    let mut snapshot = snapshot();
    snapshot.agents.push(agent("pane_1"));
    let mut state = panes_state(config, snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let (rect, _, _) = state.hits.workspace_panes[0];
    sidebar_rows(&frame)[rect.y as usize].clone()
}

#[test]
fn default_agent_layout_shows_label_then_harness() {
    assert_eq!(pane_row_text(&panes_config()), " └─● herdr · pi");
}

#[test]
fn default_agent_layout_puts_reported_summary_on_its_own_row() {
    let mut snapshot = snapshot();
    let mut with_summary = agent("pane_1");
    with_summary.tokens = vec![("summary".into(), "auth".into())];
    snapshot.agents.push(with_summary);
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " └─● herdr · pi");
    assert_eq!(rows[rect.y as usize + 1], "     auth");
}

#[test]
fn harness_label_wins_over_slug_agent_name() {
    let mut snapshot = snapshot();
    let mut slugged = agent("pane_1");
    slugged.display_agent = None;
    slugged.name = Some("con-4665-plans-ordering-web".into());
    slugged.agent = Some("claude".into());
    snapshot.agents.push(slugged);
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " └─● herdr · claude");
}

#[test]
fn missing_summary_falls_back_to_terminal_title() {
    let mut snapshot = snapshot();
    let mut titled = agent("pane_1");
    titled.terminal_title_stripped = Some("CON-4665 explain ineligible plans".into());
    snapshot.agents.push(titled);
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " └─● herdr · pi");
    assert_eq!(rows[rect.y as usize + 1], "     CON-4665 explain …");
}

#[test]
fn reported_summary_wins_over_terminal_title() {
    let mut snapshot = snapshot();
    let mut both = agent("pane_1");
    both.tokens = vec![("summary".into(), "auth".into())];
    both.terminal_title_stripped = Some("CON-4665 explain ineligible plans".into());
    snapshot.agents.push(both);
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize + 1], "     auth");
}

#[test]
fn agent_row_without_summary_reserves_a_blank_second_row() {
    let mut snapshot = snapshot();
    snapshot.agents.push(agent("pane_1"));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " └─● herdr · pi");
    assert_eq!(rows[rect.y as usize + 1], "");
    assert_eq!(state.hits.workspace_panes.len(), 2);
    assert!(
        state
            .hits
            .workspace_panes
            .iter()
            .all(|(_, _, pane)| pane == "pane_1"),
        "both rows focus the pane"
    );
}

#[test]
fn detail_row_continues_the_branch_pipe_for_later_siblings() {
    let mut snapshot = snapshot();
    snapshot.panes.push(shell_pane("pane_2", "ws_1"));
    snapshot.agents.push(agent("pane_1"));
    snapshot.agents.push(agent("pane_2"));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " ├─● herdr · pi");
    assert_eq!(rows[rect.y as usize + 1], " │");
}

fn shell_row_text(label: Option<&str>) -> String {
    let mut snapshot = snapshot();
    snapshot.panes[0].label = label.map(str::to_owned);
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let (rect, _, _) = state.hits.workspace_panes[0];
    sidebar_rows(&frame)[rect.y as usize].clone()
}

#[test]
fn shell_pane_shows_callsign_then_shell_without_prefix() {
    assert_eq!(shell_row_text(Some("neon")), " └─  neon");
    assert_eq!(shell_row_text(None), " └─  shell");
}

#[test]
fn custom_agent_layout_flattens_and_drops_tokens_the_tree_shows() {
    let mut config = panes_config();
    config.ui.sidebar.agents.rows = vec![
        vec![
            AgentSidebarToken::StateIcon,
            AgentSidebarToken::Machine,
            AgentSidebarToken::Workspace,
            AgentSidebarToken::Tab,
            AgentSidebarToken::Agent,
        ],
        vec![AgentSidebarToken::StateText],
    ];
    assert_eq!(pane_row_text(&config), " └─● pi · working");
}

#[test]
fn custom_agent_layout_without_summary_reserves_no_second_row() {
    let mut config = panes_config();
    config.ui.sidebar.agents.rows = vec![vec![AgentSidebarToken::Agent, AgentSidebarToken::Pane]];
    let mut snapshot = snapshot();
    snapshot.panes.push(shell_pane("pane_2", "ws_1"));
    snapshot.agents.push(agent("pane_1"));
    let mut state = panes_state(&config, snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " ├─● pi · herdr");
    assert_eq!(state.hits.workspace_panes.len(), 2);
    assert_eq!(state.hits.workspace_panes[1].0.y, rect.y + 1);
}

#[test]
fn custom_agent_layout_that_resolves_empty_falls_back_to_agent_and_title() {
    let mut config = panes_config();
    config.ui.sidebar.agents.rows = vec![vec![AgentSidebarToken::Machine]];
    assert_eq!(pane_row_text(&config), " └─● pi · herdr");
}

#[test]
fn tall_workspace_clips_pane_rows_to_the_sidebar_body() {
    let mut snapshot = snapshot();
    for index in 2..=20 {
        snapshot
            .panes
            .push(shell_pane(&format!("pane_{index}"), "ws_1"));
    }
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 14).expect("short sidebar");
    let body = state.hits.workspace_body;
    let hits = &state.hits.workspace_panes;
    assert_eq!(hits.len(), usize::from(body.height) - 3);
    assert!(hits
        .iter()
        .all(|(rect, _, _)| rect.y >= body.y && rect.y < body.bottom()));
    assert_eq!(sidebar_rows(&frame)[2], " repo");
}

#[test]
fn pane_rows_under_worktree_children_continue_the_tree_line() {
    let mut snapshot = snapshot();
    snapshot.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    for (number, workspace_id) in [(2, "ws_2"), (3, "ws_3")] {
        let mut child = workspace(workspace_id, number, &format!("repo-{workspace_id}"));
        child.branch = Some(format!("worktree/{workspace_id}"));
        child.worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: true,
        });
        snapshot.workspaces.push(child);
        snapshot
            .panes
            .push(shell_pane(&format!("pane_{number}"), workspace_id));
    }
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 20).expect("grouped worktrees");
    let rows = sidebar_rows(&frame);
    let start = rows
        .iter()
        .position(|row| row.starts_with("   ├─ repo"))
        .expect("first child header");
    assert_eq!(
        rows[start..start + 6],
        [
            "   ├─ repo",
            "   │  1",
            "   │  └─  shell",
            "   └─ repo",
            "      1",
            "      └─  shell",
        ],
        "children hug the parent with no gap: {rows:?}"
    );
}

fn scale_snapshot(workspaces: usize, panes_per_workspace: usize) -> ClientShellSnapshot {
    let mut snapshot = snapshot();
    snapshot.workspaces.clear();
    snapshot.tabs.clear();
    snapshot.panes.clear();
    snapshot.agents.clear();
    for w in 0..workspaces {
        let workspace_id = format!("ws_{w}");
        let tab_id = format!("tab_{workspace_id}");
        let mut entry = workspace(&workspace_id, w + 1, &format!("repo-{w}"));
        entry.branch = Some("main".into());
        entry.git_ahead_behind = Some((1, 2));
        entry.focused = w == 0;
        snapshot.workspaces.push(entry);
        snapshot.tabs.push(tab(&tab_id, &workspace_id, 1, "1"));
        for p in 0..panes_per_workspace {
            let pane_id = format!("pane_{w}_{p}");
            snapshot
                .panes
                .push(shell_pane_in_tab(&pane_id, &workspace_id, &tab_id));
            if p % 2 == 0 {
                let mut pi = agent(&pane_id);
                pi.workspace_id = workspace_id.clone();
                pi.tab_id = tab_id.clone();
                snapshot.agents.push(pi);
            }
        }
    }
    snapshot
}

#[test]
#[ignore = "manual sidebar composition scaling profile"]
fn sidebar_layout_render_scale_profile() {
    for (workspaces, panes) in [(1, 1), (1, 15), (15, 1), (8, 15)] {
        for layout in [SidebarLayout::Classic, SidebarLayout::Unified] {
            let mut config = Config::default();
            config.ui.sidebar.layout = layout;
            let mut state = panes_state(&config, scale_snapshot(workspaces, panes));
            for _ in 0..20 {
                std::hint::black_box(state.compose(106, 40).expect("sidebar frame"));
            }
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(state.compose(106, 40).expect("sidebar frame"));
            }
            eprintln!(
                "sidebar: {layout:?} {workspaces}x{panes}, {:.1} us/frame",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
