use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crate::client::shell::workspace_panes::{
    header_rows, unified_sidebar_needs_repaint, workspace_collapse_key, WorkspacePaneRow,
    WorkspacePaneSection, WorkspacePanes,
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

/// The sidebar columns of each frame row, right-trimmed. The working
/// glyph animates through braille frames, so every frame pins to the first
/// one before exact row comparisons; the frame sequence itself is covered
/// by the upstream glyph unit tests.
fn normalize_spinner(row: String) -> String {
    const FRAMES: [char; 10] = [
        '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280f}',
    ];
    row.chars()
        .map(|ch| if FRAMES.contains(&ch) { '\u{280b}' } else { ch })
        .collect()
}

fn sidebar_rows(frame: &FrameData) -> Vec<String> {
    frame_rows(frame)
        .iter()
        .map(|row| {
            normalize_spinner(
                row.chars()
                    .take(SIDEBAR_WIDTH)
                    .collect::<String>()
                    .trim_end()
                    .to_owned(),
            )
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
        status_since_ms: None,
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

fn header_title(workspace: &ClientShellWorkspace, indented: bool) -> String {
    header_rows(workspace, indented)[0]
        .iter()
        .filter_map(|token| match &token.kind {
            ResolvedTokenKind::Branch(name) | ResolvedTokenKind::Workspace(name) => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" / ")
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
fn panes_layout_renders_headers_gap_indent_and_pane_text_at_fixed_geometry() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows,
        vec![
            " spaces",
            "",
            " repo                ≡2",
            " main",
            " │ $ shell",
            " │ $ shell",
            "",
            " ws_2                ≡1",
            " │ ⠋ herdr / pi",
            " │",
            "",
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
            ("ws_1", Rect::new(0, 2, 25, 4)),
            ("ws_2", Rect::new(0, 7, 25, 3)),
        ],
    );
    assert_eq!(
        state.hits.workspace_panes,
        vec![
            (
                Rect::new(0, 4, 25, 1),
                ClientEndpointId::Local,
                "pane_1".to_owned()
            ),
            (
                Rect::new(0, 5, 25, 1),
                ClientEndpointId::Local,
                "pane_2".to_owned()
            ),
            (
                Rect::new(0, 8, 25, 1),
                ClientEndpointId::Local,
                "pane_3".to_owned()
            ),
            (
                Rect::new(0, 9, 25, 1),
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
    assert!(!bold(1, 7), "unfocused workspace header is not bold");
    assert_eq!(
        buffer[(3, 8)].fg,
        state.config.palette.peach,
        "working icon"
    );
}

#[test]
fn navigate_selection_marks_type_with_a_block_fill() {
    let mut snapshot = two_workspace_snapshot();
    snapshot.workspaces[1].branch = Some("con-4403-auth".into());
    snapshot.workspaces.push(workspace("ws_3", 3, "idle"));
    let mut state = panes_state(&panes_config(), snapshot);
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");
    let frame = state.compose(100, 24).expect("selected sidebar");
    let rows = sidebar_rows(&frame);
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row == needle)
            .unwrap_or_else(|| panic!("missing row {needle}: {rows:?}")) as u16
    };

    // Cursor fill: every selected-block cell keeps the selection background.
    let selected = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("ws_2 hit")
        .rect;
    for y in selected.y..selected.bottom() {
        for x in selected.x..selected.right() {
            assert_eq!(
                buffer[(x, y)].bg,
                palette.selection_bg,
                "cursor fill at ({x}, {y})"
            );
        }
    }

    // Names stay primary everywhere; weight marks focus and cursor.
    let name = row_of(" ws_2                ≡1");
    assert_eq!(buffer[(1, name)].fg, palette.text);
    assert!(
        buffer[(1, name)].modifier.contains(Modifier::BOLD),
        "selected name is bold"
    );
    let focused = row_of(" repo                ≡2");
    assert_eq!(buffer[(1, focused)].fg, palette.text);
    assert!(
        buffer[(1, focused)].modifier.contains(Modifier::BOLD),
        "focused name stays bold"
    );
    let idle = row_of(" ws_3                ≡0");
    assert_eq!(buffer[(1, idle)].fg, palette.text);
    assert!(
        !buffer[(1, idle)].modifier.contains(Modifier::BOLD),
        "idle name is not bold"
    );
    // Branch lines stay tertiary in every state.
    let branch = row_of(" con-4403-auth");
    assert_eq!(buffer[(1, branch)].fg, palette.overlay0);
    assert!(!buffer[(1, branch)].modifier.contains(Modifier::BOLD));
    let main = row_of(" main");
    assert_eq!(
        buffer[(1, main)].fg,
        palette.overlay0,
        "branch stays tertiary when focused"
    );

    // A lone tab skips its header, so pane rows follow the branch line
    // directly. Multi-tab workspaces keep one header row per tab.
    assert!(
        rows.iter().all(|row| row != " editor" && row != " 1"),
        "lone tabs hide their header: {rows:?}"
    );

    // Idle rows sit with the branch line while the focused pane goes
    // primary and bold. Signal rows carry their glyph hue in the title.
    // Columns come from the spaced layout: icon/blank at 5, text from 7.
    let focused_pane = row_of(" │ $ shell");
    assert_eq!(buffer[(5, focused_pane)].fg, palette.text);
    assert!(buffer[(5, focused_pane)].modifier.contains(Modifier::BOLD));
    let working_pane = row_of(" │ ⠋ herdr / pi");
    assert_eq!(buffer[(6, working_pane)].fg, palette.peach);
    assert!(!buffer[(6, working_pane)].modifier.contains(Modifier::BOLD));
    assert_eq!(buffer[(3, working_pane)].fg, palette.peach);
}

#[test]
fn agent_rows_follow_activity_detail_lifts_with_selection() {
    let mut snapshot = two_workspace_snapshot();
    snapshot.focused_workspace_id = Some("ws_2".into());
    snapshot.focused_tab_id = Some("tab_2".into());
    snapshot.focused_pane_id = Some("pane_3".into());
    snapshot.workspaces[0].focused = false;
    snapshot.workspaces[1].focused = true;
    snapshot.tabs[0].focused = false;
    snapshot.tabs[1].focused = true;
    for pane in &mut snapshot.panes {
        pane.focused = pane.pane_id == "pane_3";
    }
    snapshot.agents[0].focused = true;
    snapshot.agents[0].terminal_title_stripped = Some("reviewing auth".into());
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("focused agent");
    let rows = sidebar_rows(&frame);
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;

    let pane = rows
        .iter()
        .position(|row| row.contains("herdr / pi"))
        .expect("pane row") as u16;
    assert_eq!(
        buffer[(6, pane)].fg,
        palette.peach,
        "focused working pane keeps its signal hue"
    );
    assert!(buffer[(6, pane)].modifier.contains(Modifier::BOLD));
    // The status glyph carries the working hue next to the title.
    assert_eq!(
        buffer[(3, pane)].fg,
        palette.peach,
        "status glyph keeps the working title hue"
    );
    let detail = pane + 1;
    assert!(
        rows[detail as usize].contains("reviewing auth"),
        "detail row: {rows:?}"
    );
    // The focused workspace lifts the summary even without the cursor.
    assert_eq!(buffer[(6, detail)].fg, palette.subtext0);
    assert!(!buffer[(6, detail)].modifier.contains(Modifier::BOLD));

    // Background workspaces stay tertiary: focus back on ws_1 while the
    // cursor sits there too keeps ws_2 quiet.
    let subtext0 = palette.subtext0;
    let overlay0 = palette.overlay0;
    let snapshot = state.snapshot.as_mut().expect("snapshot");
    snapshot.focused_workspace_id = Some("ws_1".into());
    snapshot.focused_tab_id = Some("tab_1".into());
    snapshot.focused_pane_id = Some("pane_1".into());
    snapshot.workspaces[0].focused = true;
    snapshot.workspaces[1].focused = false;
    snapshot.tabs[0].focused = true;
    snapshot.tabs[1].focused = false;
    for pane in &mut snapshot.panes {
        pane.focused = pane.pane_id == "pane_1";
    }
    snapshot.agents[0].focused = false;
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    let frame = state.compose(100, 16).expect("background agent");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    assert_eq!(buffer[(6, detail)].fg, overlay0);

    // The cursor alone lifts it back without focus.
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");
    let frame = state.compose(100, 16).expect("selected agent");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    assert_eq!(buffer[(6, detail)].fg, subtext0);
    assert!(!buffer[(6, detail)].modifier.contains(Modifier::BOLD));
}

#[test]
fn tab_focus_alone_keeps_sibling_summary_tertiary() {
    // The workspace and tab hold focus, but the agent pane itself does
    // not: its summary must stay quiet.
    let mut snapshot = two_workspace_snapshot();
    snapshot.focused_workspace_id = Some("ws_2".into());
    snapshot.focused_tab_id = Some("tab_2".into());
    snapshot.focused_pane_id = Some("pane_4".into());
    snapshot.workspaces[0].focused = false;
    snapshot.workspaces[1].focused = true;
    snapshot.tabs[0].focused = false;
    snapshot.tabs[1].focused = true;
    let mut shell = shell_pane_in_tab("pane_4", "ws_2", "tab_2");
    shell.focused = true;
    snapshot.panes.push(shell);
    for pane in &mut snapshot.panes {
        if pane.pane_id != "pane_4" {
            pane.focused = false;
        }
    }
    snapshot.agents[0].focused = false;
    snapshot.agents[0].terminal_title_stripped = Some("reviewing auth".into());
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("tab-focused agent");
    let rows = sidebar_rows(&frame);
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    let detail = rows
        .iter()
        .position(|row| row.contains("reviewing auth"))
        .expect("detail row") as u16;
    assert_eq!(buffer[(6, detail)].fg, palette.overlay0);
}

#[test]
fn idle_agent_rows_sink_to_tertiary() {
    let mut snapshot = two_workspace_snapshot();
    snapshot.agents[0].agent_status = AgentStatus::Idle;
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("idle agent");
    let rows = sidebar_rows(&frame);
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    let pane = rows
        .iter()
        .position(|row| row.contains("herdr / pi"))
        .expect("pane row") as u16;
    assert_eq!(buffer[(6, pane)].fg, palette.overlay0);
    assert_eq!(
        buffer[(3, pane)].fg,
        palette.overlay0,
        "idle glyph stays tertiary with its title"
    );
}

#[test]
fn signal_statuses_paint_title_and_glyph_with_the_signal_hue() {
    for (status, expected) in [
        (AgentStatus::Working, "peach"),
        (AgentStatus::Blocked, "red"),
        (AgentStatus::Idle, "overlay0"),
    ] {
        let mut snapshot = two_workspace_snapshot();
        snapshot.agents[0].agent_status = status;
        let mut state = panes_state(&panes_config(), snapshot);
        let frame = state.compose(100, 16).expect("signal agent");
        let rows = sidebar_rows(&frame);
        let buffer = frame.to_ratatui_buffer().expect("buffer");
        let palette = &state.config.palette;
        let expected = match expected {
            "peach" => palette.peach,
            "red" => palette.red,
            _ => palette.overlay0,
        };
        let pane = rows
            .iter()
            .position(|row| row.contains("herdr / pi"))
            .expect("pane row") as u16;
        assert_eq!(buffer[(6, pane)].fg, expected, "{status:?} title");
        assert_eq!(buffer[(3, pane)].fg, expected, "{status:?} glyph");
        assert!(
            !buffer[(6, pane)].modifier.contains(Modifier::BOLD),
            "{status:?} unfocused title is not bold"
        );
    }

    // Done is client-projected from idle-after-work, so drive working
    // first on one state, then the idle completion with a bumped seq.
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.compose(100, 16).expect("working agent");
    let mut done = two_workspace_snapshot();
    done.agents[0].agent_status = AgentStatus::Idle;
    done.agents[0].state_change_seq = 2;
    state.set_snapshot(Box::new(done));
    let frame = state.compose(100, 16).expect("done agent");
    let rows = sidebar_rows(&frame);
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    let pane = rows
        .iter()
        .position(|row| row.contains("herdr / pi"))
        .expect("pane row") as u16;
    assert_eq!(buffer[(6, pane)].fg, palette.green, "done title");
    assert_eq!(buffer[(3, pane)].fg, palette.green, "done glyph");
}

#[test]
fn idle_timer_tick_requests_repaint_while_a_spinner_is_visible() {
    // Mirrors the ClientLoopEvent::Timer arm in src/client/mod.rs: a compose
    // only happens when one of the ticks requests a repaint. The working
    // spinner advances on those repaints, so an idle tick with a visible
    // spinner must ask for one, or the animation freezes between unrelated
    // activity.
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.compose(100, 16).expect("initial sidebar");
    let now = std::time::Instant::now();
    let mut outcome = state.tick_selection_autoscroll(now);
    let (_, notification_repaint) = state.tick_notifications(now);
    outcome.repaint |= notification_repaint
        | state.tick_copy_feedback(now)
        | state.tick_workspace_highlight(now)
        | state.tick_endpoint_error(now)
        | unified_sidebar_needs_repaint(&state);
    assert!(
        outcome.repaint,
        "idle tick must repaint while a spinner is visible"
    );
}

#[test]
fn idle_timer_tick_requests_repaint_while_status_elapsed_ticks() {
    // Fresh stamps tick every second; settled stamps only need their
    // minute flip. A mid-minute settled row correctly stays quiet.
    let mut fresh = snapshot();
    let mut pi = elapsed_agent("pane_1", 30_000);
    pi.agent_status = AgentStatus::Idle;
    fresh.agents.push(pi);
    assert!(
        unified_sidebar_needs_repaint(&panes_state(&panes_config(), fresh)),
        "fresh elapsed must repaint"
    );

    let mut flip = snapshot();
    let mut pi = elapsed_agent("pane_1", 12 * 60_000);
    pi.agent_status = AgentStatus::Idle;
    flip.agents.push(pi);
    assert!(
        unified_sidebar_needs_repaint(&panes_state(&panes_config(), flip)),
        "minute flip must repaint"
    );
}

#[test]
fn idle_timer_tick_stays_quiet_between_minute_flips() {
    let mut snapshot = snapshot();
    let mut pi = elapsed_agent("pane_1", 12 * 60_000 + 30_000);
    pi.agent_status = AgentStatus::Idle;
    snapshot.agents.push(pi);
    assert!(
        !unified_sidebar_needs_repaint(&panes_state(&panes_config(), snapshot)),
        "mid-minute settled rows need no repaint"
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
    let outcome = left_click(&mut state, 3, 5);
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
    left_click(&mut state, 3, 7);
    assert_eq!(
        state
            .workspace_press
            .as_ref()
            .map(|press| press.workspace_id.as_str()),
        Some("ws_2")
    );
}

#[test]
fn parked_workspace_collapses_to_a_single_count_row() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.toggle_collapsed_group(&ClientEndpointId::Local, workspace_collapse_key("ws_2"));
    let frame = state.compose(100, 16).expect("parked sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows,
        vec![
            " spaces",
            "",
            " repo                ≡2",
            " main",
            " │ $ shell",
            " │ $ shell",
            "",
            " ws_2                ≡1",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            " new               menu «",
        ],
    );
    assert_eq!(
        state
            .hits
            .workspaces
            .iter()
            .map(|hit| (hit.workspace_id.as_str(), hit.rect))
            .collect::<Vec<_>>(),
        vec![
            ("ws_1", Rect::new(0, 2, 25, 4)),
            ("ws_2", Rect::new(0, 7, 25, 1)),
        ],
    );
    assert!(
        state
            .hits
            .workspace_panes
            .iter()
            .all(|(_, _, pane)| pane != "pane_3"),
        "parked panes keep no click targets"
    );
}

#[test]
fn count_toggle_click_parks_and_unparks_without_focusing() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.compose(100, 16).expect("panes sidebar");
    // The ws_1 `≡2` ends two cells before the 25-wide block edge.
    let outcome = left_click(&mut state, 22, 2);
    assert!(state.workspace_press.is_none(), "toggle never focuses");
    assert!(outcome.repaint);
    assert!(
        state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "toggle persists through the stock collapsed groups"
    );
    let frame = state.compose(100, 16).expect("parked sidebar");
    assert_eq!(sidebar_rows(&frame)[2], " repo                ≡2");
    assert_eq!(state.hits.workspaces[0].rect, Rect::new(0, 2, 25, 1));

    let outcome = left_click(&mut state, 22, 2);
    assert!(state.workspace_press.is_none());
    assert!(outcome.repaint);
    assert!(!state
        .collapsed_groups
        .contains(&workspace_collapse_key("ws_1")));
    let frame = state.compose(100, 16).expect("unparked sidebar");
    assert_eq!(sidebar_rows(&frame)[2], " repo                ≡2");
    assert_eq!(state.hits.workspaces[0].rect, Rect::new(0, 2, 25, 4));
}

/// Snapshot with the server-published manifest entry for the sentinel,
/// mirroring production: the server assigns local `[[keys.command]]`
/// entries opaque command IDs, so the live binding carries the ID plus
/// the original key labels, never the sentinel text.
fn sentinel_snapshot(key_label: &str) -> ClientShellSnapshot {
    let mut snapshot = two_workspace_snapshot();
    snapshot.commands.push(crate::protocol::ClientShellCommand {
        command_id: "cmd_test_park".into(),
        binding_label: key_label.into(),
        binding_labels: vec![key_label.into()],
        action: crate::protocol::ClientShellCommandAction::Shell,
        description: None,
    });
    snapshot
}

#[test]
fn sentinel_binding_parks_on_configured_key_only() {
    use crossterm::event::KeyCode;
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = "prefix+u"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+u"));
    assert_eq!(state.config.keybinds.keybinds.custom_commands.len(), 1);
    let prefix = || crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let m = || crate::input::TerminalKey::new(KeyCode::Char('m'), KeyModifiers::empty());
    let u = || crate::input::TerminalKey::new(KeyCode::Char('u'), KeyModifiers::empty());

    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(m())]);
    assert!(
        !state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "unbound prefix+m is a no-op with no sentinel on it"
    );
    assert!(outcome.actions.is_empty());
    assert!(state.endpoint_error.is_none());

    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(u())]);
    assert!(
        state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "sentinel prefix+u parks the focused workspace"
    );
    assert!(
        outcome.actions.is_empty(),
        "sentinel never dispatches a command"
    );
    assert!(state.endpoint_error.is_none());
    let frame = state.compose(100, 16).expect("parked sidebar");
    assert_eq!(
        sidebar_rows(&frame)[2],
        " repo                ≡2",
        "sentinel-parked workspace renders as one count row"
    );
    assert_eq!(state.hits.workspaces[0].rect, Rect::new(0, 2, 25, 1));

    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let _ = state.handle_raw_events(vec![RawInputEvent::Key(u())]);
    assert!(
        !state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "sentinel prefix+u again unparks"
    );
}

