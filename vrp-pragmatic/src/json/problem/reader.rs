#[cfg(test)]
#[path = "../../../tests/unit/json/problem/reader_test.rs"]
mod reader_test;

#[path = "./job_reader.rs"]
mod job_reader;

#[path = "./fleet_reader.rs"]
mod fleet_reader;

use crate::constraints::*;
use crate::extensions::{MultiDimensionalCapacity, OnlyVehicleActivityCost};
use crate::json::coord_index::CoordIndex;
use crate::json::problem::reader::fleet_reader::{create_transport_costs, read_fleet, read_limits};
use crate::json::problem::reader::job_reader::{read_jobs_with_extra_locks, read_locks};
use crate::json::problem::{deserialize_matrix, deserialize_problem, Matrix, Objective};
use crate::json::*;
use crate::validation::ValidationContext;
use crate::{parse_time, StringReader};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::iter::FromIterator;
use std::sync::Arc;
use vrp_core::construction::constraints::*;
use vrp_core::models::common::{Cost, Dimensions, TimeWindow, ValueDimension};
use vrp_core::models::problem::{ActivityCost, Fleet, Job, TransportCost};
use vrp_core::models::{Extras, Lock, Problem};
use vrp_core::refinement::objectives::Objective as CoreObjective;
use vrp_core::refinement::objectives::{MultiObjective, TotalRoutes, TotalTransportCost, TotalUnassignedJobs};

pub type ApiProblem = crate::json::problem::Problem;
pub type JobIndex = HashMap<String, Job>;

/// Reads specific problem definition from various sources.
pub trait PragmaticProblem {
    fn read_pragmatic(self) -> Result<Problem, String>;
}

impl PragmaticProblem for (File, Vec<File>) {
    fn read_pragmatic(self) -> Result<Problem, String> {
        let problem = deserialize_problem(BufReader::new(&self.0)).map_err(|err| err.to_string())?;

        let matrices = self.1.iter().fold(vec![], |mut acc, matrix| {
            acc.push(deserialize_matrix(BufReader::new(matrix)).unwrap());
            acc
        });

        map_to_problem(problem, matrices)
    }
}

impl PragmaticProblem for (String, Vec<String>) {
    fn read_pragmatic(self) -> Result<Problem, String> {
        let problem = deserialize_problem(BufReader::new(StringReader::new(&self.0))).map_err(|err| err.to_string())?;

        let matrices = self.1.iter().fold(vec![], |mut acc, matrix| {
            acc.push(deserialize_matrix(BufReader::new(StringReader::new(matrix))).unwrap());
            acc
        });

        map_to_problem(problem, matrices)
    }
}

impl PragmaticProblem for (ApiProblem, Vec<Matrix>) {
    fn read_pragmatic(self) -> Result<Problem, String> {
        map_to_problem(self.0, self.1)
    }
}

pub struct ProblemProperties {
    has_multi_dimen_capacity: bool,
    has_breaks: bool,
    has_skills: bool,
    has_unreachable_locations: bool,
    has_reload: bool,
    priority: Option<Cost>,
}

fn map_to_problem(api_problem: ApiProblem, matrices: Vec<Matrix>) -> Result<Problem, String> {
    ValidationContext::new(&api_problem, Some(&matrices)).validate()?;

    let problem_props = get_problem_properties(&api_problem, &matrices);

    let coord_index = CoordIndex::new(&api_problem);
    let transport = Arc::new(create_transport_costs(&matrices));
    let activity = Arc::new(OnlyVehicleActivityCost::default());
    let fleet = read_fleet(&api_problem, &problem_props, &coord_index);

    let mut job_index = Default::default();
    let (jobs, locks) = read_jobs_with_extra_locks(
        &api_problem,
        &problem_props,
        &coord_index,
        &fleet,
        transport.as_ref(),
        &mut job_index,
    );
    let locks = locks.into_iter().chain(read_locks(&api_problem, &job_index).into_iter()).collect();
    let limits = read_limits(&api_problem).unwrap_or_else(|| Arc::new(|_| (None, None)));
    let extras = Arc::new(create_extras(&problem_props, coord_index));
    let mut constraint =
        create_constraint_pipeline(&fleet, activity.clone(), transport.clone(), &problem_props, &locks, limits);

    let objective = Arc::new(create_objective(&api_problem, &mut constraint, &problem_props));

    Ok(Problem {
        fleet: Arc::new(fleet),
        jobs: Arc::new(jobs),
        locks,
        constraint: Arc::new(constraint),
        activity,
        transport,
        objective,
        extras,
    })
}

