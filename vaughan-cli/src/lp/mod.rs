//! `vaughan lp` — LP Brew plan/deploy CLI (adapters over `vaughan_core::core::lp_*`).

mod deploy;

pub use deploy::{run_lp_deploy, run_lp_plan, LpBrewArgs, LpDeployArgs, LpPlanArgs};