#[test]
fn sentinel_on_prefix_m_parks_without_endpoint_error() {
    use crossterm::event::KeyCode;
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = "prefix+m"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+m"));
    let prefix = || crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let m = || crate::input::TerminalKey::new(KeyCode::Char('m'), KeyModifiers::empty());

    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(m())]);
    assert!(
        state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "sentinel prefix+m parks the focused workspace"
    );
    assert!(
        outcome.actions.is_empty(),
        "sentinel never dispatches a command"
    );
    assert!(state.endpoint_error.is_none());
}

#[test]
fn non_sentinel_prefix_binding_still_reaches_record_binding() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    let mut outcome = ClientShellInput::default();
    let binding = crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextTab);
    assert!(!super::super::workspace_panes::handle_prefix_binding(
        &mut state,
        &binding,
        &mut outcome
    ));
}

#[test]
fn sentinel_stays_inert_in_classic_layout() {
    use crossterm::event::KeyCode;
    let config: Config = toml::from_str(
        r#"
[[keys.command]]
key = "prefix+u"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+u"));
    let prefix = || crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let u = || crate::input::TerminalKey::new(KeyCode::Char('u'), KeyModifiers::empty());
    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(u())]);
    assert!(
        !state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "classic layout ignores the sentinel"
    );
    assert!(outcome.actions.is_empty());
    assert!(state.endpoint_error.is_none());
}

