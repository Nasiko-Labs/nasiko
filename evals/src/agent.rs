//! The replayed agent's container script, and reading how it ended.

use crate::grade::AgentExit;
use crate::opencode;

/// Saves the agent's changes; run by the agent script at the end, and before a stop. The diff is
/// taken against the task image's one root commit, not `HEAD`: an agent that commits its work
/// (the prompt asks it not to, and some do anyway) would otherwise hand in an empty patch.
pub const SAVE_DIFF: &str = "cd /testbed && git add -A \
     && git diff --cached --binary \"$(git rev-list --max-parents=0 HEAD | tail -n 1)\" \
     > /eval/out/raw.patch";

/// Seconds before the runner's hard stop at which `timeout` stops opencode, so the diff is
/// still taken from a time-limited attempt.
pub const TIMEOUT_MARGIN_SECS: u64 = 120;

/// Parallel compile jobs inside eval containers.
pub const CARGO_JOBS: &str = "2";

/// Files the agent script reads from `/eval/in` and writes to `/eval/out`.
pub const PROMPT_FILE: &str = "prompt.md";
pub const CONFIG_FILE: &str = "opencode.json";
pub const SCRIPT_FILE: &str = "agent.sh";
pub const PATCH_FILE: &str = "raw.patch";
pub const EXPORT_FILE: &str = "session-export.json";
pub const EXIT_CODE_FILE: &str = "exit_code";

/// Runs the agent, then saves its session and the diff whatever happened.
pub fn script(agent_timeout_secs: u64) -> String {
    format!(
        "#!/bin/bash\nset -u\nmkdir -p \"$HOME\" /eval/out\ncd /testbed\n\
         timeout --signal=TERM {agent_timeout_secs} opencode run --model {model} --agent build --format json --auto --dir /testbed \"$(cat /eval/in/prompt.md)\" > /eval/out/events.jsonl 2> /eval/out/agent.log\n\
         echo $? > /eval/out/exit_code\n\
         SID=$(node -e 'const l=require(\"fs\").readFileSync(\"/eval/out/events.jsonl\",\"utf8\").split(\"\\n\");for(const x of l){{try{{const j=JSON.parse(x);if(j.sessionID){{console.log(j.sessionID);break}}}}catch(e){{}}}}')\n\
         if [ -n \"$SID\" ]; then opencode export \"$SID\" > /eval/out/session-export.json 2>> /eval/out/agent.log; fi\n\
         {SAVE_DIFF}\n",
        model = opencode::MODEL_ARG,
    )
}

/// Why the runner stopped a container before it ended by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    DiskLimit,
    TimeLimit,
}

/// How the agent ended. `exit_code_file` is the text of `/eval/out/exit_code` (absent when the
/// script never got that far); `container_exit` is the container's own exit code.
pub fn exit(
    stopped: Option<Stopped>,
    container_exit: Option<i32>,
    exit_code_file: Option<&str>,
    hit_step_limit: bool,
) -> AgentExit {
    match stopped {
        Some(Stopped::DiskLimit) => return AgentExit::DiskLimit,
        Some(Stopped::TimeLimit) => return AgentExit::TimeLimit,
        None => {}
    }
    let code: Option<i32> = exit_code_file.and_then(|c| c.trim().parse().ok());
    match code {
        Some(124) => AgentExit::TimeLimit,
        _ if hit_step_limit => AgentExit::StepLimit,
        Some(0) => AgentExit::Finished,
        Some(_) => AgentExit::Crashed,
        // The script never wrote its exit code: the container itself died (e.g. killed for
        // memory, exit 137) before the agent finished.
        None if container_exit.is_some() => AgentExit::Crashed,
        None => AgentExit::InfraError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_saves_session_and_diff_after_a_timeout() {
        let s = script(600);
        assert!(s.contains("timeout --signal=TERM 600 opencode run"));
        let timeout_at = s.find("timeout").expect("timeout");
        assert!(s.find("opencode export").expect("export") > timeout_at);
        assert!(s.trim_end().ends_with("/eval/out/raw.patch"));
    }

    #[test]
    fn exits_are_classified() {
        assert_eq!(
            exit(Some(Stopped::DiskLimit), None, None, false),
            AgentExit::DiskLimit
        );
        assert_eq!(
            exit(None, Some(0), Some("124\n"), false),
            AgentExit::TimeLimit
        );
        assert_eq!(exit(None, Some(0), Some("0"), true), AgentExit::StepLimit);
        assert_eq!(exit(None, Some(0), Some("0"), false), AgentExit::Finished);
        assert_eq!(exit(None, Some(0), Some("1"), false), AgentExit::Crashed);
        assert_eq!(exit(None, Some(137), None, false), AgentExit::Crashed);
        assert_eq!(exit(None, None, None, false), AgentExit::InfraError);
    }
}