fn create_constraint_pipeline(
    fleet: &Fleet,
    activity: Arc<dyn ActivityCost + Send + Sync>,
    transport: Arc<dyn TransportCost + Send + Sync>,
    props: &ProblemProperties,
    locks: &Vec<Arc<Lock>>,
    limits: TravelLimitFunc,
) -> ConstraintPipeline {
    let mut constraint = ConstraintPipeline::default();
    constraint.add_module(Box::new(TransportConstraintModule::new(
        activity.clone(),
        transport.clone(),
        limits,
        1,
        2,
        3,
    )));

    add_capacity_module(&mut constraint, &props);

    if props.has_breaks {
        constraint.add_module(Box::new(BreakModule::new(BREAK_CONSTRAINT_CODE, Some(-100.), false)));
    }

    if props.has_skills {
        constraint.add_module(Box::new(SkillsModule::new(SKILLS_CONSTRAINT_CODE)));
    }

    if let Some(priority_cost) = props.priority {
        constraint.add_module(Box::new(PriorityModule::new(priority_cost, PRIORITY_CONSTRAINT_CODE)));
    }

    if !locks.is_empty() {
        constraint.add_module(Box::new(StrictLockingModule::new(fleet, locks.clone(), LOCKING_CONSTRAINT_CODE)));
    }

    if props.has_unreachable_locations {
        constraint.add_module(Box::new(ReachableModule::new(transport.clone(), REACHABLE_CONSTRAINT_CODE)));
    }

    constraint
}

fn add_capacity_module(constraint: &mut ConstraintPipeline, props: &ProblemProperties) {
    constraint.add_module(if props.has_reload {
        let threshold = 0.9;
        if props.has_multi_dimen_capacity {
            Box::new(CapacityConstraintModule::<MultiDimensionalCapacity>::new_with_multi_trip(
                CAPACITY_CONSTRAINT_CODE,
                Arc::new(ReloadMultiTrip::new(Box::new(|capacity| *capacity * 0.9))),
            ))
        } else {
            Box::new(CapacityConstraintModule::<i32>::new_with_multi_trip(
                CAPACITY_CONSTRAINT_CODE,
                Arc::new(ReloadMultiTrip::new(Box::new(move |capacity| (*capacity as f64 * threshold).round() as i32))),
            ))
        }
    } else {
        if props.has_multi_dimen_capacity {
            Box::new(CapacityConstraintModule::<MultiDimensionalCapacity>::new(CAPACITY_CONSTRAINT_CODE))
        } else {
            Box::new(CapacityConstraintModule::<i32>::new(CAPACITY_CONSTRAINT_CODE))
        }
    });
}

fn add_work_balance_module(constraint: &mut ConstraintPipeline, props: &ProblemProperties) {
    // TODO do not use hard coded penalty
    let balance_penalty = 1000.;
    if props.has_multi_dimen_capacity {
        constraint.add_module(Box::new(WorkBalanceModule::new_load_balanced::<MultiDimensionalCapacity>(
            balance_penalty,
            Box::new(|loaded, total| {
                let mut max_ratio = 0_f64;

                for (idx, value) in total.capacity.iter().enumerate() {
                    let ratio = loaded.capacity[idx] as f64 / *value as f64;
                    max_ratio = max_ratio.max(ratio);
                }

                max_ratio
            }),
        )));
    } else {
        constraint.add_module(Box::new(WorkBalanceModule::new_load_balanced::<i32>(
            balance_penalty,
            Box::new(|loaded, capacity| *loaded as f64 / *capacity as f64),
        )));
    }
}

