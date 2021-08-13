#[cfg(test)]
#[path = "../../../tests/unit/construction/heuristics/selectors_test.rs"]
mod selectors_test;

use crate::construction::heuristics::*;
use crate::models::problem::Job;
use crate::utils::{map_reduce, parallel_collect, Either, Noise};
use rand::prelude::*;

/// On each insertion step, selects a list of routes where jobs can be inserted.
/// It is up to implementation to decide whether list consists of all possible routes or just some subset.
pub trait RouteSelector {
    /// Returns routes for job insertion.
    fn select<'a>(&'a self, ctx: &'a mut InsertionContext, jobs: &[Job])
        -> Box<dyn Iterator<Item = RouteContext> + 'a>;
}

/// Returns a list of all possible routes for insertion.
pub struct AllRouteSelector {}

impl Default for AllRouteSelector {
    fn default() -> Self {
        Self {}
    }
}

impl RouteSelector for AllRouteSelector {
    fn select<'a>(
        &'a self,
        ctx: &'a mut InsertionContext,
        _jobs: &[Job],
    ) -> Box<dyn Iterator<Item = RouteContext> + 'a> {
        ctx.solution.routes.shuffle(&mut ctx.environment.random.get_rng());
        Box::new(ctx.solution.routes.iter().cloned().chain(ctx.solution.registry.next()))
    }
}

/// On each insertion step, selects a list of jobs to be inserted.
/// It is up to implementation to decide whether list consists of all jobs or just some subset.
pub trait JobSelector {
    /// Returns a portion of all jobs.
    fn select<'a>(&'a self, ctx: &'a mut InsertionContext) -> Box<dyn Iterator<Item = Job> + 'a>;
}

/// Returns a list of all jobs to be inserted.
pub struct AllJobSelector {}

impl Default for AllJobSelector {
    fn default() -> Self {
        Self {}
    }
}

impl JobSelector for AllJobSelector {
    fn select<'a>(&'a self, ctx: &'a mut InsertionContext) -> Box<dyn Iterator<Item = Job> + 'a> {
        ctx.solution.required.shuffle(&mut ctx.environment.random.get_rng());

        Box::new(ctx.solution.required.iter().cloned())
    }
}

/// Evaluates insertion.
pub trait InsertionEvaluator {
    /// Evaluates insertion of a single job into given collection of routes.
    fn evaluate_job<'a>(
        &self,
        ctx: &'a InsertionContext,
        job: &Job,
        routes: &[RouteContext],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>);

    /// Evaluates insertion of multiple jobs into given route.
    fn evaluate_route<'a>(
        &self,
        ctx: &'a InsertionContext,
        route: &RouteContext,
        jobs: &[Job],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>);

    /// Evaluates insertion of a job collection into given collection of routes.
    fn evaluate_all<'a>(
        &self,
        ctx: &'a InsertionContext,
        jobs: &[Job],
        routes: &[RouteContext],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>);
}

/// Evaluates job insertion in routes at given position.
pub struct PositionInsertionEvaluator {
    insertion_position: InsertionPosition,
}

impl Default for PositionInsertionEvaluator {
    fn default() -> Self {
        Self::new(InsertionPosition::Any)
    }
}

impl PositionInsertionEvaluator {
    /// Creates a new instance of `PositionInsertionEvaluator`.
    pub fn new(insertion_position: InsertionPosition) -> Self {
        Self { insertion_position }
    }

    /// Evaluates all jobs ad routes.
    pub(crate) fn evaluate_and_collect_all<'a>(
        &self,
        ctx: &'a InsertionContext,
        jobs: &[Job],
        routes: &[RouteContext],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (Vec<InsertionResult>, InsertionCache<'a>) {
        let (results, caches): (Vec<_>, Vec<_>) = if Self::is_fold_jobs(ctx) {
            parallel_collect(jobs, |job| self.evaluate_job(ctx, job, routes, result_selector))
        } else {
            parallel_collect(routes, |route| self.evaluate_route(ctx, route, jobs, result_selector))
        }
        .into_iter()
        .unzip();

        (results, caches.into_iter().fold(InsertionCache::new(ctx), InsertionCache::merge))
    }

    fn is_fold_jobs(ctx: &InsertionContext) -> bool {
        // NOTE can be performance beneficial to use concrete strategy depending on jobs/routes ratio,
        //      but this approach brings better exploration results
        ctx.environment.random.is_head_not_tails()
    }
}