#[test]
fn sentinel_stays_inert_in_collapsed_rail() {
    use crossterm::event::KeyCode;
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = "prefix+u"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+u"));
    state.sidebar_collapsed = true;
    let prefix = || crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let u = || crate::input::TerminalKey::new(KeyCode::Char('u'), KeyModifiers::empty());
    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(u())]);
    assert!(
        !state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "collapsed rail swallows the sentinel without toggling"
    );
    assert!(outcome.actions.is_empty());
    assert!(state.endpoint_error.is_none());
}

#[test]
fn sentinel_without_focused_workspace_dispatches_nothing() {
    use crossterm::event::KeyCode;
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = "prefix+u"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut snapshot = sentinel_snapshot("prefix+u");
    snapshot.focused_workspace_id = None;
    let mut state = panes_state(&config, snapshot);
    let prefix = || crate::input::TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let u = || crate::input::TerminalKey::new(KeyCode::Char('u'), KeyModifiers::empty());
    let _ = state.handle_raw_events(vec![RawInputEvent::Key(prefix())]);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(u())]);
    assert!(
        !state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "no focused workspace means nothing to park"
    );
    assert!(outcome.actions.is_empty());
    assert!(state.endpoint_error.is_none());
}

#[test]
fn sentinel_only_fires_on_the_local_keymap() {
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = "prefix+u"
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+u"));
    let binding = crate::input::KeybindMatch::Command(
        state.config.keybinds.keybinds.custom_commands[0].clone(),
    );
    let mut outcome = ClientShellInput::default();
    assert!(super::super::workspace_panes::handle_prefix_binding(
        &mut state,
        &binding,
        &mut outcome
    ));
    assert!(
        state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_1")),
        "local keymap toggles"
    );
    state.config.keybinding_source = ClientShellKeybindingSource::Endpoint;
    let mut outcome = ClientShellInput::default();
    assert!(
        !super::super::workspace_panes::handle_prefix_binding(&mut state, &binding, &mut outcome),
        "endpoint keymap never consumes, so colliding endpoint commands dispatch"
    );
}

