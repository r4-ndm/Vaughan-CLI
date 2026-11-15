//! Sentient policy root-approval wait screen (`sudo vaughan signerd approve`).
//!
//! Callers: [`super::settings::SettingsView`] constructs [`PolicyApproval`] on
//! submit, delegates rendering to [`render`], delegates key handling while
//! waiting, and drives [`PolicyApproval::poll`] from the UI tick loop.
//!
//! Must NOT hold policy field-editing widgets, daemon transport framing, or
//! secret-entry inputs; step-up RPC orchestration stays in
//! `vaughan_core::core::policy_step_up`.

use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use serde_json::Value;
use tokio::runtime::Handle;
use vaughan_agent::sentient::AgentSessionPolicy;
use vaughan_core::core::policy_step_up::{self, Staged, StepUpError, StepUpState};
use vaughan_core::core::SignerClient;

use crate::brand;

/// Minimum UI ticks between `policy_pending` socket polls (~2s at 80ms/tick).
pub const POLL_INTERVAL_TICKS: u64 = 25;

/// Exact operator command to approve the staged policy update as root.
pub const APPROVE_COMMAND: &str = "sudo vaughan signerd approve";

/// Exact console guidance shown while waiting for root approval.
pub const CONSOLE_ADVICE: &str =
    "run it from a clean console (Ctrl+Alt+F3) or run0, not a normal terminal tab";

/// Status message shown when leaving the wait screen before root approval.
pub const CANCELLED_STATUS: &str =
    "Policy update not applied; the staged update stays pending until it expires (10 min).";

/// Semantic styling role for a line on the root-approval wait card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    /// Primary informational text (summary, digest).
    Normal,
    /// Highlighted root command the operator must run.
    Command,
    /// Secondary guidance or countdown text.
    Muted,
    /// Blank vertical separator line.
    Spacer,
}

/// A single structured line rendered on the root-approval wait card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// Line text content (empty for [`Emphasis::Spacer`]).
    pub text: String,
    /// Semantic emphasis determining visual styling in [`render`].
    pub emphasis: Emphasis,
}

impl StatusLine {
    fn new(emphasis: Emphasis, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis,
        }
    }

    fn spacer() -> Self {
        Self {
            text: String::new(),
            emphasis: Emphasis::Spacer,
        }
    }
}

/// Outcome of a single non-blocking poll tick on [`PolicyApproval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Still waiting for root approval (or throttled between socket polls).
    Waiting,
    /// Root approval observed and `policy_update` applied with the edited draft.
    Applied {
        /// Active policy digest returned by `policy_update`.
        digest: String,
    },
    /// Poll or apply failed (including [`StepUpError::Expired`]).
    Failed(StepUpError),
}

/// Key action while the root-approval wait screen is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ignore key and keep waiting.
    None,
    /// Cancel waiting and return to the policy editor without applying.
    Cancel,
}

/// State of an in-flight Sentient policy root-approval step-up in the TUI.
#[derive(Debug, Clone)]
pub struct PolicyApproval {
    staged: Staged,
    state: StepUpState,
    last_poll_tick: Option<u64>,
    summary: String,
}

