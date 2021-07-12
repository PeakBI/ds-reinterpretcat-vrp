use crate::utils::compare_floats;
use std::cmp::Ordering;
use std::sync::Arc;

/// An *objective* defines a *total ordering relation* and a *distance metric* on a set of
/// `solutions`. Given any two solutions, an objective answers the following two questions:
///
/// - "which solution is the better one" (total order)
/// - "how similar are the two solutions" (distance metric)
pub trait Objective {
    /// The solution value type that we define the objective on.
    type Solution;

    /// An objective defines a total ordering between any two solution values.
    ///
    /// This answers the question, is solution `a` better, equal or worse than solution `b`,
    /// according to the objective.
    fn total_order(&self, a: &Self::Solution, b: &Self::Solution) -> Ordering {
        let fitness_a = self.fitness(a);
        let fitness_b = self.fitness(b);

        compare_floats(fitness_a, fitness_b)
    }

    /// An objective defines a distance metric between any two solution values.
    ///
    /// The distance metric answer the question, how similar the solutions `a` and `b` are,
    /// according to the objective. A zero value would mean, that both solutions are in fact the same,
    /// according to the objective. Larger magnitudes would mean "less similar".
    fn distance(&self, a: &Self::Solution, b: &Self::Solution) -> f64 {
        let fitness_a = self.fitness(a);
        let fitness_b = self.fitness(b);

        fitness_a - fitness_b
    }

    /// An objective fitness value for given `solution`.
    fn fitness(&self, solution: &Self::Solution) -> f64;
}

/// A multi objective.
pub trait MultiObjective: Objective {
    /// Returns collection of objective functions.
    fn objectives<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &Arc<dyn Objective<Solution = Self::Solution> + Send + Sync>> + 'a>;
}

/// Calculates dominance order of two solutions using multiple objectives.
pub fn dominance_order<S>(a: &S, b: &S, objectives: &[Arc<dyn Objective<Solution = S> + Send + Sync>]) -> Ordering {
    let mut less_cnt = 0;
    let mut greater_cnt = 0;

    for objective in objectives.iter() {
        match objective.total_order(a, b) {
            Ordering::Less => {
                less_cnt += 1;
            }
            Ordering::Greater => {
                greater_cnt += 1;
            }
            Ordering::Equal => {}
        }
    }

    if less_cnt > 0 && greater_cnt == 0 {
        Ordering::Less
    } else if greater_cnt > 0 && less_cnt == 0 {
        Ordering::Greater
    } else {
        debug_assert!((less_cnt > 0 && greater_cnt > 0) || (less_cnt == 0 && greater_cnt == 0));
        Ordering::Equal
    }
}
