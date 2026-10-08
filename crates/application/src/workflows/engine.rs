//! Pure run planner. Given a graph and the current state of every node
//! execution it decides which executions are ready, which must be skipped
//! (an untaken branch, or no data), and whether the run is finished.
//!
//! Repetition: a `logic.for_each` node completes with an item count `n`.
//! Everything after it runs once per item in its own *iteration* (`"0"`,
//! `"1"`, ... nested as `"2/0"`). A `logic.merge` collapses one level and
//! waits for every iteration to settle. The planner is re-run after every
//! node execution and is idempotent.

use std::collections::{HashMap, HashSet};

use super::{model::WorkflowGraph, validate::duration_seconds};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRunState {
    Queued,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl NodeRunState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRunSnapshot {
    pub node_id: String,
    pub iteration: String,
    pub state: NodeRunState,
    /// Output ports that carry data (completed runs only).
    pub fired_ports: Vec<String>,
    /// Number of items a `for_each` run fanned out to.
    pub items: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedNode {
    pub node_id: String,
    pub iteration: String,
    /// Seconds to wait before running (the wait block).
    pub delay_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanOutcome {
    pub ready: Vec<PlannedNode>,
    /// Executions that will never run: `(node, iteration, reason)`.
    pub skipped: Vec<(String, String, String)>,
    /// Nothing is running, queued or ready any more.
    pub finished: bool,
    /// At least one execution failed or was cancelled.
    pub failed: bool,
}

pub fn child_iteration(parent: &str, index: usize) -> String {
    if parent.is_empty() {
        index.to_string()
    } else {
        format!("{parent}/{index}")
    }
}

fn depth_of(iteration: &str) -> usize {
    if iteration.is_empty() {
        0
    } else {
        iteration.split('/').count()
    }
}

fn prefix(iteration: &str, depth: usize) -> String {
    iteration
        .split('/')
        .take(depth)
        .collect::<Vec<_>>()
        .join("/")
}

fn parent(iteration: &str) -> String {
    prefix(iteration, depth_of(iteration).saturating_sub(1))
}

fn is_under(iteration: &str, ancestor: &str) -> bool {
    ancestor.is_empty() || iteration == ancestor || iteration.starts_with(&format!("{ancestor}/"))
}

/// Topological order; `None` when the graph has a cycle.
fn topo_order(graph: &WorkflowGraph) -> Option<Vec<&str>> {
    let mut indegree: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0))
        .collect();
    for edge in &graph.edges {
        if let Some(degree) = indegree.get_mut(edge.to.node.as_str()) {
            *degree += 1;
        }
    }
    let mut order = Vec::new();
    let mut ready: Vec<&str> = graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .filter(|id| indegree.get(id) == Some(&0))
        .collect();
    while let Some(current) = ready.pop() {
        order.push(current);
        for edge in graph.outgoing(current) {
            if let Some(degree) = indegree.get_mut(edge.to.node.as_str()) {
                *degree -= 1;
                if *degree == 0 {
                    ready.push(edge.to.node.as_str());
                }
            }
        }
    }
    (order.len() == graph.nodes.len()).then_some(order)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeState {
    Waiting,
    Live,
    Dead,
}

struct Planner<'a> {
    graph: &'a WorkflowGraph,
    order: Vec<&'a str>,
    depth: HashMap<&'a str, usize>,
}

impl<'a> Planner<'a> {
    fn new(graph: &'a WorkflowGraph) -> Option<Self> {
        let order = topo_order(graph)?;
        let mut depth: HashMap<&str, usize> = HashMap::new();
        for id in &order {
            let node_is_merge = graph
                .node(id)
                .is_some_and(|node| node.node_type == "logic.merge");
            let mut best = 0;
            for edge in graph.incoming(id) {
                let source = edge.from.node.as_str();
                let source_depth = depth.get(source).copied().unwrap_or(0);
                let source_is_loop = graph
                    .node(source)
                    .is_some_and(|node| node.node_type == "logic.for_each");
                let contribution = if source_is_loop {
                    source_depth + 1
                } else if node_is_merge {
                    source_depth.saturating_sub(1)
                } else {
                    source_depth
                };
                best = best.max(contribution);
            }
            depth.insert(id, best);
        }
        Some(Self {
            graph,
            order,
            depth,
        })
    }

    fn depth(&self, id: &str) -> usize {
        self.depth.get(id).copied().unwrap_or(0)
    }

    /// Iterations in which `node` executes. `None` while an upstream loop has
    /// not finished and so the fan-out is unknown.
    fn contexts(
        &self,
        runs: &HashMap<(&str, &str), &NodeRunSnapshot>,
        by_node: &HashMap<&str, Vec<&NodeRunSnapshot>>,
        known: &mut HashMap<String, Option<Vec<String>>>,
        node_id: &str,
    ) -> Option<Vec<String>> {
        if let Some(cached) = known.get(node_id) {
            return cached.clone();
        }
        let Some(node) = self.graph.node(node_id) else {
            return Some(Vec::new());
        };
        let result = if node.node_type.starts_with("trigger.") {
            Some(vec![String::new()])
        } else {
            let node_depth = self.depth(node_id);
            let is_merge = node.node_type == "logic.merge";
            let mut result: Option<Option<Vec<String>>> = None;
            for edge in self.graph.incoming(node_id) {
                let source = edge.from.node.as_str();
                let source_node = self.graph.node(source);
                let source_is_loop =
                    source_node.is_some_and(|node| node.node_type == "logic.for_each");
                let source_depth = self.depth(source);
                let contribution = if source_is_loop {
                    source_depth + 1
                } else if is_merge {
                    source_depth.saturating_sub(1)
                } else {
                    source_depth
                };
                if contribution != node_depth {
                    continue;
                }
                let source_ctx = self.contexts(runs, by_node, known, source);
                let candidate = source_ctx.map(|source_ctx| {
                    if source_is_loop {
                        let mut children = Vec::new();
                        let mut complete = true;
                        for context in &source_ctx {
                            match runs.get(&(source, context.as_str())) {
                                Some(run) if run.state.is_terminal() => {
                                    for index in 0..run.items {
                                        children.push(child_iteration(context, index));
                                    }
                                }
                                _ => complete = false,
                            }
                        }
                        complete.then_some(children)
                    } else if is_merge {
                        let mut parents: Vec<String> = Vec::new();
                        for context in &source_ctx {
                            let p = parent(context);
                            if !parents.contains(&p) {
                                parents.push(p);
                            }
                        }
                        Some(parents)
                    } else {
                        Some(source_ctx)
                    }
                });
                result = Some(candidate.flatten());
                break;
            }
            let _ = by_node;
            result.unwrap_or(Some(vec![String::new()]))
        };
        known.insert(node_id.to_owned(), result.clone());
        result
    }
}

fn edge_state(
    planner: &Planner<'_>,
    runs: &HashMap<(&str, &str), &NodeRunSnapshot>,
    contexts: &mut HashMap<String, Option<Vec<String>>>,
    by_node: &HashMap<&str, Vec<&NodeRunSnapshot>>,
    edge: &super::model::GraphEdge,
    target: &str,
    iteration: &str,
) -> EdgeState {
    let source = edge.from.node.as_str();
    let source_depth = planner.depth(source);
    let expected: Vec<String> = if source_depth > planner.depth(target) {
        match planner.contexts(runs, by_node, contexts, source) {
            Some(all) => all.into_iter().filter(|c| is_under(c, iteration)).collect(),
            None => return EdgeState::Waiting,
        }
    } else {
        vec![prefix(iteration, source_depth)]
    };
    if expected.is_empty() {
        return EdgeState::Dead;
    }
    let mut any_live = false;
    for context in &expected {
        match runs.get(&(source, context.as_str())) {
            Some(run) => match run.state {
                NodeRunState::Completed => {
                    if run.fired_ports.iter().any(|port| port == &edge.from.port) {
                        any_live = true;
                    }
                }
                NodeRunState::Skipped | NodeRunState::Failed | NodeRunState::Cancelled => {}
                NodeRunState::Queued | NodeRunState::Running => return EdgeState::Waiting,
            },
            None => return EdgeState::Waiting,
        }
    }
    if any_live {
        EdgeState::Live
    } else {
        EdgeState::Dead
    }
}

/// Where one input of a node execution gets its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputSource {
    pub port: String,
    pub source_node: String,
    pub source_port: String,
    /// Iterations of the source whose output feeds this input.
    pub iterations: Vec<String>,
    /// The source is a `for_each`: the value is the element of its list at
    /// this position of the node's iteration key (`None` when not a loop).
    pub loop_index: Option<usize>,
}