#[test]
fn lone_command_reusing_one_sentinel_key_is_never_swallowed() {
    let config: Config = toml::from_str(
        r#"
[ui.sidebar]
layout = "unified"

[[keys.command]]
key = ["prefix+u", "prefix+m"]
command = "unified:toggle-workspace-park"
"#,
    )
    .expect("sentinel config");
    let mut state = panes_state(&config, sentinel_snapshot("prefix+u"));
    let lone = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("m"),
        label: "prefix+m".into(),
        command: "cmd_real".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };
    let mut outcome = ClientShellInput::default();
    assert!(
        !super::super::workspace_panes::handle_prefix_binding(
            &mut state,
            &crate::input::KeybindMatch::Command(lone),
            &mut outcome
        ),
        "[prefix+m] must not match the [prefix+u, prefix+m] sentinel set"
    );
    assert!(outcome.actions.is_empty());
}

#[test]
fn parked_header_click_still_focuses_without_unparking() {
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    state.toggle_collapsed_group(&ClientEndpointId::Local, workspace_collapse_key("ws_2"));
    state.compose(100, 16).expect("parked sidebar");
    left_click(&mut state, 3, 7);
    assert_eq!(
        state
            .workspace_press
            .as_ref()
            .map(|press| press.workspace_id.as_str()),
        Some("ws_2")
    );
    assert!(
        state
            .collapsed_groups
            .contains(&workspace_collapse_key("ws_2")),
        "name clicks focus; only the count toggles"
    );
}

