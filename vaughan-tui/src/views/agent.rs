//! AI Agent View: Multi-mode interactive terminal interface.
//!
//! Handles:
//! - Human-Only mode cold-storage isolation notice.
//! - AI Assisted chat REPL with tool execution activity.
//! - Ground-truth transaction proposal review modal with independent bytecode rendering.
//! - Degen mode autonomous trader status and emergency stop.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry, ToolContext, ToolRegistry};
use vaughan_core::core::profile::OperatingMode;

use crate::app::KeyOutcome;
use crate::input::Input;
use crate::views::{body_areas, labeled_input, status_paragraph};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMessage {
    User(String),
    Assistant(String),
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
    System(String),
}

/// Agent View state.
pub struct AgentView {
    input: Input,
    history: Vec<AgentMessage>,
    active_proposal: Option<TxProposal>,
    status: String,
    scroll_offset: usize,
    registry: ToolRegistry,
}

impl Default for AgentView {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentView {
    pub fn new() -> Self {
        Self {
            input: Input::new(
                false,
                "Type a command (e.g. inspect 0x..., balance, transfer)...",
            ),
            history: vec![AgentMessage::System(
                "Vaughan Agent subsystem initialized. Enter a command or question.".to_string(),
            )],
            active_proposal: None,
            status: String::new(),
            scroll_offset: 0,
            registry: default_assist_registry(),
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    pub fn active_proposal(&self) -> Option<&TxProposal> {
        self.active_proposal.as_ref()
    }

    pub fn take_active_proposal(&mut self) -> Option<TxProposal> {
        self.active_proposal.take()
    }

    pub fn add_message(&mut self, msg: AgentMessage) {
        self.history.push(msg);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        mode: OperatingMode,
        context: &ToolContext,
    ) -> KeyOutcome {
        if mode == OperatingMode::HumanOnly {
            if key.code == KeyCode::Esc {
                return KeyOutcome::NotHandled;
            }
            return KeyOutcome::Consumed;
        }

        // If there is an active proposal pending approval:
        if let Some(proposal) = &self.active_proposal {
            match key.code {
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.status =
                        format!("Proposal {} approved for broadcast.", proposal.proposal_id);
                    // Leave proposal for app event loop to broadcast
                    return KeyOutcome::Consumed;
                }
                KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => {
                    self.status = format!("Proposal {} denied.", proposal.proposal_id);
                    self.active_proposal = None;
                    return KeyOutcome::Consumed;
                }
                _ => return KeyOutcome::Consumed,
            }
        }

        match key.code {
            KeyCode::Esc => KeyOutcome::NotHandled, // Bubble to app to return to dashboard
            KeyCode::Up => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.scroll_offset += 1;
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                let prompt = self.input.value().to_string();
                self.input.set_value("");
                if !prompt.trim().is_empty() {
                    self.execute_prompt(prompt.trim(), context);
                }
                KeyOutcome::Consumed
            }
            _ => {
                self.input.handle_key(key);
                KeyOutcome::Consumed
            }
        }
    }

    fn execute_prompt(&mut self, prompt: &str, context: &ToolContext) {
        self.history.push(AgentMessage::User(prompt.to_string()));

        // Local sensory / proposal command parser dispatch
        let tokens: Vec<&str> = prompt.split_whitespace().collect();
        if tokens.is_empty() {
            return;
        }

        let cmd = tokens[0].to_lowercase();
        match cmd.as_str() {
            "help" => {
                self.history.push(AgentMessage::Assistant(
                    "Supported commands:\n\
                     - balance [0x...]: Check balance\n\
                     - inspect <0x...>: Fingerprint contract ABI & candidate selectors\n\
                     - transfer <0x...> <amount_wei>: Propose native transfer\n\
                     - clear: Clear history"
                        .to_string(),
                ));
            }
            "clear" => {
                self.history.clear();
                self.history
                    .push(AgentMessage::System("History cleared.".to_string()));
            }
            "balance" => {
                let account = if tokens.len() > 1 {
                    tokens[1]
                } else if let Some(addr) = context.active_address {
                    &format!("{addr:#x}")
                } else {
                    self.history.push(AgentMessage::Assistant(
                        "Error: No account address provided".to_string(),
                    ));
                    return;
                };

                let reg = self.registry.clone();
                let ctx = context.clone();
                let acc = account.to_string();

                let res = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        reg.execute(
                            "get_balance",
                            serde_json::json!({ "account_address": acc }),
                            &ctx,
                        )
                        .await
                    })
                });

