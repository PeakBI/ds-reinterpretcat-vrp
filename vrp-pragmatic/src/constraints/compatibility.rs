#[cfg(test)]
#[path = "../../tests/unit/constraints/compatibility_test.rs"]
mod compatibility_test;

use crate::constraints::COMPATIBILITY_DIMEN_KEY;
use std::slice::Iter;
use std::sync::Arc;
use vrp_core::construction::constraints::*;
use vrp_core::construction::heuristics::{RouteContext, SolutionContext};
use vrp_core::models::common::ValueDimension;
use vrp_core::models::problem::Job;

/// A compatibility module provides the way to avoid assigning some jobs in the same tour.
pub struct CompatibilityModule {
    code: i32,
    constraints: Vec<ConstraintVariant>,
    state_key: i32,
    state_keys: Vec<i32>,
    dimen_keys: Vec<i32>,
}

impl CompatibilityModule {
    /// Creates a new instance of `CompatibilityModule`.
    pub fn new(code: i32, state_key: i32) -> Self {
        Self {
            code,
            constraints: vec![ConstraintVariant::HardRoute(Arc::new(CompatibilityHardRouteConstraint {
                code,
                state_key,
            }))],
            state_key,
            state_keys: vec![state_key],
            dimen_keys: vec![COMPATIBILITY_DIMEN_KEY],
        }
    }
}

impl ConstraintModule for CompatibilityModule {
    fn accept_insertion(&self, solution_ctx: &mut SolutionContext, route_index: usize, job: &Job) {
        if get_job_compatibility(job).is_some() {
            self.accept_route_state(solution_ctx.routes.get_mut(route_index).unwrap())
        }
    }

    fn accept_route_state(&self, ctx: &mut RouteContext) {
        let new_comp = get_route_compatibility(ctx);
        let current_compat = ctx.state.get_route_state::<Option<String>>(self.state_key);

        match (new_comp, current_compat) {
            (None, None) => {}
            (None, Some(_)) => {
                ctx.state_mut().put_route_state::<Option<String>>(self.state_key, None);
            }
            (value, None) | (value, Some(_)) => ctx.state_mut().put_route_state(self.state_key, value),
        }
    }

    fn accept_solution_state(&self, _: &mut SolutionContext) {}

    fn merge(&self, source: Job, candidate: Job) -> Result<Job, i32> {
        match (get_job_compatibility(&source), get_job_compatibility(&candidate)) {
            (None, None) => Ok(source),
            (Some(s_compat), Some(c_compat)) if s_compat == c_compat => Ok(source),
            _ => Err(self.code),
        }
    }

    fn state_keys(&self) -> Iter<i32> {
        self.state_keys.iter()
    }

    fn dimen_keys(&self) -> Iter<i32> {
        self.dimen_keys.iter()
    }

    fn get_constraints(&self) -> Iter<ConstraintVariant> {
        self.constraints.iter()
    }
}

struct CompatibilityHardRouteConstraint {
    code: i32,
    state_key: i32,
}

impl HardRouteConstraint for CompatibilityHardRouteConstraint {
    fn evaluate_job(
        &self,
        _: &SolutionContext,
        route_ctx: &RouteContext,
        job: &Job,
    ) -> Option<RouteConstraintViolation> {
        get_job_compatibility(job).and_then(|job_compat| {
            match route_ctx.state.get_route_state::<Option<String>>(self.state_key) {
                None | Some(None) => None,
                Some(Some(route_compat)) if job_compat == route_compat => None,
                _ => Some(RouteConstraintViolation { code: self.code }),
            }
        })
    }
}

fn get_job_compatibility(job: &Job) -> Option<&String> {
    job.dimens().get_value::<String>(COMPATIBILITY_DIMEN_KEY)
}

fn get_route_compatibility(route_ctx: &RouteContext) -> Option<String> {
    route_ctx.route.tour.jobs().filter_map(|job| get_job_compatibility(&job).cloned()).next()
}