/// Resolve the sources of every input of `node_id` running in `iteration`.
pub fn input_sources(
    graph: &WorkflowGraph,
    snapshots: &[NodeRunSnapshot],
    node_id: &str,
    iteration: &str,
) -> Vec<InputSource> {
    let Some(planner) = Planner::new(graph) else {
        return Vec::new();
    };
    let runs: HashMap<(&str, &str), &NodeRunSnapshot> = snapshots
        .iter()
        .map(|run| ((run.node_id.as_str(), run.iteration.as_str()), run))
        .collect();
    let by_node: HashMap<&str, Vec<&NodeRunSnapshot>> = HashMap::new();
    let mut contexts: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let node_depth = planner.depth(node_id);
    graph
        .incoming(node_id)
        .map(|edge| {
            let source = edge.from.node.as_str();
            let source_depth = planner.depth(source);
            let is_loop = graph
                .node(source)
                .is_some_and(|node| node.node_type == "logic.for_each");
            let iterations = if source_depth > node_depth {
                planner
                    .contexts(&runs, &by_node, &mut contexts, source)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|context| is_under(context, iteration))
                    .collect()
            } else {
                vec![prefix(iteration, source_depth)]
            };
            let loop_index = if is_loop {
                iteration
                    .split('/')
                    .nth(source_depth)
                    .and_then(|segment| segment.parse().ok())
            } else {
                None
            };
            InputSource {
                port: edge.to.port.clone(),
                source_node: source.to_owned(),
                source_port: edge.from.port.clone(),
                iterations,
                loop_index,
            }
        })
        .collect()
}