                match res {
                    Ok(val) => {
                        let bal = val["balance_wei"].as_str().unwrap_or("0");
                        self.history.push(AgentMessage::Assistant(format!(
                            "Account: {account}\nBalance: {bal} wei"
                        )));
                    }
                    Err(e) => {
                        self.history.push(AgentMessage::Assistant(format!(
                            "Error querying balance: {e}"
                        )));
                    }
                }
            }
            "inspect" => {
                if tokens.len() < 2 {
                    self.history.push(AgentMessage::Assistant(
                        "Usage: inspect <0xAddress>".to_string(),
                    ));
                    return;
                }
                let target = tokens[1].to_string();
                let reg = self.registry.clone();
                let ctx = context.clone();

                let target_addr = target.clone();
                let res = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        reg.execute(
                            "inspect_contract",
                            serde_json::json!({ "address": target_addr }),
                            &ctx,
                        )
                        .await
                    })
                });

                match res {
                    Ok(val) => {
                        self.history.push(AgentMessage::Assistant(format!(
                            "Contract Inspection for {target}:\n\
                             Fingerprint: {}\n\
                             Candidate Selectors: {}",
                            val["fingerprint"], val["candidate_selectors"]
                        )));
                    }
                    Err(e) => {
                        self.history.push(AgentMessage::Assistant(format!(
                            "Error inspecting contract: {e}"
                        )));
                    }
                }
            }
            "transfer" => {
                if tokens.len() < 3 {
                    self.history.push(AgentMessage::Assistant(
                        "Usage: transfer <0xRecipient> <amount_wei>".to_string(),
                    ));
                    return;
                }
                let recipient = tokens[1].to_string();
                let amount = tokens[2].to_string();
                let reg = self.registry.clone();
                let ctx = context.clone();

                let res = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        reg.execute(
                            "propose_transfer",
                            serde_json::json!({
                                "recipient": recipient,
                                "amount": amount,
                                "explanation": format!("User requested transfer of {amount} wei to {recipient}")
                            }),
                            &ctx,
                        )
                        .await
                    })
                });

                match res {
                    Ok(val) => match serde_json::from_value::<TxProposal>(val) {
                        Ok(prop) => {
                            self.history.push(AgentMessage::Assistant(format!(
                                "Drafted proposal: {}\nReview details in the confirmation modal below.",
                                prop.proposal_id
                            )));
                            self.active_proposal = Some(prop);
                        }
                        Err(e) => {
                            self.history.push(AgentMessage::Assistant(format!(
                                "Error decoding proposal: {e}"
                            )));
                        }
                    },
                    Err(e) => {
                        self.history.push(AgentMessage::Assistant(format!(
                            "Error creating proposal: {e}"
                        )));
                    }
                }
            }
            _ => {
                self.history.push(AgentMessage::Assistant(format!(
                    "Command not recognized: '{prompt}'. Type 'help' for available commands."
                )));
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, mode: OperatingMode) {
        let [body, status_rect] = body_areas(area);

        if mode == OperatingMode::HumanOnly {
            let p = Paragraph::new(vec![
                Line::from(Span::styled(
                    "🔒 HUMAN PURIST MODE (Cold Storage)",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("AI agent subsystem is completely deactivated and barred from running."),
                Line::from("Zero network requests to LLMs are made, and no autonomous tooling is registered."),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Esc to return to the Dashboard.",
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .block(Block::default().title(" Agent Subsystem ").borders(Borders::ALL));
            frame.render_widget(p, body);
            frame.render_widget(status_paragraph(&self.status), status_rect);
            return;
        }

        // Split body into History, Proposal Review (if any), and Input Bar
        let chunks = if self.active_proposal.is_some() {
            Layout::vertical([
                Constraint::Min(4),
                Constraint::Length(10),
                Constraint::Length(3),
            ])
            .split(body)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).split(body)
        };

        // Render History List
        let items: Vec<ListItem> = self
            .history
            .iter()
            .map(|msg| match msg {
                AgentMessage::User(u) => ListItem::new(Line::from(vec![
                    Span::styled(
                        "[You]: ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(u),
                ])),
                AgentMessage::Assistant(a) => ListItem::new(Line::from(vec![
                    Span::styled(
                        "[Advisor]: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(a),
                ])),
                AgentMessage::ToolCall { name, args } => ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("[Tool Call: {name}]: "),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(args, Style::default().fg(Color::DarkGray)),
                ])),
                AgentMessage::ToolResult { name, result } => ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("[Tool Result: {name}]: "),
                        Style::default().fg(Color::Magenta),
                    ),
                    Span::styled(result, Style::default().fg(Color::DarkGray)),
                ])),
                AgentMessage::System(s) => ListItem::new(Line::from(vec![
                    Span::styled("[System]: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(s, Style::default().fg(Color::DarkGray)),
                ])),
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .title(format!(" Agent Chat ({mode:?}) "))
                .borders(Borders::ALL),
        );
        frame.render_widget(list, chunks[0]);

        // Render Proposal Card if pending
        if let Some(prop) = &self.active_proposal {
            let sim_tag = if prop.simulation_success {
                Span::styled(" [SIMULATION: SUCCESS] ", Style::default().fg(Color::Green))
            } else {
                Span::styled(" [SIMULATION: REVERTED] ", Style::default().fg(Color::Red))
            };

            let prop_lines = vec![
                Line::from(vec![
                    Span::styled("Action: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{:?}", prop.proposal_type)),
                    sim_tag,
                ]),
                Line::from(vec![
                    Span::styled("Target: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{:#x}", prop.to)),
                    Span::raw(" | "),
                    Span::styled("Value: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{} wei", prop.value_wei)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Raw Calldata: ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:#x}", prop.calldata),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        "AI Rationale (Untrusted): ",
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(&prop.llm_explanation, Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(vec![
                    Span::styled(
                        " [a] Approve & Sign ",
                        Style::default()
                            .bg(Color::Green)
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        " [d] Deny & Discard ",
                        Style::default()
                            .bg(Color::Red)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];

            let prop_block = Paragraph::new(prop_lines).wrap(Wrap { trim: true }).block(
                Block::default()
                    .title(format!(
                        " Transaction Proposal Review — {} ",
                        prop.proposal_id
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            );
            frame.render_widget(prop_block, chunks[1]);
        }

        // Render Input Box
        let input_chunk = if self.active_proposal.is_some() {
            chunks[2]
        } else {
            chunks[1]
        };
        let input_widget = labeled_input("Prompt", &self.input, self.active_proposal.is_none());
        frame.render_widget(input_widget, input_chunk);

        frame.render_widget(status_paragraph(&self.status), status_rect);
    }
}
