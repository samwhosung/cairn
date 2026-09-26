use std::collections::BTreeMap;

use crate::journal::{Head, Kind};

/// The journal lines of a step's commands, and the line of its latest undo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub lines: Vec<usize>,
    pub undone_at: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct Stacks {
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_line_was_command: bool,
}

#[derive(Default)]
pub struct Steps(BTreeMap<String, Stacks>);

impl Steps {
    /// The step a journal line starts, joins, takes back or puts back.
    pub fn follow(&mut self, line: usize, head: &Head) -> Result<Step, String> {
        let s = self.0.entry(head.author.clone()).or_default();
        let bad = |what: &str| format!("journal line {line}: {} {what}", head.author);
        match head.kind {
            Kind::Step => {
                s.redo.clear();
                s.undo.push(Step {
                    lines: vec![line],
                    undone_at: None,
                });
                s.last_line_was_command = true;
            }
            Kind::Join => {
                let top = s
                    .undo
                    .last_mut()
                    .filter(|_| s.last_line_was_command)
                    .ok_or_else(|| bad("joins no step"))?;
                top.lines.push(line);
                s.redo.clear();
            }
            Kind::Undo => {
                let mut step = s.undo.pop().ok_or_else(|| bad("has nothing to undo"))?;
                step.undone_at = Some(line);
                s.redo.push(step);
                s.last_line_was_command = false;
            }
            Kind::Redo => {
                let step = s.redo.pop().ok_or_else(|| bad("has nothing to redo"))?;
                s.undo.push(step);
                s.last_line_was_command = false;
            }
        }
        let stepped = match head.kind {
            Kind::Step | Kind::Join | Kind::Redo => s.undo.last(),
            Kind::Undo => s.redo.last(),
        };
        Ok(stepped.cloned().unwrap_or_default())
    }

    pub fn next_undo(&self, author: &str) -> Option<&Step> {
        self.0.get(author)?.undo.last()
    }

    pub fn next_redo(&self, author: &str) -> Option<&Step> {
        self.0.get(author)?.redo.last()
    }

    pub fn joinable(&self, author: &str) -> bool {
        self.0.get(author).is_some_and(|s| s.last_line_was_command)
    }
}
