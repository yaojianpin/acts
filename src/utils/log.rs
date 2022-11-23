use crate::{utils, Step, TaskState, Workflow};
use colored::Colorize;
use std::io::{Result, Write};

pub fn tree(workflow: &Workflow) -> Result<()> {
    let mut buffer = Vec::new();
    let mut levels: Vec<bool> = Vec::new();
    let (start, end, elapsed) = utils::fmt_timestamp(
        workflow.start_time(),
        workflow.end_time(),
        "%Y-%m-%d %H:%M:%S",
    );
    writeln!(&mut buffer, "workflow: {}", workflow.name)?;
    writeln!(&mut buffer, "version: {}", workflow.ver)?;
    writeln!(&mut buffer, "state: {}", colered_state(workflow.state()))?;
    writeln!(&mut buffer, "time: {} - {}", start, end)?;
    writeln!(&mut buffer, "elapsed: {}ms", elapsed)?;

    for (i, job) in workflow.jobs.iter().map(|job| job.clone()).enumerate() {
        levels.push(i == workflow.jobs.len() - 1);
        let (.., elapsed) =
            utils::fmt_timestamp(job.start_time(), job.end_time(), "%Y-%m-%d %H:%M:%S");

        let line = print_line(
            i,
            workflow.jobs.len(),
            &mut levels,
            format!(
                "{}",
                &format!(
                    "job:{}  result: {}  elapsed:{}ms",
                    job.name,
                    colered_state(job.state()),
                    elapsed,
                )
            ),
        );
        writeln!(&mut buffer, "{}", line)?;
        let step_count = job.steps.len();
        for (j, step) in job.steps.iter().enumerate() {
            let (.., elapsed) =
                utils::fmt_timestamp(step.start_time(), step.end_time(), "%Y-%m-%d %H:%M:%S");
            levels.push(j == job.steps.len() - 1);

            let line = print_line(
                j,
                step_count,
                &mut levels,
                format!(
                    "step:{}({})  result: {} elapsed:{}ms",
                    step.name,
                    step.id,
                    colered_state(step.state()),
                    elapsed,
                ),
            );
            writeln!(&mut buffer, "{}", line)?;
            output_step(step, &mut levels, &mut buffer)?;
            levels.pop();
        }
        levels.pop();
    }

    println!("{}", String::from_utf8(buffer).unwrap());
    Ok(())
}

fn print_line(i: usize, count: usize, levels: &mut Vec<bool>, text: String) -> String {
    let mut line = String::new();
    let items = &levels[0..levels.len() - 1];
    for is_last in items {
        if *is_last {
            line.push_str("   ");
        } else {
            line.push_str("|  ");
        }
    }
    if i == count - 1 {
        line.push_str("|_");
    } else {
        line.push_str("|-");
    }
    line.push_str(&text);
    line
}

fn output_step(step: &Step, levels: &mut Vec<bool>, buffer: &mut Vec<u8>) -> Result<()> {
    let branch_count = step.branches.len();

    let acts = step.acts.read().unwrap();
    for (i, act) in acts.iter().enumerate() {
        levels.push(i == acts.len() - 1);

        let (.., elapsed) =
            utils::fmt_timestamp(act.start_time(), act.end_time(), "%Y-%m-%d %H:%M:%S");

        let line = print_line(
            i,
            acts.len(),
            levels,
            format!(
                "act: {}  result: {} owner: {} user: {} elapsed:{}ms",
                act.id,
                colered_state(act.state()),
                act.owner,
                match act.user() {
                    Some(u) => u,
                    None => "none".to_string(),
                },
                elapsed,
            ),
        );
        writeln!(buffer, "{}", line)?;
        levels.pop();
    }

    for (i, branch) in step.branches.iter().enumerate() {
        let (.., elapsed) =
            utils::fmt_timestamp(branch.start_time(), branch.end_time(), "%Y-%m-%d %H:%M:%S");
        levels.push(i == step.branches.len() - 1);
        let line = print_line(
            i,
            branch_count,
            levels,
            format!(
                "branch: {}  result: {} elapsed:{}ms",
                branch.id,
                colered_state(branch.state()),
                elapsed,
            ),
        );
        writeln!(buffer, "{}", line)?;
        let count = branch.steps.len();
        for (j, step) in branch.steps.iter().enumerate() {
            let (.., elapsed) =
                utils::fmt_timestamp(step.start_time(), step.end_time(), "%Y-%m-%d %H:%M:%S");
            levels.push(j == branch.steps.len() - 1);
            let line = print_line(
                j,
                count,
                levels,
                format!(
                    "step: {}({})  result: {} elapsed:{}ms",
                    step.name,
                    step.id,
                    colered_state(step.state()),
                    elapsed,
                ),
            );
            writeln!(buffer, "{}", line)?;
            output_step(step, levels, buffer)?;
            levels.pop();
        }

        levels.pop();
    }

    Ok(())
}

fn colered_state(state: TaskState) -> String {
    let s: String = state.clone().into();
    let colored_value = match state {
        TaskState::None | TaskState::Skip | TaskState::WaitingEvent | TaskState::Pending => {
            s.yellow()
        }
        TaskState::Running => s.blue(),
        TaskState::Fail(..) | TaskState::Abort(..) => s.red(),
        TaskState::Success => s.green(),
    };

    colored_value.to_string()
}