/// Plan the next executions for a run.
pub fn plan_run(graph: &WorkflowGraph, snapshots: &[NodeRunSnapshot]) -> PlanOutcome {
    let Some(planner) = Planner::new(graph) else {
        return PlanOutcome {
            finished: true,
            failed: true,
            ..PlanOutcome::default()
        };
    };
    let mut working: Vec<NodeRunSnapshot> = snapshots.to_vec();
    let mut outcome = PlanOutcome::default();
    let failed_already = working
        .iter()
        .any(|run| matches!(run.state, NodeRunState::Failed | NodeRunState::Cancelled));
    outcome.failed = failed_already;

    if !failed_already {
        loop {
            let runs: HashMap<(&str, &str), &NodeRunSnapshot> = working
                .iter()
                .map(|run| ((run.node_id.as_str(), run.iteration.as_str()), run))
                .collect();
            let mut by_node: HashMap<&str, Vec<&NodeRunSnapshot>> = HashMap::new();
            for run in &working {
                by_node.entry(run.node_id.as_str()).or_default().push(run);
            }
            let mut contexts: HashMap<String, Option<Vec<String>>> = HashMap::new();
            let mut new_skips: Vec<(String, String, String)> = Vec::new();
            let mut new_ready: Vec<PlannedNode> = Vec::new();
            let mut seen_ready: HashSet<(String, String)> = HashSet::new();

            for node_id in &planner.order {
                let Some(node) = graph.node(node_id) else {
                    continue;
                };
                if node.node_type.starts_with("trigger.") {
                    continue;
                }
                let Some(node_contexts) = planner.contexts(&runs, &by_node, &mut contexts, node_id)
                else {
                    continue;
                };
                let is_merge = node.node_type == "logic.merge";
                let def = super::catalog::node_type(&node.node_type);
                for iteration in node_contexts {
                    if runs.contains_key(&(*node_id, iteration.as_str())) {
                        continue;
                    }
                    let mut waiting = false;
                    let mut live_ports: HashSet<&str> = HashSet::new();
                    let mut any_live = false;
                    for edge in graph.incoming(node_id) {
                        match edge_state(
                            &planner,
                            &runs,
                            &mut contexts,
                            &by_node,
                            edge,
                            node_id,
                            &iteration,
                        ) {
                            EdgeState::Waiting => waiting = true,
                            EdgeState::Live => {
                                any_live = true;
                                live_ports.insert(edge.to.port.as_str());
                            }
                            EdgeState::Dead => {}
                        }
                    }
                    if waiting {
                        continue;
                    }
                    let satisfied = if is_merge {
                        any_live
                    } else {
                        def.is_none_or(|def| {
                            def.inputs
                                .iter()
                                .filter(|port| port.required)
                                .all(|port| live_ports.contains(port.id.as_str()))
                        })
                    };
                    if satisfied {
                        if seen_ready.insert(((*node_id).to_owned(), iteration.clone())) {
                            let delay_secs = if node.node_type == "logic.wait" {
                                node.config
                                    .get("duration")
                                    .and_then(duration_seconds)
                                    .unwrap_or(0)
                            } else {
                                0
                            };
                            new_ready.push(PlannedNode {
                                node_id: (*node_id).to_owned(),
                                iteration,
                                delay_secs,
                            });
                        }
                    } else {
                        new_skips.push((
                            (*node_id).to_owned(),
                            iteration,
                            "Nothing reached this step.".to_owned(),
                        ));
                    }
                }
            }

            if new_skips.is_empty() {
                outcome.ready = new_ready;
                break;
            }
            for (node_id, iteration, reason) in new_skips {
                working.push(NodeRunSnapshot {
                    node_id: node_id.clone(),
                    iteration: iteration.clone(),
                    state: NodeRunState::Skipped,
                    fired_ports: Vec::new(),
                    items: 0,
                });
                outcome.skipped.push((node_id, iteration, reason));
            }
        }
    }

    let active = working
        .iter()
        .any(|run| matches!(run.state, NodeRunState::Queued | NodeRunState::Running));
    outcome.finished = !active && outcome.ready.is_empty();
    outcome
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::workflows::{
        model::{GraphEdge, GraphNode, WorkflowGraph},
        validate::tests::{edge, node},
    };

    fn run(
        node: &str,
        iteration: &str,
        state: NodeRunState,
        fired: &[&str],
        items: usize,
    ) -> NodeRunSnapshot {
        NodeRunSnapshot {
            node_id: node.to_owned(),
            iteration: iteration.to_owned(),
            state,
            fired_ports: fired.iter().map(|port| (*port).to_owned()).collect(),
            items,
        }
    }

    fn graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> WorkflowGraph {
        WorkflowGraph { nodes, edges }
    }

    fn ready_ids(outcome: &PlanOutcome) -> Vec<(String, String)> {
        let mut ids: Vec<_> = outcome
            .ready
            .iter()
            .map(|planned| (planned.node_id.clone(), planned.iteration.clone()))
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn linear_flow_advances_one_step_at_a_time() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node(
                    "w",
                    "logic.wait",
                    json!({"duration": {"value": 2, "unit": "minutes"}}),
                ),
                node("n", "integration.notify", json!({"title": "x"})),
            ],
            vec![
                edge("t", "data", "w", "data"),
                edge("w", "data", "n", "data"),
            ],
        );
        let start = [run("t", "", NodeRunState::Completed, &["data"], 0)];
        let plan = plan_run(&g, &start);
        assert_eq!(ready_ids(&plan), vec![("w".into(), "".into())]);
        assert_eq!(plan.ready[0].delay_secs, 120);
        assert!(!plan.finished);

        let running = [
            start[0].clone(),
            run("w", "", NodeRunState::Running, &[], 0),
        ];
        assert!(plan_run(&g, &running).ready.is_empty());
        assert!(!plan_run(&g, &running).finished);

        let done = [
            start[0].clone(),
            run("w", "", NodeRunState::Completed, &["data"], 0),
            run("n", "", NodeRunState::Completed, &["data"], 0),
        ];
        let plan = plan_run(&g, &done);
        assert!(plan.finished && !plan.failed);
    }

    #[test]
    fn untaken_branch_is_skipped_and_propagates() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("if", "logic.if", json!({"condition": {"rules": []}})),
                node("yes", "integration.notify", json!({"title": "y"})),
                node("no", "integration.notify", json!({"title": "n"})),
                node(
                    "after_no",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "minutes"}}),
                ),
            ],
            vec![
                edge("t", "data", "if", "data"),
                edge("if", "yes", "yes", "data"),
                edge("if", "no", "no", "data"),
                edge("no", "data", "after_no", "data"),
            ],
        );
        let runs = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("if", "", NodeRunState::Completed, &["yes"], 0),
        ];
        let plan = plan_run(&g, &runs);
        assert_eq!(ready_ids(&plan), vec![("yes".into(), "".into())]);
        let skipped: Vec<_> = plan.skipped.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(skipped.contains(&"no") && skipped.contains(&"after_no"));
    }

    #[test]
    fn for_each_fans_out_and_merge_waits_for_every_iteration() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("q", "data.find_records", json!({})),
                node("each", "logic.for_each", json!({})),
                node(
                    "w",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "seconds"}}),
                ),
                node("m", "logic.merge", json!({})),
                node("n", "integration.notify", json!({"title": "done"})),
            ],
            vec![
                edge("t", "data", "q", "start"),
                edge("q", "records", "each", "items"),
                edge("each", "item", "w", "data"),
                edge("w", "data", "m", "inputs"),
                edge("m", "merged", "n", "data"),
            ],
        );
        let mut runs = vec![
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("q", "", NodeRunState::Completed, &["records"], 0),
            run("each", "", NodeRunState::Completed, &["item"], 2),
        ];
        let plan = plan_run(&g, &runs);
        assert_eq!(
            ready_ids(&plan),
            vec![("w".into(), "0".into()), ("w".into(), "1".into())]
        );

        runs.push(run("w", "0", NodeRunState::Completed, &["data"], 0));
        runs.push(run("w", "1", NodeRunState::Running, &[], 0));
        assert!(plan_run(&g, &runs).ready.is_empty());

        runs[4] = run("w", "1", NodeRunState::Completed, &["data"], 0);
        let plan = plan_run(&g, &runs);
        assert_eq!(ready_ids(&plan), vec![("m".into(), "".into())]);

        runs.push(run("m", "", NodeRunState::Completed, &["merged"], 0));
        let plan = plan_run(&g, &runs);
        assert_eq!(ready_ids(&plan), vec![("n".into(), "".into())]);
    }

    #[test]
    fn a_failure_stops_further_planning() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node(
                    "a",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "seconds"}}),
                ),
                node(
                    "b",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "seconds"}}),
                ),
            ],
            vec![
                edge("t", "data", "a", "data"),
                edge("t", "data", "b", "data"),
            ],
        );
        let runs = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("a", "", NodeRunState::Failed, &[], 0),
        ];
        let plan = plan_run(&g, &runs);
        assert!(plan.failed && plan.finished && plan.ready.is_empty());
    }

    fn screening_graph() -> WorkflowGraph {
        graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("s", "ai.run_review", json!({"task": "screening"})),
                node("each", "logic.for_each", json!({})),
                node("n", "integration.notify", json!({"title": "x"})),
                node("all", "integration.notify", json!({"title": "all"})),
            ],
            vec![
                edge("t", "data", "s", "records"),
                edge("s", "included", "each", "items"),
                edge("each", "item", "n", "data"),
                edge("s", "records", "all", "data"),
            ],
        )
    }

    #[test]
    fn a_waiting_screen_holds_every_branch_until_the_verdicts_are_in() {
        let g = screening_graph();
        let waiting = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("s", "", NodeRunState::Queued, &[], 0),
        ];
        let plan = plan_run(&g, &waiting);
        assert!(plan.ready.is_empty());
        assert!(plan.skipped.is_empty());
        assert!(!plan.finished);

        let running = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("s", "", NodeRunState::Running, &[], 0),
        ];
        assert!(plan_run(&g, &running).ready.is_empty());
    }

    #[test]
    fn verdict_ports_route_only_the_records_that_left_on_them() {
        let g = screening_graph();
        // Records with a verdict of "would include" only.
        let included = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run(
                "s",
                "",
                NodeRunState::Completed,
                &["records", "included"],
                0,
            ),
        ];
        let plan = plan_run(&g, &included);
        assert_eq!(
            ready_ids(&plan),
            vec![("all".into(), "".into()), ("each".into(), "".into())]
        );
        assert!(plan.skipped.is_empty());

        // Nothing was included: the loop and its notification are skipped, the
        // all-records branch still runs.
        let none_included = [
            run("t", "", NodeRunState::Completed, &["data"], 0),
            run("s", "", NodeRunState::Completed, &["records", "unsure"], 0),
        ];
        let plan = plan_run(&g, &none_included);
        assert_eq!(ready_ids(&plan), vec![("all".into(), "".into())]);
        let skipped: Vec<_> = plan.skipped.iter().map(|(id, _, _)| id.as_str()).collect();
        assert!(skipped.contains(&"each"), "{skipped:?}");
    }

    #[test]
    fn a_run_that_only_passes_records_on_still_feeds_the_all_records_port() {
        let g = screening_graph();
        // Test runs and non-screening tasks fire only the records port.
        let plan = plan_run(
            &g,
            &[
                run("t", "", NodeRunState::Completed, &["data"], 0),
                run("s", "", NodeRunState::Completed, &["records"], 0),
            ],
        );
        assert_eq!(ready_ids(&plan), vec![("all".into(), "".into())]);
        assert!(plan.skipped.iter().any(|(id, _, _)| id == "each"));
        // The loop fans out to nothing, so its notification never gets a run.
        assert!(ready_ids(&plan).iter().all(|(id, _)| id != "n"));
    }
}
