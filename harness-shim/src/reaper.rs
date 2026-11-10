//! The shim as the reaper of its session's processes (Linux).
//!
//! A harness can leave processes behind: a job started with `nohup ... &` whose shell then exits,
//! a dev server that daemonizes itself. Their parent gone, the kernel re-parents them to the
//! nearest *subreaper*, and without one to the pod's init (tini), outside the shim's tree: the
//! session's stats stop counting them and stopping the session no longer reaches them. So the
//! shim makes itself a child subreaper at startup ([`adopt_orphans`]): such processes stay in its
//! tree, are counted, and are stopped with it ([`stop_tree`]).
//!
//! The shim then has to reap what it adopts, or each would linger as a zombie. Its own children
//! (the harness, git) are tokio's to wait for: taking one's exit status here would break that
//! wait. So they are spawned as an [`OwnChild`], whose pid is recorded as the shim's own from
//! before it exists until its wait returns, and the reaper, which reads `/proc` under the same
//! lock, only reaps zombies it did not spawn.

use std::collections::BTreeSet;
use std::process::ExitStatus;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::procstats::{self, ProcessId, StatLine};

/// How long the processes left in the shim's tree get after SIGTERM before SIGKILL, once the
/// harness itself has stopped.
pub const TREE_STOP_GRACE: Duration = Duration::from_secs(2);
/// How often [`stop_tree`] looks whether the tree has emptied.
const STOP_POLL: Duration = Duration::from_millis(100);

/// The pids of the shim's own children whose wait has not returned: tokio's to reap.
static OWN_CHILDREN: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

fn own_children() -> MutexGuard<'static, BTreeSet<u32>> {
    OWN_CHILDREN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One of the shim's own children. Its pid is recorded as the shim's own from before the child
/// can exit until its wait returns, so the reaper never takes its exit status from tokio. One
/// dropped unwaited is killed, and waited for in the background before its pid is forgotten.
pub struct OwnChild {
    /// `None` only once dropped.
    child: Option<Child>,
    /// `None` once the child has been waited for (or was never recorded).
    pid: Option<u32>,
}

impl OwnChild {
    /// Spawns `command`, recording the child under the lock the reaper scans under.
    pub(crate) fn spawn(command: &mut Command) -> std::io::Result<Self> {
        let mut own = own_children();
        let child = command.spawn()?;
        let pid = child.id();
        if let Some(pid) = pid {
            own.insert(pid);
        }
        Ok(Self {
            child: Some(child),
            pid,
        })
    }

    fn child(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("an own child is present until dropped")
    }

    /// The child's pid, `None` once it has been waited for.
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child().stdin.take()
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child().stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child().stderr.take()
    }

    /// Waits for the child to exit, then forgets it as the shim's own: reaped, its pid may now
    /// name another process. Cancel-safe, like tokio's.
    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let status = self.child().wait().await?;
        if let Some(pid) = self.pid.take() {
            own_children().remove(&pid);
        }
        Ok(status)
    }

    /// SIGKILLs the child and waits for it.
    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.child().start_kill()?;
        self.wait().await.map(drop)
    }
}

impl Drop for OwnChild {
    /// An unwaited child is killed and waited for on the runtime, and only then forgotten, so its
    /// pid is not kept as the shim's own for good. With no runtime to wait on, it is left to
    /// tokio's orphan queue and forgotten at once: an orphan has no waiter the reaper could
    /// take its status from.
    fn drop(&mut self) {
        let (Some(mut child), Some(pid)) = (self.child.take(), self.pid.take()) else {
            return;
        };
        let _ = child.start_kill();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            drop(child);
            own_children().remove(&pid);
            return;
        };
        runtime.spawn(async move {
            if let Err(err) = child.wait().await {
                tracing::warn!(pid, %err, "waiting for a dropped child failed");
            }
            own_children().remove(&pid);
        });
    }
}

/// Makes the shim a child subreaper and starts reaping the processes it adopts. A no-op on a
/// platform without subreapers.
pub fn adopt_orphans() -> anyhow::Result<()> {
    platform::become_subreaper()?;
    platform::spawn_reaping();
    Ok(())
}

