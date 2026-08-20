//! Deterministic TUI Agent View tests.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use vaughan_agent::tools::ToolContext;
use vaughan_core::core::profile::OperatingMode;
use vaughan_tui::app::KeyOutcome;
use vaughan_tui::views::agent::{AgentMessage, AgentView};

fn press(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn agent_view_human_only_mode_blocks_agent_and_shows_cold_storage() {
    let mut view = AgentView::new();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    // Typing into HumanOnly view is consumed and ignored
    let outcome = view.handle_key(press('a'), OperatingMode::HumanOnly, &context);
    assert!(matches!(outcome, KeyOutcome::Consumed));

    // Esc bubbles up to navigate back to dashboard
    let outcome = view.handle_key(key(KeyCode::Esc), OperatingMode::HumanOnly, &context);
    assert!(matches!(outcome, KeyOutcome::NotHandled));

    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::HumanOnly);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("HUMAN PURIST MODE"));
    assert!(content.contains("AI agent subsystem is completely deactivated"));
}

#[test]
fn agent_view_ai_assisted_mode_renders_chat_and_proposals() {
    let mut view = AgentView::new();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    view.add_message(AgentMessage::User("inspect 0x1111".to_string()));
    view.add_message(AgentMessage::Assistant("Contract is ERC-20".to_string()));

    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::AiAssisted);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("[You]: inspect 0x1111"));
    assert!(content.contains("[Advisor]: Contract is ERC-20"));
}