#[test]
fn parked_count_takes_the_hottest_pane_status_color() {
    let mut snapshot = two_workspace_snapshot();
    snapshot.agents[0].agent_status = AgentStatus::Blocked;
    let mut state = panes_state(&panes_config(), snapshot);
    state.toggle_collapsed_group(&ClientEndpointId::Local, workspace_collapse_key("ws_2"));
    let frame = state.compose(100, 16).expect("parked sidebar");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    assert_eq!(buffer[(21, 7)].fg, palette.red);
    assert!(!buffer[(21, 7)].modifier.contains(Modifier::BOLD));
}

#[test]
fn parked_remote_workspace_collapses_to_one_row() {
    let (mut state, remote) = state_with_remote(ClientEndpointStatus::Online);
    state.toggle_collapsed_group(&remote, workspace_collapse_key("ws_1"));
    let frame = state.compose(100, 20).expect("machine sidebar");
    let rows = sidebar_rows(&frame);
    // Both machines resolve their header to the same repo name, so find
    // the remote block through its hit rect instead of its text.
    let remote_hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1" && hit.endpoint_id == remote)
        .expect("remote workspace hit");
    assert!(rows[remote_hit.rect.y as usize].ends_with("≡1"));
    assert_eq!(
        state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == "ws_1" && hit.endpoint_id == remote)
            .map(|hit| hit.rect.height),
        Some(1)
    );
    assert!(
        state
            .hits
            .workspace_panes
            .iter()
            .all(|(_, endpoint, _)| *endpoint != remote),
        "parked machine keeps no pane targets"
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
        assert_eq!(rows[rect.y as usize], "   │ $ shell", "{rows:?}");
        assert_eq!(
            rows[rect.y as usize - 1],
            "   main",
            "lone tab hides its header, so the branch sits above: {rows:?}"
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
        rows.iter().map(|row| row.pane_id).collect::<Vec<_>>(),
        vec!["pane_1", "pane_3", "pane_4", "pane_2",]
    );
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
fn lone_tab_skips_its_header_and_keeps_the_spine() {
    // One tab: name and branch, then directly spine + panes.
    let mut state = panes_state(&panes_config(), two_workspace_snapshot());
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows[2..6],
        [
            " repo                ≡2",
            " main",
            " │ $ shell",
            " │ $ shell",
        ],
        "{rows:?}"
    );
    assert_eq!(
        workspace_pane_display_height(&two_workspace_snapshot(), "ws_1"),
        2
    );
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
    assert_eq!(header_name(&top, false), "review");
    assert_eq!(header_title(&top, false), "herdr / review");
    assert_eq!(
        header_rows(&top, false)[0]
            .iter()
            .map(|token| &token.kind)
            .collect::<Vec<_>>(),
        vec![
            &ResolvedTokenKind::Branch("herdr".into()),
            &ResolvedTokenKind::Workspace("review".into()),
        ]
    );

    let mut child = top.clone();
    child.branch = Some("worktree/feat".into());
    assert_eq!(header_name(&child, true), "review");
    assert_eq!(header_title(&child, true), "herdr / review");

    let mut bare = top.clone();
    bare.branch = None;
    bare.worktree = None;
    bare.new_workspace_cwd = "/repo/scratch".into();
    assert_eq!(header_name(&bare, false), "review");
    assert_eq!(header_title(&bare, false), "scratch / review");

    bare.custom_label = false;
    assert_eq!(header_name(&bare, false), "scratch");
    assert_eq!(header_title(&bare, false), "scratch");
}

