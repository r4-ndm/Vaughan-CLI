//! In-chat `/model` picker (OpenCode-style filter + list).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem},
    Frame,
};
use vaughan_agent::{
    create_llm_client, load_file_config, models_for_provider, normalize_gemini_model,
    parse_model_ref, provider_id, save_file_config, AgentFileConfig, ProviderType,
};

use super::{AgentMessage, AgentView};
use crate::app::KeyOutcome;
use crate::brand;

/// OpenCode-inspired `/model` overlay: filter + ↑/↓ list for the active provider.
pub(super) struct ModelPicker {
    pub(super) filter: String,
    pub(super) selected: usize,
}

impl AgentView {
    pub(super) fn open_model_picker(&mut self) {
        let selected = models_for_provider(self.config.provider)
            .iter()
            .position(|m| m.id == self.config.model_name)
            .unwrap_or(0);
        self.model_picker = Some(ModelPicker {
            filter: String::new(),
            selected,
        });
        self.status =
            "Select model (↑/↓, type to filter, Enter) — Esc cancels. Custom ids allowed.".into();
    }

    /// Visible picker rows: `(model_id, label)`.
    fn picker_rows(&self) -> Vec<(String, String)> {
        let Some(picker) = &self.model_picker else {
            return Vec::new();
        };
        let filter = picker.filter.trim().to_ascii_lowercase();
        let mut rows: Vec<(String, String)> = models_for_provider(self.config.provider)
            .iter()
            .filter(|m| {
                if filter.is_empty() {
                    return true;
                }
                m.id.to_ascii_lowercase().contains(&filter)
                    || m.label.to_ascii_lowercase().contains(&filter)
            })
            .map(|m| (m.id.to_string(), m.label.to_string()))
            .collect();

        let raw = picker.filter.trim();
        if !raw.is_empty() && !rows.iter().any(|(id, _)| id.eq_ignore_ascii_case(raw)) {
            rows.insert(0, (raw.to_string(), "custom".into()));
        }
        if rows.is_empty() && !raw.is_empty() {
            rows.push((raw.to_string(), "custom".into()));
        }
        rows
    }

    fn apply_model(&mut self, model_id: &str) {
        let model = if self.config.provider == ProviderType::Gemini {
            normalize_gemini_model(model_id)
        } else {
            model_id.trim().to_string()
        };
        if model.is_empty() {
            self.history
                .push(AgentMessage::System("Model id cannot be empty.".into()));
            return;
        }
        self.config.model_name = model;
        self.llm =
            create_llm_client(self.config.clone()).expect("LLM client construction is infallible");
        self.refresh_badge();
        self.persist_model();
        self.model_picker = None;
        self.status.clear();
        self.history.push(AgentMessage::System(format!(
            "Model set to {}/{}. Conversation history kept.",
            provider_id(self.config.provider),
            self.config.model_name
        )));
    }

    fn persist_model(&self) {
        let Some(dir) = &self.profile_dir else {
            return;
        };
        let file = match load_file_config(dir) {
            Ok(Some(mut existing)) => {
                existing.model = self.config.model_name.clone();
                existing
            }
            _ => match self.config.provider {
                ProviderType::Ollama => AgentFileConfig::ollama(&self.config.model_name),
                ProviderType::Gemini => AgentFileConfig::gemini(&self.config.model_name),
                ProviderType::OpenAi => AgentFileConfig::openai(
                    &self.config.model_name,
                    Some(self.config.endpoint_url.clone()),
                ),
                ProviderType::Cursor => {
                    let mut cfg = AgentFileConfig::cursor(&self.config.model_name);
                    cfg.endpoint_url = Some(self.config.endpoint_url.clone());
                    cfg
                }
            },
        };
        if let Err(e) = save_file_config(dir, &file) {
            // Non-fatal — session switch still applies.
            tracing::warn!("failed to persist model to agent.toml: {e}");
        }
    }

    pub(super) fn handle_model_command(&mut self, rest: &str) -> KeyOutcome {
        let rest = rest.trim();
        if rest.is_empty() {
            self.open_model_picker();
            return KeyOutcome::Consumed;
        }
        let Some((provider_override, model_id)) = parse_model_ref(rest) else {
            self.history.push(AgentMessage::System(
                "Usage: /model  |  /model <id>  |  /model provider/id".into(),
            ));
            return KeyOutcome::Consumed;
        };
        if let Some(requested) = provider_override {
            if requested != self.config.provider {
                self.history.push(AgentMessage::System(format!(
                    "Provider switch to `{}` needs API key / endpoint setup — type /provider. \
                     Same-provider: /model <id> (current: {}).",
                    provider_id(requested),
                    provider_id(self.config.provider)
                )));
                return KeyOutcome::Consumed;
            }
        }
        self.apply_model(&model_id);
        KeyOutcome::Consumed
    }

    pub(super) fn handle_model_picker_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let rows = self.picker_rows();
        let row_count = rows.len();

        match key.code {
            KeyCode::Esc => {
                self.model_picker = None;
                self.status.clear();
                KeyOutcome::Consumed
            }
            KeyCode::Up => {
                if let Some(p) = self.model_picker.as_mut() {
                    if p.selected > 0 {
                        p.selected -= 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                if let Some(p) = self.model_picker.as_mut() {
                    if row_count > 0 && p.selected + 1 < row_count {
                        p.selected += 1;
                    }
                }
                KeyOutcome::Consumed
            }
            KeyCode::Enter => {
                if row_count == 0 {
                    self.history.push(AgentMessage::System(
                        "No models match — type a custom id or Esc to cancel.".into(),
                    ));
                    return KeyOutcome::Consumed;
                }
                let selected = self
                    .model_picker
                    .as_ref()
                    .map(|p| p.selected.min(row_count - 1))
                    .unwrap_or(0);
                let model_id = rows[selected].0.clone();
                self.apply_model(&model_id);
                KeyOutcome::Consumed
            }
            KeyCode::Backspace => {
                if let Some(p) = self.model_picker.as_mut() {
                    p.filter.pop();
                    p.selected = 0;
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char(c) if !c.is_control() => {
                if let Some(p) = self.model_picker.as_mut() {
                    p.filter.push(c);
                    p.selected = 0;
                }
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    pub(super) fn render_model_picker(&self, frame: &mut Frame, area: Rect, picker: &ModelPicker) {
        let rows = self.picker_rows();
        let selected = if rows.is_empty() {
            0
        } else {
            picker.selected.min(rows.len() - 1)
        };
        let filter_hint = if picker.filter.is_empty() {
            "(type to filter)".to_string()
        } else {
            format!("filter: {}", picker.filter)
        };
        let title = format!(
            " Select model · {} · {filter_hint} ",
            provider_id(self.config.provider)
        );

        let items: Vec<ListItem> = if rows.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "No matches — keep typing a custom model id",
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            rows.iter()
                .enumerate()
                .map(|(i, (id, label))| {
                    let current = id == &self.config.model_name;
                    let marker = if current { "★ " } else { "  " };
                    let line = format!("{marker}{id}  —  {label}");
                    let style = if i == selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if current {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(line, style)))
                })
                .collect()
        };

        // Clear underlay so the list doesn't blend into chat history.
        frame.render_widget(Clear, area);
        let list = List::new(items);
        let inner = brand::render_faded_box(frame, area, Some(brand::fade_line(&title)));
        frame.render_widget(list, inner);
    }
}