impl InsertionEvaluator for PositionInsertionEvaluator {
    fn evaluate_job<'a>(
        &self,
        ctx: &'a InsertionContext,
        job: &Job,
        routes: &[RouteContext],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>) {
        let mut cache = InsertionCache::new(ctx);
        (
            routes.iter().fold(InsertionResult::make_failure(), |acc, route_ctx| {
                evaluate_job_insertion_in_route(
                    ctx,
                    route_ctx,
                    job,
                    self.insertion_position,
                    acc,
                    &mut cache,
                    result_selector,
                )
            }),
            cache,
        )
    }

    fn evaluate_route<'a>(
        &self,
        ctx: &'a InsertionContext,
        route: &RouteContext,
        jobs: &[Job],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>) {
        let mut cache = InsertionCache::new(ctx);
        (
            jobs.iter().fold(InsertionResult::make_failure(), |acc, job| {
                evaluate_job_insertion_in_route(
                    ctx,
                    route,
                    job,
                    self.insertion_position,
                    acc,
                    &mut cache,
                    result_selector,
                )
            }),
            cache,
        )
    }

    fn evaluate_all<'a>(
        &self,
        ctx: &'a InsertionContext,
        jobs: &[Job],
        routes: &[RouteContext],
        result_selector: &(dyn ResultSelector + Send + Sync),
    ) -> (InsertionResult, InsertionCache<'a>) {
        if Self::is_fold_jobs(ctx) {
            map_reduce(
                jobs,
                |job| self.evaluate_job(ctx, job, routes, result_selector),
                || (InsertionResult::make_failure(), InsertionCache::new(ctx)),
                |(a_result, a_cache), (b_result, b_cache)| {
                    (result_selector.select_insertion(ctx, a_result, b_result), InsertionCache::merge(a_cache, b_cache))
                },
            )
        } else {
            map_reduce(
                routes,
                |route| self.evaluate_route(ctx, route, jobs, result_selector),
                || (InsertionResult::make_failure(), InsertionCache::new(ctx)),
                |(a_result, a_cache), (b_result, b_cache)| {
                    (result_selector.select_insertion(ctx, a_result, b_result), InsertionCache::merge(a_cache, b_cache))
                },
            )
        }
    }
}

/// Insertion result selector.
pub trait ResultSelector {
    /// Selects one insertion result from two to promote as best.
    fn select_insertion(
        &self,
        ctx: &InsertionContext,
        left: InsertionResult,
        right: InsertionResult,
    ) -> InsertionResult;

    /// Selects one insertion result from two to promote as best.
    fn select_cost(&self, _route_ctx: &RouteContext, left: f64, right: f64) -> Either {
        if left < right {
            Either::Left
        } else {
            Either::Right
        }
    }
}

/// Selects best result.
pub struct BestResultSelector {}

impl Default for BestResultSelector {
    fn default() -> Self {
        Self {}
    }
}

impl ResultSelector for BestResultSelector {
    fn select_insertion(&self, _: &InsertionContext, left: InsertionResult, right: InsertionResult) -> InsertionResult {
        InsertionResult::choose_best_result(left, right)
    }
}

/// Selects results with noise.
pub struct NoiseResultSelector {
    noise: Noise,
}

impl NoiseResultSelector {
    /// Creates a new instance of `NoiseResultSelector`.
    pub fn new(noise: Noise) -> Self {
        Self { noise }
    }
}

impl ResultSelector for NoiseResultSelector {
    fn select_insertion(&self, _: &InsertionContext, left: InsertionResult, right: InsertionResult) -> InsertionResult {
        match (&left, &right) {
            (InsertionResult::Success(_), InsertionResult::Failure(_)) => left,
            (InsertionResult::Failure(_), InsertionResult::Success(_)) => right,
            (InsertionResult::Success(left_success), InsertionResult::Success(right_success)) => {
                let left_cost = self.noise.add(left_success.cost);
                let right_cost = self.noise.add(right_success.cost);

                if left_cost < right_cost {
                    left
                } else {
                    right
                }
            }
            _ => right,
        }
    }

    fn select_cost(&self, _route_ctx: &RouteContext, left: f64, right: f64) -> Either {
        let left = self.noise.add(left);
        let right = self.noise.add(right);

        if left < right {
            Either::Left
        } else {
            Either::Right
        }
    }
}
