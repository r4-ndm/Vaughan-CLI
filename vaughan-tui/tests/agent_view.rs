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
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let mut view = AgentView::new();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    // Typing into HumanOnly view is consumed and ignored
    let outcome = view.handle_key(press('a'), OperatingMode::HumanOnly, &context, &handle);
    assert!(matches!(outcome, KeyOutcome::Consumed));

    // Esc bubbles up to navigate back to dashboard
    let outcome = view.handle_key(
        key(KeyCode::Esc),
        OperatingMode::HumanOnly,
        &context,
        &handle,
    );
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

#[test]
fn agent_view_model_command_opens_picker() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let mut view = AgentView::with_config(vaughan_agent::ModelConfig::default_local_ollama());
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    for c in "/model".chars() {
        let _ = view.handle_key(press(c), OperatingMode::AiAssisted, &context, &handle);
    }
    let outcome = view.handle_key(
        key(KeyCode::Enter),
        OperatingMode::AiAssisted,
        &context,
        &handle,
    );
    assert!(matches!(outcome, KeyOutcome::Consumed));

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::AiAssisted);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("Select model"), "{content}");
    assert!(content.contains("llama3.2"), "{content}");
}

#[test]
fn agent_view_model_direct_switch() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let mut view = AgentView::with_config(vaughan_agent::ModelConfig::default_local_ollama());
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    for c in "/model mistral".chars() {
        let _ = view.handle_key(press(c), OperatingMode::AiAssisted, &context, &handle);
    }
    let _ = view.handle_key(
        key(KeyCode::Enter),
        OperatingMode::AiAssisted,
        &context,
        &handle,
    );
    assert_eq!(view.model_config().model_name, "mistral");
}

#[test]
fn agent_view_idle_hides_chrome_hint_footer() {
    let view = AgentView::new();
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::AiAssisted);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(
        !content.contains("p portfolio"),
        "idle agent should not show chrome hint strip:\n{content}"
    );
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn agent_view_empty_prompt_defers_chrome_hotkeys() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let mut view = AgentView::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    // Empty prompt: navigation keys bubble up.
    assert!(matches!(
        view.handle_key(press('h'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::NotHandled
    ));
    assert!(matches!(
        view.handle_key(press('v'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::NotHandled
    ));
    assert!(matches!(
        view.handle_key(
            key(KeyCode::Tab),
            OperatingMode::AiAssisted,
            &context,
            &handle
        ),
        KeyOutcome::NotHandled
    ));
    // `p` stays agent-local (portfolio).
    assert!(matches!(
        view.handle_key(press('p'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::StartJob(_)
    ));
    let _ = view.handle_key(
        key(KeyCode::Esc),
        OperatingMode::AiAssisted,
        &context,
        &handle,
    );

    // Non-empty prompt: non-chrome letters type into chat (`x` is a footer hotkey).
    let _ = view.handle_key(press('z'), OperatingMode::AiAssisted, &context, &handle);
    assert!(matches!(
        view.handle_key(press('h'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::Consumed
    ));
    // Ctrl+h still navigates while typing.
    assert!(matches!(
        view.handle_key(ctrl('h'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::NotHandled
    ));
    // Ctrl+p opens portfolio mid-typing.
    assert!(matches!(
        view.handle_key(ctrl('p'), OperatingMode::AiAssisted, &context, &handle),
        KeyOutcome::StartJob(_)
    ));
}

#[test]
fn agent_view_p_opens_portfolio_when_prompt_empty() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let mut view = AgentView::new();
    let context = ToolContext {
        rpc_url: "http://127.0.0.1:8545".to_string(),
        chain_id: 31337,
        active_address: None,
    };

    let outcome = view.handle_key(press('p'), OperatingMode::AiAssisted, &context, &handle);
    assert!(matches!(outcome, KeyOutcome::StartJob(_)));

    view.apply_portfolio(Ok(vec![vaughan_core::chains::Balance {
        token: vaughan_core::chains::TokenInfo {
            symbol: "tPLS".into(),
            name: "Pulse".into(),
            decimals: 18,
            contract_address: None,
        },
        raw: "1000000000000000000".into(),
        formatted: "1.0".into(),
        usd_value: None,
    }]));

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::AiAssisted);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("Portfolio"), "{content}");
    assert!(content.contains("tPLS"), "{content}");

    let _ = view.handle_key(
        key(KeyCode::Esc),
        OperatingMode::AiAssisted,
        &context,
        &handle,
    );
    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), OperatingMode::AiAssisted);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(!content.contains("loading balances"), "{content}");
    assert!(!content.contains("p portfolio"), "{content}");
}
