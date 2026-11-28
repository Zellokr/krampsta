//! `update_plan`: the model's plan for a task of several steps, shown to the user as a checklist
//! in the chat. The tool keeps nothing: each call carries the whole plan, and its card draws it
//! from the call's arguments ([`Plan::parse`]), so a plan from saved history shows the same.

use serde_json::{Value, json};

use crate::ToolSpec;

/// The tool's name.
pub const TOOL: &str = "update_plan";
/// The most steps a plan takes.
pub const MAX_STEPS: usize = 20;
/// The longest a step's text may be, in characters.
pub const MAX_STEP_CHARS: usize = 200;
/// Arguments longer than this aren't a plan (and aren't parsed for its card).
pub const MAX_ARGUMENTS: usize = 8 * 1024;

/// Where a step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub status: StepStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    /// What changed and why, when the model says.
    pub note: Option<String>,
}

impl Plan {
    /// The plan in a call's arguments, or why they aren't one (for the model to fix).
    pub fn parse(arguments: &str) -> Result<Plan, String> {
        if arguments.len() > MAX_ARGUMENTS {
            return Err(format!("the plan is too long (over {MAX_ARGUMENTS} bytes); keep steps short"));
        }
        let value: Value = serde_json::from_str(arguments).map_err(|e| format!("the arguments aren't JSON ({e})"))?;
        let steps = value["steps"].as_array().ok_or("`steps` must be a list of steps")?;
        if steps.is_empty() || steps.len() > MAX_STEPS {
            return Err(format!("a plan has 1 to {MAX_STEPS} steps"));
        }
        let steps = steps
            .iter()
            .map(|s| {
                let text = s["step"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or("each step needs a `step` text")?;
                if text.chars().count() > MAX_STEP_CHARS {
                    return Err(format!("a step's text is at most {MAX_STEP_CHARS} characters"));
                }
                let status = match s["status"].as_str() {
                    Some("pending") => StepStatus::Pending,
                    Some("in_progress") => StepStatus::InProgress,
                    Some("completed") => StepStatus::Completed,
                    _ => return Err("each step's `status` is pending, in_progress or completed".to_owned()),
                };
                Ok(PlanStep { text: text.to_owned(), status })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if steps.iter().filter(|s| s.status == StepStatus::InProgress).count() > 1 {
            return Err("only one step can be in_progress at a time".into());
        }
        let note = value["note"].as_str().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
        Ok(Plan { steps, note })
    }

    pub fn completed(&self) -> usize {
        self.steps.iter().filter(|s| s.status == StepStatus::Completed).count()
    }

    pub fn current(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.status == StepStatus::InProgress)
    }
}

/// Runs `update_plan`: checks the plan and tells the model where it stands.
pub fn run(arguments: &str) -> Result<String, String> {
    let plan = Plan::parse(arguments)?;
    let (done, total) = (plan.completed(), plan.steps.len());
    Ok(match plan.current() {
        Some(step) => format!("Plan updated: {done} of {total} steps done; now: {}.", step.text),
        None if done == total => format!("Plan updated: all {total} steps done."),
        None => format!("Plan updated: {done} of {total} steps done."),
    })
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: TOOL.into(),
        description: "Keep a short plan for a task of several steps; the user sees it as a checklist. \
            Call it once you know the steps, then again as each one starts or finishes, sending the \
            whole plan each time with exactly one step in_progress while you work. Skip it for \
            simple tasks or questions you can answer in a step or two."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array", "minItems": 1, "maxItems": MAX_STEPS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string", "description": "What the step does, in a few words."},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["step", "status"]
                    }
                },
                "note": {"type": "string", "description": "Optional: what changed in the plan and why."}
            },
            "required": ["steps"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(steps: &[(&str, &str)]) -> String {
        let steps: Vec<Value> = steps.iter().map(|(step, status)| json!({"step": step, "status": status})).collect();
        json!({"steps": steps}).to_string()
    }

    #[test]
    fn a_plan_says_where_it_stands() {
        let a =
            args(&[("Read the parser", "completed"), ("Cache the table", "in_progress"), ("Add a test", "pending")]);
        let plan = Plan::parse(&a).unwrap();
        assert_eq!((plan.completed(), plan.current().map(|s| s.text.as_str())), (1, Some("Cache the table")));
        assert_eq!(run(&a).unwrap(), "Plan updated: 1 of 3 steps done; now: Cache the table.");
        assert_eq!(run(&args(&[("a", "completed")])).unwrap(), "Plan updated: all 1 steps done.");
    }

    #[test]
    fn a_bad_plan_says_what_to_fix() {
        let two_at_once = args(&[("a", "in_progress"), ("b", "in_progress")]);
        assert_eq!(run(&two_at_once).unwrap_err(), "only one step can be in_progress at a time");
        assert!(run(&args(&[("a", "done")])).unwrap_err().contains("pending, in_progress or completed"));
        assert!(run(&args(&[])).unwrap_err().contains("1 to 20 steps"));
        assert!(run(&args(&[("  ", "pending")])).unwrap_err().contains("`step` text"));
        assert!(run("nope").unwrap_err().contains("aren't JSON"));
        let long = "x".repeat(MAX_STEP_CHARS + 1);
        assert!(run(&args(&[(&long, "pending")])).unwrap_err().contains("at most"));
    }

    #[test]
    fn a_note_is_kept_and_blank_ones_dropped() {
        let with = json!({"steps": [{"step": "a", "status": "pending"}], "note": " Split the work. "}).to_string();
        assert_eq!(Plan::parse(&with).unwrap().note.as_deref(), Some("Split the work."));
        let blank = json!({"steps": [{"step": "a", "status": "pending"}], "note": " "}).to_string();
        assert_eq!(Plan::parse(&blank).unwrap().note, None);
    }
}
