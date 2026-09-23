//! Due-check logic for DCA plans (pure; no I/O).

use super::types::{DcaPlan, DcaStatus, DcaTrigger};

/// Whether this plan may fire at `now` (unix seconds).
pub fn is_due(plan: &DcaPlan, now: u64) -> bool {
    if plan.status != DcaStatus::Active {
        return false;
    }
    if plan.slices_done >= plan.max_slices {
        return false;
    }
    match &plan.trigger {
        DcaTrigger::Time { .. } => now >= plan.next_due_at,
    }
}

/// Advance `next_due_at` after a successful (or dry-run) fire.
///
/// Catch-up: if the laptop slept through several intervals, schedule the *next*
/// interval from `now` (not from the missed `next_due_at`) so we never burst.
pub fn advance_schedule(plan: &mut DcaPlan, now: u64) {
    let Some(interval) = plan.trigger.interval_secs() else {
        return;
    };
    let interval = interval.max(1);
    plan.next_due_at = now.saturating_add(interval);
}

/// Indices of active plans that are due, in file order (caller fires at most one).
pub fn due_indices(plans: &[DcaPlan], now: u64) -> Vec<usize> {
    plans
        .iter()
        .enumerate()
        .filter(|(_, p)| is_due(p, now))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dca::types::DcaVenue;

    fn plan(due: u64, status: DcaStatus) -> DcaPlan {
        DcaPlan {
            id: "p".into(),
            chain_id: 369,
            account: "0x1111111111111111111111111111111111111111".into(),
            token_out: "0x2222222222222222222222222222222222222222".into(),
            slice_amount_wei: "1".into(),
            trigger: DcaTrigger::Time {
                interval_secs: 3600,
            },
            venue: DcaVenue::default(),
            max_slippage_bps: 100,
            max_slices: 5,
            slices_done: 0,
            spent_wei: "0".into(),
            consecutive_failures: 0,
            status,
            created_at: 0,
            next_due_at: due,
            dry_run: true,
            slice_log: vec![],
        }
    }

    #[test]
    fn due_only_when_active_and_time_reached() {
        assert!(is_due(&plan(100, DcaStatus::Active), 100));
        assert!(is_due(&plan(100, DcaStatus::Active), 200));
        assert!(!is_due(&plan(100, DcaStatus::Active), 99));
        assert!(!is_due(&plan(100, DcaStatus::Paused), 200));
    }

    #[test]
    fn catch_up_does_not_burst() {
        let mut p = plan(0, DcaStatus::Active);
        // Missed many hours; after one fire, next is now+interval (not backlog).
        advance_schedule(&mut p, 10_000);
        assert_eq!(p.next_due_at, 10_000 + 3600);
    }

    #[test]
    fn due_indices_order() {
        let plans = vec![
            plan(50, DcaStatus::Active),
            plan(10, DcaStatus::Paused),
            plan(20, DcaStatus::Active),
        ];
        assert_eq!(due_indices(&plans, 100), vec![0, 2]);
    }
}
