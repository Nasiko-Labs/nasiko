use std::collections::{BTreeMap, BTreeSet};

use super::{SelectionConfig, SelectionError, SelectionResult, ToolCandidate, ToolDependencyGraph};

#[derive(Debug, Clone)]
pub struct SelectedTools {
    pub indices: Vec<usize>,
    pub dependency_count: usize,
    pub uncertain_count: usize,
    pub budget_overflow: bool,
}

pub(crate) fn validate_result(
    candidates: &[ToolCandidate],
    result: &SelectionResult,
) -> Result<(), SelectionError> {
    let ids = candidates
        .iter()
        .map(|c| c.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != candidates.len() {
        return Err(SelectionError::InvalidPolicy);
    }
    if result
        .probabilities
        .keys()
        .any(|id| !ids.contains(id.as_str()))
    {
        return Err(SelectionError::UnknownId);
    }
    if result.probabilities.len() != ids.len() {
        return Err(SelectionError::Incomplete);
    }
    if result
        .probabilities
        .values()
        .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(SelectionError::Probability);
    }
    Ok(())
}

/// Explicit dependencies only. Cycles terminate; unknown endpoints reject the graph.
pub fn dependency_closure(
    candidates: &[ToolCandidate],
    seeds: &BTreeSet<usize>,
    graph: &ToolDependencyGraph,
) -> Result<BTreeSet<usize>, SelectionError> {
    let names = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect::<BTreeMap<_, _>>();
    if names.len() != candidates.len() || seeds.iter().any(|&i| i >= candidates.len()) {
        return Err(SelectionError::InvalidPolicy);
    }
    for (name, dependencies) in graph {
        if !names.contains_key(name.as_str())
            || dependencies.iter().any(|d| !names.contains_key(d.as_str()))
        {
            return Err(SelectionError::InvalidPolicy);
        }
    }
    let mut closure = seeds.clone();
    let mut pending = seeds.iter().copied().collect::<Vec<_>>();
    while let Some(index) = pending.pop() {
        if let Some(dependencies) = graph.get(&candidates[index].name) {
            for name in dependencies {
                let dependency = names[name.as_str()];
                if closure.insert(dependency) {
                    pending.push(dependency);
                }
            }
        }
    }
    Ok(closure)
}

pub fn apply_policy(
    candidates: &[ToolCandidate],
    result: &SelectionResult,
    cfg: &SelectionConfig,
) -> Result<SelectedTools, SelectionError> {
    validate_result(candidates, result)?;
    validate_config(candidates, cfg)?;
    let floor = cfg.uncertainty_floor.unwrap_or(cfg.include_threshold);
    let mandatory = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.mandatory)
        .map(|(i, _)| i)
        .collect::<BTreeSet<_>>();
    let mut selected = dependency_closure(candidates, &mandatory, &cfg.dependencies)?;
    let mut admitted = mandatory.clone();
    let cost = |indices: &BTreeSet<usize>| {
        indices.iter().fold(0u64, |sum, &i| {
            sum + u64::from(candidates[i].estimated_tokens)
        })
    };
    let fits = |indices: &BTreeSet<usize>| {
        cfg.max_tools.is_none_or(|cap| indices.len() <= cap)
            && cfg
                .max_tool_tokens
                .is_none_or(|cap| cost(indices) <= u64::from(cap))
    };
    let budget_overflow = !fits(&selected);
    let mut optional = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.mandatory && result.probabilities[&c.id] >= floor)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    // Ranking ties resolve to native order; emitted indices always retain native order.
    optional.sort_by(|&a, &b| {
        result.probabilities[&candidates[b].id]
            .total_cmp(&result.probabilities[&candidates[a].id])
            .then(a.cmp(&b))
    });
    for index in optional {
        if selected.contains(&index) {
            continue;
        }
        let mut proposed = selected.clone();
        proposed.insert(index);
        proposed = dependency_closure(candidates, &proposed, &cfg.dependencies)?;
        // Admit a complete dependency bundle or none of it.
        if fits(&proposed) {
            selected = proposed;
            admitted.insert(index);
        }
    }
    if selected.len() < cfg.min_selected_tools && !candidates.is_empty() {
        return Err(SelectionError::Empty);
    }
    let uncertain_count = selected
        .iter()
        .filter(|&&i| {
            !candidates[i].mandatory
                && result.probabilities[&candidates[i].id] >= floor
                && result.probabilities[&candidates[i].id] < cfg.include_threshold
        })
        .count();
    Ok(SelectedTools {
        dependency_count: selected.len() - admitted.len(),
        indices: selected.into_iter().collect(),
        uncertain_count,
        budget_overflow,
    })
}

pub(crate) fn validate_config(
    candidates: &[ToolCandidate],
    cfg: &SelectionConfig,
) -> Result<(), SelectionError> {
    if !cfg.include_threshold.is_finite()
        || !(0.0..=1.0).contains(&cfg.include_threshold)
        || cfg
            .uncertainty_floor
            .is_some_and(|f| !f.is_finite() || f < 0.0 || f > cfg.include_threshold)
    {
        return Err(SelectionError::InvalidPolicy);
    }
    dependency_closure(candidates, &BTreeSet::new(), &cfg.dependencies)?;
    Ok(())
}