/// The zombies among `pid`'s children in `table` that it did not spawn itself (`own`): processes
/// it adopted, which only it can reap.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn adopted_zombies(table: &[StatLine], pid: u32, own: &BTreeSet<u32>) -> Vec<u32> {
    table
        .iter()
        .filter(|e| e.ppid == pid && e.state == 'Z' && !own.contains(&e.pid))
        .map(|e| e.pid)
        .collect()
}

/// Calls `reap` for each zombie child of the shim's in the table `read` returns that it did not
/// spawn itself. The table is read under the lock [`OwnChild::spawn`] records under, so a child
/// spawned around the read is either recorded and in the table, or in neither: never in the table
/// unrecorded, where it would be reaped from under tokio.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn reap_adopted_with(read: impl FnOnce() -> Option<Vec<StatLine>>, mut reap: impl FnMut(u32)) {
    let own = own_children();
    let Some(table) = read() else {
        return;
    };
    for zombie in adopted_zombies(&table, std::process::id(), &own) {
        reap(zombie);
    }
}

/// Stops every process still in the shim's tree: SIGTERM, up to `grace` for them to go, then
/// SIGKILL. The shim is their subreaper, so that is every process the session left, whatever
/// process group or session it moved to. Call it with the harness already stopped, on the way
/// out.
pub async fn stop_tree(grace: Duration) {
    let me = std::process::id();
    let Some(left) = live_tree(me) else {
        return;
    };
    tracing::info!(
        processes = left.len(),
        "stopping the processes the session left"
    );
    signal_all(&left, libc::SIGTERM);
    let deadline = tokio::time::Instant::now() + grace;
    while tokio::time::Instant::now() < deadline {
        platform::reap_adopted();
        if live_tree(me).is_none() {
            return;
        }
        tokio::time::sleep(STOP_POLL).await;
    }
    if let Some(stubborn) = live_tree(me) {
        tracing::warn!(
            processes = stubborn.len(),
            "processes the session left ignored SIGTERM; killing them"
        );
        signal_all(&stubborn, libc::SIGKILL);
    }
    platform::reap_adopted();
}

/// The live processes in `root`'s tree, `None` when there are none (or `/proc` is unreadable).
fn live_tree(root: u32) -> Option<Vec<ProcessId>> {
    procstats::descendant_processes(root).filter(|processes| !processes.is_empty())
}