fn create_objective(
    api_problem: &ApiProblem,
    constraint: &mut ConstraintPipeline,
    props: &ProblemProperties,
) -> MultiObjective {
    if let Some(objectives) = &api_problem.objectives {
        let mut map_objectives = |objectives: &Vec<Objective>| {
            let mut core_objectives: Vec<Box<dyn CoreObjective + Send + Sync>> = vec![];
            let mut cost_idx = None;
            objectives.iter().enumerate().for_each(|(idx, objective)| match objective {
                Objective::MinimizeCost { goal: _ } => {
                    cost_idx = Some(idx);
                    core_objectives.push(Box::new(TotalTransportCost::default()));
                }
                Objective::MinimizeTours { goal: _ } => {
                    constraint.add_module(Box::new(FleetUsageConstraintModule::new_minimized()));
                    core_objectives.push(Box::new(TotalRoutes::default()));
                }
                Objective::MinimizeUnassignedJobs { goal: _ } => {
                    core_objectives.push(Box::new(TotalUnassignedJobs::default()));
                }
                Objective::BalanceMaxLoad { threshold: _ } => {
                    add_work_balance_module(constraint, props);
                }
                Objective::BalanceActivities { threshold: _ } => todo!("Balance activities is not yet implemented"),
            });
            (core_objectives, cost_idx)
        };

        let (primary, primary_cost_idx) = map_objectives(&objectives.primary);
        let (secondary, secondary_cost_idx) = map_objectives(&objectives.secondary.clone().unwrap_or_else(|| vec![]));

        MultiObjective::new(
            primary,
            secondary,
            Arc::new(move |primary, secondary| {
                primary_cost_idx
                    .map(|idx| primary.get(idx).unwrap())
                    .or(secondary_cost_idx.map(|idx| secondary.get(idx).unwrap()))
                    .expect("Cannot get cost value objective")
                    .value()
            }),
        )
    } else {
        constraint.add_module(Box::new(FleetUsageConstraintModule::new_minimized()));
        MultiObjective::default()
    }
}

fn create_extras(props: &ProblemProperties, coord_index: CoordIndex) -> Extras {
    let mut extras = Extras::default();
    extras.insert(
        "capacity_type".to_string(),
        Box::new((if props.has_multi_dimen_capacity { "multi" } else { "single" }).to_string()),
    );
    extras.insert("coord_index".to_owned(), Box::new(coord_index));

    extras
}

fn parse_time_window(tw: &Vec<String>) -> TimeWindow {
    assert_eq!(tw.len(), 2);
    TimeWindow::new(parse_time(tw.first().unwrap()), parse_time(tw.last().unwrap()))
}

fn get_problem_properties(api_problem: &ApiProblem, matrices: &Vec<Matrix>) -> ProblemProperties {
    let has_unreachable_locations = matrices.iter().any(|m| m.error_codes.is_some());
    let has_multi_dimen_capacity = api_problem.fleet.vehicles.iter().any(|t| t.capacity.len() > 1)
        || api_problem.plan.jobs.iter().any(|job| {
            job.pickups
                .iter()
                .chain(job.deliveries.iter())
                .flat_map(|tasks| tasks.iter())
                .any(|task| task.demand.as_ref().map_or(false, |d| d.len() > 1))
        });
    let has_breaks = api_problem
        .fleet
        .vehicles
        .iter()
        .flat_map(|t| &t.shifts)
        .any(|shift| shift.breaks.as_ref().map_or(false, |b| b.len() > 0));
    let has_skills = api_problem.plan.jobs.iter().any(|job| job.skills.is_some());
    let has_reload = api_problem
        .fleet
        .vehicles
        .iter()
        .any(|t| t.shifts.iter().any(|s| s.reloads.as_ref().map_or(false, |reloads| !reloads.is_empty())));

    let priority = api_problem.plan.jobs.iter().filter_map(|job| job.priority).any(|priority| priority > 1);

    let priority = if priority {
        api_problem
            .config
            .as_ref()
            .and_then(|c| c.features.as_ref())
            .and_then(|features| features.priority.as_ref().map(|priority| priority.weight_cost))
            .or(Some(100.))
    } else {
        None
    };

    ProblemProperties {
        has_multi_dimen_capacity,
        has_breaks,
        has_skills,
        has_unreachable_locations,
        has_reload,
        priority,
    }
}

fn add_skills(dimens: &mut Dimensions, skills: &Option<Vec<String>>) {
    if let Some(skills) = skills {
        dimens.set_value("skills", HashSet::<String>::from_iter(skills.iter().cloned()));
    }
}
