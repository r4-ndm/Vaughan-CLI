//! Recurring-buy (DCA) plans for Sentient TUI sessions.
//!
//! Layering (keep thin):
//! - [`types`] — serde models only
//! - [`store`] — atomic disk I/O
//! - [`trigger`] — pure “is due?” math
//! - [`engine`] — quote → [`crate::sentient::SentientTrader`] → record
//!
//! Phase 2 indicator triggers plug in via [`types::DcaTrigger`] without rewriting
//! the engine. Do not put TUI widgets or MCP JSON shaping here.

pub mod build;
pub mod engine;
pub mod store;
pub mod trigger;
pub mod types;

pub use build::{build_plan, build_proposal};
pub use engine::{fire_plan, next_due_plan_id, now_unix, poll_due_id, DcaFireResult};
pub use store::{add_plan, cancel_plan, load_plans, save_plans, set_paused};
pub use trigger::{advance_schedule, due_indices, is_due};
pub use types::{
    agg_picker_labels, dex_picker_slugs, DcaPlan, DcaPlanFile, DcaPlanProposal, DcaRouteResolved,
    DcaStatus, DcaTrigger, DcaVenue, SliceLog, DCA_PLANS_JSON, MAX_CONSECUTIVE_FAILURES,
    MIN_INTERVAL_SECS,
};