/// Sends `signal` to each of `processes` still running under its pid. One that has exited, and
/// whose pid may since have gone to an unrelated process, is skipped.
pub(crate) fn signal_all(processes: &[ProcessId], signal: libc::c_int) {
    for process in processes {
        // 0 and 1 would address a process group or every process; a descendant is neither.
        let Some(pid) = libc::pid_t::try_from(process.pid).ok().filter(|id| *id > 1) else {
            continue;
        };
        if !procstats::still_running(*process) {
            continue;
        }
        // SAFETY: signal delivery only, to a process in the caller's own tree.
        unsafe { libc::kill(pid, signal) };
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use anyhow::{Context, bail};
    use tokio::signal::unix::{SignalKind, signal};

    pub(super) fn become_subreaper() -> anyhow::Result<()> {
        // SAFETY: a process-attribute flag on the calling process; no memory is passed.
        let set = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if set != 0 {
            bail!(
                "becoming a child subreaper: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    /// Reaps on every SIGCHLD for as long as the process lives.
    pub(super) fn spawn_reaping() {
        let mut children = match signal(SignalKind::child()).context("installing SIGCHLD handler") {
            Ok(children) => children,
            Err(err) => {
                tracing::error!(%err, "processes the session leaves will not be reaped");
                return;
            }
        };
        tokio::spawn(async move {
            while children.recv().await.is_some() {
                reap_adopted();
            }
        });
    }

    /// Reaps every adopted zombie.
    pub(super) fn reap_adopted() {
        super::reap_adopted_with(crate::procstats::read_proc_table, waitpid);
    }

    pub(super) fn waitpid(pid: u32) {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: waits for one specific child that is ours to reap; no status is read.
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    pub(super) fn become_subreaper() -> anyhow::Result<()> {
        Ok(())
    }

    pub(super) fn spawn_reaping() {}

    pub(super) fn reap_adopted() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, ppid: u32, state: char) -> StatLine {
        StatLine {
            pid,
            state,
            ppid,
            utime: 0,
            stime: 0,
            cutime: 0,
            cstime: 0,
            start_time: 0,
            rss_pages: 0,
        }
    }

    #[test]
    fn only_adopted_zombie_children_are_reaped() {
        let shim = 10;
        let table = [
            entry(shim, 1, 'S'),
            // The harness: the shim's own child, exited, tokio's to reap.
            entry(11, shim, 'Z'),
            // A daemonized server that exited: adopted, the shim's to reap.
            entry(12, shim, 'Z'),
            // A live adopted process: nothing to reap yet.
            entry(13, shim, 'S'),
            // A zombie of another parent: not the shim's.
            entry(14, 13, 'Z'),
        ];
        let own = BTreeSet::from([11]);
        assert_eq!(adopted_zombies(&table, shim, &own), vec![12]);
    }

    /// The race this guards: the table read, then a child spawned and exited, then the scan. With
    /// the read under the spawn lock, no spawn fits between the read and the scan.
    #[test]
    fn the_table_is_read_while_no_child_can_be_recorded() {
        let mut locked_during_read = false;
        reap_adopted_with(
            || {
                locked_during_read = OWN_CHILDREN.try_lock().is_err();
                Some(Vec::new())
            },
            |_| {},
        );
        assert!(locked_during_read);
    }

    /// One dropped without a wait is killed, waited for, and then forgotten.
    #[tokio::test]
    async fn a_dropped_own_child_is_killed_and_then_forgotten() {
        let child = OwnChild::spawn(Command::new("sleep").arg("30")).expect("spawn sleep");
        let pid = child.id().expect("a live child has a pid");
        assert!(own_children().contains(&pid));
        drop(child);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while own_children().contains(&pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "the dropped child is still recorded"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn an_own_child_stays_recorded_until_its_wait_returns() {
        let mut child = OwnChild::spawn(&mut Command::new("true")).expect("spawn true");
        let pid = child.id().expect("a live child has a pid");
        assert!(own_children().contains(&pid));
        assert!(child.wait().await.expect("waited").success());
        assert!(!own_children().contains(&pid));
    }

    /// An own child that has exited, and is a zombie when the reaper scans, is left to tokio's
    /// wait, which still sees its exit status.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_reaper_leaves_an_exited_own_child_to_its_wait() {
        let mut child = OwnChild::spawn(&mut Command::new("true")).expect("spawn true");
        let pid = child.id().expect("a live child has a pid");
        let only_it = || -> Option<Vec<StatLine>> {
            let table = procstats::read_proc_table()?;
            Some(table.into_iter().filter(|e| e.pid == pid).collect())
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !only_it().is_some_and(|t| t.iter().any(|e| e.state == 'Z')) {
            assert!(std::time::Instant::now() < deadline, "`true` never exited");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        reap_adopted_with(only_it, platform::waitpid);
        let status = child.wait().await.expect("tokio still reaps its own child");
        assert!(status.success());
    }

    /// A process is signalled only while its pid still names it: a snapshot whose start time no
    /// longer matches (the pid went to another process) is skipped.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_process_is_signalled_only_while_its_pid_still_names_it() {
        let mut child = OwnChild::spawn(Command::new("sleep").arg("30").kill_on_drop(true))
            .expect("spawn sleep");
        let pid = child.id().expect("a live child has a pid");
        let table = procstats::read_proc_table().expect("/proc is readable");
        let seen = table
            .iter()
            .find(|e| e.pid == pid)
            .expect("sleep is listed");
        let reused = ProcessId {
            pid,
            start_time: seen.start_time + 1,
        };
        signal_all(&[reused], libc::SIGKILL);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), child.wait())
                .await
                .is_err(),
            "a stale snapshot was signalled"
        );
        let current = ProcessId {
            pid,
            start_time: seen.start_time,
        };
        signal_all(&[current], libc::SIGKILL);
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("the current snapshot was signalled")
            .expect("waited");
        assert!(!status.success());
    }
}
