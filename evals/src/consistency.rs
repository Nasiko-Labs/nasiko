//! Does a model solve a task every time, or only sometimes?
//!
//! A run with several attempts per task answers this. The headline resolved rate counts each
//! task once (its first attempt); the further attempts are used here, as repeat measurements of
//! the same task, never as extra tasks.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::grade::FailureClass;
use crate::report::{AttemptRecord, Rate, rate_of};

/// Fewest attempts a task needs before its consistency can be judged.
pub const MIN_ATTEMPTS: u32 = 2;

/// How steadily one model solved the tasks it attempted more than once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consistency {
    pub model: String,
    /// Fewest and most attempts among the counted tasks; equal in a complete run.
    pub min_attempts: u32,
    pub max_attempts: u32,
    /// Tasks left out because they have a single attempt on record.
    pub single_attempt_tasks: u32,
    /// Share of counted tasks solved in every attempt.
    pub every_time: Rate,
    /// Share of counted tasks solved in at least one attempt.
    pub at_least_once: Rate,
    /// Share of counted tasks solved in some attempts but not all.
    pub sometimes: Rate,
    /// The "sometimes" tasks, sorted by id.
    pub sometimes_tasks: Vec<String>,
    /// Counted tasks with an attempt that failed on the sandbox, not on the model's work
    /// ([`FailureClass::InfraError`]), sorted by id. Such an attempt counts as not solved, so a
    /// flaky sandbox can make a task read as "sometimes" or as never solved every time.
    pub sandbox_failed_tasks: Vec<String>,
}

/// One entry per model that has at least one task with [`MIN_ATTEMPTS`] attempts, sorted by
/// model name. A run with one attempt per task gives an empty list: nothing can be said.
pub fn per_model(records: &[AttemptRecord]) -> Vec<Consistency> {
    let mut by_model: BTreeMap<&str, TaskAttempts> = BTreeMap::new();
    for r in records {
        by_model
            .entry(r.model.as_str())
            .or_default()
            .entry(r.instance_id.as_str())
            .or_default()
            .push(r);
    }
    by_model
        .into_iter()
        .filter_map(|(model, tasks)| of_model(model, &tasks))
        .collect()
}

/// One model's attempts, by task id.
type TaskAttempts<'a> = BTreeMap<&'a str, Vec<&'a AttemptRecord>>;

fn of_model(model: &str, tasks: &TaskAttempts) -> Option<Consistency> {
    let repeated: Vec<(&str, &Vec<&AttemptRecord>)> = tasks
        .iter()
        .filter(|(_, attempts)| attempts.len() as u32 >= MIN_ATTEMPTS)
        .map(|(id, attempts)| (*id, attempts))
        .collect();
    let min_attempts = repeated.iter().map(|(_, a)| a.len() as u32).min()?;
    let max_attempts = repeated.iter().map(|(_, a)| a.len() as u32).max()?;
    let counted = repeated.len() as u32;
    let tasks_where = |holds: fn(&[&AttemptRecord]) -> bool| -> Vec<String> {
        repeated
            .iter()
            .filter(|(_, attempts)| holds(attempts))
            .map(|(id, _)| id.to_string())
            .collect()
    };
    let every_time = tasks_where(|a| a.iter().all(|r| r.grade.resolved));
    let at_least_once = tasks_where(|a| a.iter().any(|r| r.grade.resolved));
    let sometimes_tasks =
        tasks_where(|a| a.iter().any(|r| r.grade.resolved) && !a.iter().all(|r| r.grade.resolved));
    let sandbox_failed_tasks = tasks_where(|a| {
        a.iter()
            .any(|r| r.grade.failure == Some(FailureClass::InfraError))
    });
    Some(Consistency {
        model: model.to_string(),
        min_attempts,
        max_attempts,
        single_attempt_tasks: tasks.len() as u32 - counted,
        every_time: rate_of(every_time.len() as u32, counted),
        at_least_once: rate_of(at_least_once.len() as u32, counted),
        sometimes: rate_of(sometimes_tasks.len() as u32, counted),
        sometimes_tasks,
        sandbox_failed_tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::fixtures::{infra_failed, nth};

    fn three_attempts(id: &str, model: &str, outcomes: [bool; 3]) -> Vec<AttemptRecord> {
        outcomes
            .iter()
            .enumerate()
            .map(|(i, resolved)| nth(id, model, i as u32 + 1, *resolved))
            .collect()
    }

    #[test]
    fn tasks_are_sorted_into_every_time_sometimes_and_never() {
        let mut records = three_attempts("always", "flint", [true, true, true]);
        records.extend(three_attempts("flaky", "flint", [true, false, true]));
        records.extend(three_attempts("late", "flint", [false, false, true]));
        records.extend(three_attempts("never", "flint", [false, false, false]));
        let all = per_model(&records);
        let [flint] = all.as_slice() else {
            panic!("one model expected, got {all:?}");
        };
        assert_eq!((flint.min_attempts, flint.max_attempts), (3, 3));
        assert_eq!(flint.every_time, rate_of(1, 4));
        assert_eq!(flint.at_least_once, rate_of(3, 4));
        assert_eq!(flint.sometimes, rate_of(2, 4));
        assert_eq!(flint.sometimes_tasks, vec!["flaky", "late"]);
        assert_eq!(flint.single_attempt_tasks, 0);
        assert!(flint.sandbox_failed_tasks.is_empty());
    }

    #[test]
    fn a_task_with_an_attempt_lost_to_the_sandbox_is_named() {
        let records = vec![
            infra_failed("a", "flint", 1),
            nth("a", "flint", 2, true),
            nth("a", "flint", 3, true),
            nth("b", "flint", 1, true),
            nth("b", "flint", 2, false),
        ];
        let flint = &per_model(&records)[0];
        assert_eq!(flint.sometimes_tasks, vec!["a", "b"]);
        assert_eq!(flint.sandbox_failed_tasks, vec!["a"]);
    }

    #[test]
    fn one_attempt_per_task_says_nothing() {
        let records = vec![nth("a", "flint", 1, true), nth("b", "flint", 1, false)];
        assert!(per_model(&records).is_empty());
    }

    #[test]
    fn a_task_with_one_attempt_is_left_out_and_counted_as_such() {
        let mut records = three_attempts("a", "flint", [true, true, true]);
        records.push(nth("b", "flint", 1, false));
        records.push(nth("c", "flint", 1, true));
        records.push(nth("c", "flint", 2, true));
        records.push(nth("a", "claude", 1, true));
        let all = per_model(&records);
        assert_eq!(all.len(), 1, "claude has no repeated task");
        let flint = &all[0];
        assert_eq!(flint.every_time, rate_of(2, 2));
        assert_eq!((flint.min_attempts, flint.max_attempts), (2, 3));
        assert_eq!(flint.single_attempt_tasks, 1);
        assert!(flint.sometimes_tasks.is_empty());
    }
}