#[test]
fn renamed_header_renders_slash_with_both_sides_primary() {
    let mut snapshot = snapshot();
    snapshot.workspaces[0].label = "manifest-order".into();
    snapshot.workspaces[0].custom_label = true;
    snapshot.workspaces[0].focused = true;
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("renamed sidebar");
    let rows = sidebar_rows(&frame);
    let header = rows
        .iter()
        .position(|row| row.contains("manifest-or"))
        .expect("renamed header row") as u16;
    assert_eq!(rows[header as usize], " repo / manifest-or… ≡1");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    assert_eq!(buffer[(2, header)].fg, palette.text);
    assert!(
        buffer[(2, header)].modifier.contains(Modifier::BOLD),
        "focused repo is bold"
    );
    assert_eq!(buffer[(6, header)].fg, palette.overlay0, "slash divider");
    assert!(
        !buffer[(6, header)].modifier.contains(Modifier::BOLD),
        "divider stays quiet"
    );
    // Label starts after ` repo / ` (1 + 4 + 3).
    assert_eq!(buffer[(8, header)].fg, palette.text);
    assert!(
        buffer[(8, header)].modifier.contains(Modifier::BOLD),
        "focused label is bold"
    );
}

#[test]
fn header_resolves_base_repo_through_linked_checkout_without_worktree_field() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = format!(
        "herdr-header-repo-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let tmp = std::env::temp_dir().join(unique);
    let main_repo = tmp.join("ordering-web");
    let main_git = main_repo.join(".git");
    let checkout_name = "con-4403-search-product-details";
    let worktree_gitdir = main_git.join("worktrees").join(checkout_name);
    let checkout = tmp.join("ordering-web-wt").join("feat").join(checkout_name);
    std::fs::create_dir_all(&worktree_gitdir).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(main_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(worktree_gitdir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();
    std::fs::write(
        worktree_gitdir.join("commondir"),
        main_git.display().to_string(),
    )
    .unwrap();
    std::fs::write(
        checkout.join(".git"),
        format!("gitdir: {}\n", worktree_gitdir.display()),
    )
    .unwrap();

    let mut ws = workspace("ws_repo", 1, checkout_name);
    ws.new_workspace_cwd = checkout.display().to_string();
    ws.label = checkout_name.into();
    ws.custom_label = false;
    ws.worktree = None;
    assert_eq!(header_name(&ws, false), "ordering-web");

    ws.custom_label = true;
    ws.label = "plan-dependencies".into();
    assert_eq!(header_name(&ws, false), "plan-dependencies");
    assert_eq!(header_title(&ws, false), "ordering-web / plan-dependencies");

    std::fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn header_details_show_only_known_git_values_for_children_too() {
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
    assert_eq!(
        header_rows(&workspace, true)
            .iter()
            .map(|row| row.iter().map(|token| &token.kind).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        rows.iter()
            .map(|row| row.iter().map(|token| &token.kind).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    workspace.branch = None;
    workspace.git_ahead_behind = Some((0, 0));
    assert_eq!(header_rows(&workspace, false).len(), 1);
    assert_eq!(header_rows(&workspace, true).len(), 1);
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
    assert_eq!(pane_row_text(&panes_config()), " │ ⠋ herdr / pi");
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
    assert_eq!(rows[rect.y as usize], " │ ⠋ herdr / pi");
    assert_eq!(rows[rect.y as usize + 1], " │   auth");
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
    assert_eq!(rows[rect.y as usize], " │ ⠋ herdr / claude");
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
    assert_eq!(rows[rect.y as usize], " │ ⠋ herdr / pi");
    assert_eq!(rows[rect.y as usize + 1], " │   CON-4665 explain …");
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
    assert_eq!(rows[rect.y as usize + 1], " │   auth");
}

#[test]
fn agent_row_without_summary_reserves_a_blank_second_row() {
    let mut snapshot = snapshot();
    snapshot.agents.push(agent("pane_1"));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    assert_eq!(rows[rect.y as usize], " │ ⠋ herdr / pi");
    assert_eq!(rows[rect.y as usize + 1], " │");
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
fn tab_spine_spans_the_full_height_of_its_pane_rows() {
    // Two tabs: each tab's pane rows (and their reserved summary rows)
    // carry one quiet spine below the tab name, while tab and header rows
    // carry none. Shell panes mark with a dim `$` where agents show a glyph.
    let mut snapshot = snapshot();
    snapshot.tabs.push(tab("tab_2", "ws_1", 2, "servers"));
    snapshot
        .panes
        .push(shell_pane_in_tab("pane_2", "ws_1", "tab_2"));
    snapshot.panes.push(shell_pane("pane_3", "ws_1"));
    snapshot.agents.push(agent("pane_1"));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows[2..10],
        [
            " repo                ≡3",
            " main",
            " 1",
            " │ ⠋ herdr / pi",
            " │",
            " │ $ shell",
            " servers",
            " │ $ shell",
        ],
        "{rows:?}"
    );
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let palette = &state.config.palette;
    // Tab `1` holds focus, so its spine goes primary; `servers` stays
    // quiet. Neither ever bolds.
    for y in [5u16, 6, 7] {
        assert_eq!(buffer[(1, y)].fg, palette.text, "selected spine at {y}");
        assert!(
            !buffer[(1, y)].modifier.contains(Modifier::BOLD),
            "spine never bolds at {y}"
        );
    }
    assert_eq!(buffer[(1, 9)].fg, palette.overlay0, "quiet spine");
    assert!(
        !buffer[(1, 9)].modifier.contains(Modifier::BOLD),
        "spine never bolds"
    );
    assert!(!rows[4].contains("│"), "tab has no spine: {rows:?}");
    for glyph in ["├", "└", "─"] {
        assert!(
            rows.iter().all(|row| !row.contains(glyph)),
            "no branch glyph {glyph}: {rows:?}"
        );
    }
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
    assert_eq!(shell_row_text(Some("neon")), " │ $ neon");
    assert_eq!(shell_row_text(None), " │ $ shell");
}

#[test]
fn custom_agent_layout_flattens_and_drops_tokens_the_layout_shows() {
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
    assert_eq!(pane_row_text(&config), " │ ⠋ pi / working");
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
    assert_eq!(rows[rect.y as usize], " │ ⠋ pi / herdr");
    assert_eq!(state.hits.workspace_panes.len(), 2);
    assert_eq!(state.hits.workspace_panes[1].0.y, rect.y + 1);
}

#[test]
fn custom_agent_layout_that_resolves_empty_falls_back_to_agent_and_title() {
    let mut config = panes_config();
    config.ui.sidebar.agents.rows = vec![vec![AgentSidebarToken::Machine]];
    assert_eq!(pane_row_text(&config), " │ ⠋ pi / herdr");
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
    assert_eq!(hits.len(), usize::from(body.height) - 2);
    assert!(hits
        .iter()
        .all(|(rect, _, _)| rect.y >= body.y && rect.y < body.bottom()));
    assert_eq!(sidebar_rows(&frame)[2], " repo               ≡20");
}

#[test]
fn pane_rows_under_worktree_children_indent_with_spaces() {
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
        .position(|row| row == "       repo          ≡1")
        .expect("first child header");
    assert_eq!(
        rows[start..start + 6],
        [
            "       repo          ≡1",
            "       worktree/ws_2",
            "       │ $ shell",
            "       repo          ≡1",
            "       worktree/ws_3",
            "       │ $ shell",
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

fn elapsed_agent(pane_id: &str, age_ms: u64) -> ClientShellAgent {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let mut pi = agent(pane_id);
    pi.status_since_ms = Some(now.saturating_sub(age_ms));
    pi
}

#[test]
fn working_row_right_aligns_elapsed_on_first_row_only() {
    let mut snapshot = snapshot();
    snapshot
        .agents
        .push(elapsed_agent("pane_1", 12 * 60_000 + 30_000));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    let (rect, _, _) = state.hits.workspace_panes[0];
    let first = &rows[rect.y as usize];
    assert!(
        first.starts_with(" │ ⠋ herdr / pi"),
        "label keeps its full text: {first:?}"
    );
    assert!(first.ends_with("12m"), "elapsed right aligned: {first:?}");
    assert_eq!(
        rows.iter().filter(|row| row.contains("12m")).count(),
        1,
        "detail row carries no elapsed"
    );
}

#[test]
fn settled_row_shows_time_in_status_while_shell_rows_show_none() {
    let mut snapshot = snapshot();
    let mut pi = elapsed_agent("pane_1", 12 * 60_000 + 30_000);
    pi.agent_status = AgentStatus::Idle;
    snapshot.agents.push(pi);
    snapshot.panes.push(shell_pane("pane_2", "ws_1"));
    let mut state = panes_state(&panes_config(), snapshot);
    let frame = state.compose(100, 16).expect("panes sidebar");
    let rows = sidebar_rows(&frame);
    assert_eq!(
        rows.iter().filter(|row| row.contains("12m")).count(),
        1,
        "settled agent row keeps its elapsed: {rows:?}"
    );
    let idle = rows
        .iter()
        .find(|row| row.contains("12m"))
        .expect("idle elapsed row");
    assert!(
        idle.ends_with("12m"),
        "settled elapsed right aligned: {idle:?}"
    );
}