fn run_step_up<F, T>(handle: &Handle, fut: F) -> Result<T, StepUpError>
where
    F: Future<Output = Result<T, StepUpError>> + Send,
    T: Send,
{
    crate::blocking::block_on_handle(handle, fut)
        .map_err(|e| StepUpError::Transport(e.to_string()))?
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn policy_summary(p: &AgentSessionPolicy) -> String {
    format!(
        "enforcement={}  max_position={}  daily_cap={}  slippage={}bps  unsafe={}",
        p.enforcement.as_str(),
        p.max_position_pct,
        p.daily_cap_pct,
        p.max_slippage_bps,
        p.acknowledge_unsafe,
    )
}

fn emphasis_style(emphasis: Emphasis) -> Style {
    match emphasis {
        Emphasis::Normal | Emphasis::Spacer => Style::default(),
        Emphasis::Command => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        Emphasis::Muted => Style::default().fg(Color::DarkGray),
    }
}

impl PolicyApproval {
    /// Serialize `draft` and stage it on the default [`SignerClient`].
    pub fn start_draft(
        handle: &Handle,
        draft: &AgentSessionPolicy,
        current_digest: &str,
    ) -> Result<Self, String> {
        let draft_json =
            serde_json::to_value(draft).map_err(|e| format!("Policy encode error: {e}"))?;
        let expected = (!current_digest.is_empty()).then(|| current_digest.to_string());
        Self::start(
            handle,
            &SignerClient::default(),
            draft_json,
            expected,
            policy_summary(draft),
        )
        .map_err(|e| e.to_string())
    }

    /// Stage `draft_json` via `policy_step_up::stage` without blocking on approval.
    pub fn start(
        handle: &Handle,
        client: &SignerClient,
        draft_json: Value,
        expected_digest: Option<String>,
        summary: String,
    ) -> Result<Self, StepUpError> {
        let staged = run_step_up(
            handle,
            policy_step_up::stage(client, &draft_json, expected_digest),
        )?;
        let state = StepUpState::Waiting {
            expires_at_unix: staged.expires_at_unix,
        };
        Ok(Self {
            staged,
            state,
            last_poll_tick: None,
            summary,
        })
    }

    /// Access the staged policy handle preserved from submit time.
    pub fn staged(&self) -> &Staged {
        &self.staged
    }

    /// Advance the step-up state on UI ticks, throttling `policy_pending` RPCs.
    pub fn poll(&mut self, tick: u64, handle: &Handle, client: &SignerClient) -> Progress {
        if let Some(prev) = self.last_poll_tick {
            if tick.saturating_sub(prev) < POLL_INTERVAL_TICKS {
                return Progress::Waiting;
            }
        }
        self.last_poll_tick = Some(tick);

        let next = match run_step_up(handle, policy_step_up::poll(client, &self.staged)) {
            Ok(state) => state,
            Err(err) => return Progress::Failed(err),
        };
        self.state = next;

        match next {
            StepUpState::Waiting { .. } => Progress::Waiting,
            StepUpState::Expired => Progress::Failed(StepUpError::Expired),
            StepUpState::Ready => {
                match run_step_up(handle, policy_step_up::apply(client, &self.staged)) {
                    Ok(digest) => Progress::Applied { digest },
                    Err(err) => Progress::Failed(err),
                }
            }
        }
    }

    /// Translate a key press while the wait screen is open.
    pub fn handle_key(&self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => Action::Cancel,
            _ => Action::None,
        }
    }

    /// Build structured wait lines with explicit emphasis roles relative to `now_unix`.
    pub fn status_lines(&self, now_unix: u64) -> Vec<StatusLine> {
        let expires_at = match self.state {
            StepUpState::Waiting { expires_at_unix } => expires_at_unix,
            StepUpState::Ready | StepUpState::Expired => self.staged.expires_at_unix,
        };
        let secs_left = expires_at.saturating_sub(now_unix);
        let digest = &self.staged.policy_digest;
        let digest_short = &digest[..digest.len().min(12)];
        vec![
            StatusLine::new(Emphasis::Normal, format!("Staged: {}", self.summary)),
            StatusLine::new(Emphasis::Normal, format!("Policy digest: {digest_short}")),
            StatusLine::spacer(),
            StatusLine::new(Emphasis::Command, format!("Run: {APPROVE_COMMAND}")),
            StatusLine::new(Emphasis::Muted, CONSOLE_ADVICE),
            StatusLine::new(
                Emphasis::Muted,
                format!(
                    "Expires in {secs_left}s · Esc returns without applying (stays pending 10 min)"
                ),
            ),
        ]
    }
}

/// Render the root-approval wait card inside `area`.
pub fn render(frame: &mut Frame, area: Rect, approval: Option<&PolicyApproval>) {
    let inner = brand::render_faded_box(
        frame,
        area,
        Some(brand::fade_line(" Root approval required (step-up) ")),
    );
    let Some(approval) = approval else {
        frame.render_widget(
            Paragraph::new("No staged policy update in progress.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    };

    let lines: Vec<Line> = approval
        .status_lines(now_unix_secs())
        .into_iter()
        .map(|line| Line::from(Span::styled(line.text, emphasis_style(line.emphasis))))
        .collect();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
