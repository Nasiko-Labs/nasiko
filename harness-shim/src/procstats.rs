//! Process, CPU and memory accounting for the shim's own process tree, read from `/proc`.
//!
//! The sandbox has no per-actor stats API, so the shim reports its descendants itself. The
//! parsing and arithmetic are platform-independent; only the `/proc` walk is Linux-only.
//!
//! A process whose parent exits is re-parented to its nearest subreaper. On Linux the shim is one
//! (`reaper`), so a backgrounded job whose shell exited, or a server that daemonized itself, stays
//! in the shim's tree and is counted; elsewhere it would go to the init (tini) and escape the
//! count. CPU of exited descendants is kept through the `cutime`/`cstime` the shim and its live
//! processes accumulate when they reap children, so a short-lived subprocess does not make the
//! counter fall.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// One process's accounting fields from `/proc/<pid>/stat`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct StatLine {
    pub pid: u32,
    /// The state letter: `R`, `S`, `Z` (a zombie, exited and not yet reaped), ...
    pub state: char,
    pub ppid: u32,
    pub utime: u64,
    pub stime: u64,
    /// CPU of children this process has waited for.
    pub cutime: u64,
    pub cstime: u64,
    /// When the process started, in ticks since boot: tells it apart from a later process that
    /// reuses its pid.
    pub start_time: u64,
    /// Resident set size in pages.
    pub rss_pages: u64,
}

/// A process as seen at one instant: its pid and its start time, which together name it even
/// after the pid has gone to another process.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) struct ProcessId {
    pub pid: u32,
    pub start_time: u64,
}

/// Totals over the shim's descendants at one instant.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub at: Instant,
    /// `None` when `/proc` cannot be read (or on a platform without it): unknown, never 0.
    pub processes: Option<u32>,
    /// Cumulative user + system CPU ticks, including reaped children.
    pub cpu_ticks: u64,
    pub memory_bytes: u64,
}

/// Parses one `/proc/<pid>/stat` line. The command name sits in parentheses and may itself
/// contain spaces or parentheses, so everything after the *last* `)` is the fixed-layout tail.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_stat_line(line: &str) -> Option<StatLine> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let pid = line.get(..open)?.trim().parse().ok()?;
    let tail: Vec<&str> = line.get(close + 1..)?.split_whitespace().collect();
    // After the comm: state is field 3, so ppid is tail[1], utime/stime/cutime/cstime are
    // fields 14-17, the start time is field 22 and rss is field 24.
    Some(StatLine {
        pid,
        state: tail.first()?.chars().next()?,
        ppid: tail.get(1)?.parse().ok()?,
        utime: tail.get(11)?.parse().ok()?,
        stime: tail.get(12)?.parse().ok()?,
        cutime: tail.get(13)?.parse().ok()?,
        cstime: tail.get(14)?.parse().ok()?,
        start_time: tail.get(19)?.parse().ok()?,
        rss_pages: tail.get(21)?.parse().ok()?,
    })
}

/// CPU use in thousandths of a core over `secs` seconds, given `delta_ticks` at `hz` ticks/s.
pub fn millicores(delta_ticks: u64, hz: u64, secs: f64) -> u32 {
    if hz == 0 || secs <= 0.0 {
        return 0;
    }
    let value = delta_ticks as f64 / hz as f64 / secs * 1000.0;
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The entries whose ancestor chain reaches `root` (the root itself is not included).
pub fn descendants(entries: &[StatLine], root: u32) -> Vec<StatLine> {
    let parents: HashMap<u32, u32> = entries.iter().map(|e| (e.pid, e.ppid)).collect();
    let reaches_root = |start: &StatLine| {
        let mut current = start.ppid;
        // The bound stops a malformed table from looping forever.
        for _ in 0..parents.len() {
            if current == root {
                return true;
            }
            match parents.get(&current) {
                Some(&parent) if parent != current => current = parent,
                _ => return false,
            }
        }
        false
    };
    entries
        .iter()
        .filter(|e| reaches_root(e))
        .copied()
        .collect()
}

/// Every live process in `root`'s tree right now, `None` when `/proc` cannot be read.
pub(crate) fn descendant_processes(root: u32) -> Option<Vec<ProcessId>> {
    Some(live_descendants(&read_proc_table()?, root))
}

/// The entries of `root`'s tree in `entries` that still run: a zombie has exited already, and
/// waits only to be reaped.
pub(crate) fn live_descendants(entries: &[StatLine], root: u32) -> Vec<ProcessId> {
    descendants(entries, root)
        .iter()
        .filter(|e| e.state != 'Z')
        .map(|e| ProcessId {
            pid: e.pid,
            start_time: e.start_time,
        })
        .collect()
}

/// Whether `process` still runs under its pid: the pid names a process that started when it did.
/// False once it has exited, or where `/proc` cannot be read.
pub(crate) fn still_running(process: ProcessId) -> bool {
    read_stat(process.pid).is_some_and(|now| now.start_time == process.start_time)
}

/// Ticks per second of the `utime`/`stime` counters.
pub fn clock_ticks_per_sec() -> u64 {
    // SAFETY: sysconf only reads a system constant.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(hz).unwrap_or(0)
}

fn page_size() -> u64 {
    // SAFETY: sysconf only reads a system constant.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(size).unwrap_or(4096)
}

/// Cumulative CPU ticks of `root`'s tree: live descendants' own and reaped-child time, plus the
/// reaped-child time of `root` itself (its own time is the shim's, not the harness's).
pub fn tree_cpu_ticks(entries: &[StatLine], tree: &[StatLine], root: u32) -> u64 {
    let live: u64 = tree
        .iter()
        .map(|e| e.utime + e.stime + e.cutime + e.cstime)
        .sum();
    let reaped_by_root: u64 = entries
        .iter()
        .find(|e| e.pid == root)
        .map_or(0, |e| e.cutime + e.cstime);
    live + reaped_by_root
}

/// Totals over the descendants of `root` right now.
pub fn sample(root: u32) -> Sample {
    let Some(entries) = read_proc_table().filter(|e| e.iter().any(|x| x.pid == root)) else {
        return Sample {
            at: Instant::now(),
            processes: None,
            cpu_ticks: 0,
            memory_bytes: 0,
        };
    };
    let tree = descendants(&entries, root);
    let pages: u64 = tree.iter().map(|e| e.rss_pages).sum();
    Sample {
        at: Instant::now(),
        processes: Some(u32::try_from(tree.len()).unwrap_or(u32::MAX)),
        cpu_ticks: tree_cpu_ticks(&entries, &tree, root),
        memory_bytes: pages.saturating_mul(page_size()),
    }
}

/// Remembers the previous sample so CPU can be reported as a rate.
#[derive(Default)]
pub struct Sampler {
    previous: Mutex<Option<Sample>>,
}

impl Sampler {
    /// Stores `current` and returns the CPU use since the previous sample (0 for the first).
    pub fn record(&self, current: Sample) -> u32 {
        let previous = self
            .previous
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(current);
        let Some(previous) = previous else {
            return 0;
        };
        let secs = current
            .at
            .saturating_duration_since(previous.at)
            .as_secs_f64();
        millicores(
            current.cpu_ticks.saturating_sub(previous.cpu_ticks),
            clock_ticks_per_sec(),
            secs,
        )
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn read_proc_table() -> Option<Vec<StatLine>> {
    let dir = std::fs::read_dir("/proc").ok()?;
    let table = dir
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        // A process can exit between listing and reading; it is simply skipped.
        .filter_map(read_stat)
        .collect();
    Some(table)
}

#[cfg(target_os = "linux")]
fn read_stat(pid: u32) -> Option<StatLine> {
    parse_stat_line(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn read_proc_table() -> Option<Vec<StatLine>> {
    None
}

#[cfg(not(target_os = "linux"))]
fn read_stat(_pid: u32) -> Option<StatLine> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "4242 (my prog) S 1 4242 4242 0 -1 4194304 100 0 0 0 7 3 5 2 20 0 1 0 5000 1000000 250 18446744073709551615";

    #[test]
    fn parses_a_comm_containing_a_space() {
        let parsed = parse_stat_line(LINE).unwrap();
        assert_eq!(
            parsed,
            StatLine {
                pid: 4242,
                state: 'S',
                ppid: 1,
                utime: 7,
                stime: 3,
                cutime: 5,
                cstime: 2,
                start_time: 5000,
                rss_pages: 250
            }
        );
    }

    #[test]
    fn parses_a_comm_containing_a_paren() {
        let line = LINE.replace("(my prog)", "(a) b)");
        assert_eq!(parse_stat_line(&line).unwrap().ppid, 1);
    }

    #[test]
    fn rejects_a_truncated_line() {
        assert!(parse_stat_line("12 (x) S 1").is_none());
        assert!(parse_stat_line("garbage").is_none());
    }

    #[test]
    fn millicores_scales_ticks_to_thousandths_of_a_core() {
        // 100 ticks at 100 Hz is one CPU-second; over one second that is one full core.
        assert_eq!(millicores(100, 100, 1.0), 1000);
        assert_eq!(millicores(50, 100, 2.0), 250);
        assert_eq!(millicores(0, 100, 1.0), 0);
        assert_eq!(millicores(10, 0, 1.0), 0);
        assert_eq!(millicores(10, 100, 0.0), 0);
    }

    fn entry(pid: u32, ppid: u32) -> StatLine {
        StatLine {
            pid,
            state: 'S',
            ppid,
            utime: 0,
            stime: 0,
            cutime: 0,
            cstime: 0,
            start_time: u64::from(pid) * 100,
            rss_pages: 0,
        }
    }

    #[test]
    fn descendants_follow_the_ancestor_chain_and_exclude_the_root() {
        let table = [
            entry(10, 1),
            entry(11, 10),
            entry(12, 11),
            entry(20, 1),
            entry(21, 20),
        ];
        let mut pids: Vec<u32> = descendants(&table, 10).iter().map(|e| e.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![11, 12]);
    }

    #[test]
    fn cpu_keeps_time_of_reaped_children_and_ignores_the_roots_own() {
        let at = |pid, ppid, own, reaped| StatLine {
            pid,
            state: 'S',
            ppid,
            utime: own,
            stime: 0,
            cutime: reaped,
            cstime: 0,
            start_time: 0,
            rss_pages: 0,
        };
        let table = [at(10, 1, 100, 40), at(11, 10, 7, 3), at(12, 11, 5, 0)];
        let tree = descendants(&table, 10);
        // 11: 7+3, 12: 5, plus what the root reaped (40); the root's own 100 is excluded.
        assert_eq!(tree_cpu_ticks(&table, &tree, 10), 55);
    }

    /// A daemonized server's parent exited: with the shim as subreaper it is re-parented to the
    /// shim, so it is in the shim's tree and counted, wherever its process group went.
    #[test]
    fn a_process_re_parented_to_the_shim_is_in_its_tree() {
        let shim = 10;
        // 11 is the harness; 12 was started by a shell that exited, so its parent is now the shim.
        let table = [
            entry(shim, 1),
            entry(11, shim),
            entry(12, shim),
            entry(13, 12),
        ];
        let mut pids: Vec<u32> = descendants(&table, shim).iter().map(|e| e.pid).collect();
        pids.sort_unstable();
        assert_eq!(pids, vec![11, 12, 13]);
    }

    /// A zombie in the tree has exited: it is not signalled, and does not keep the tree from
    /// reading as empty.
    #[test]
    fn a_zombie_is_not_a_live_descendant() {
        let shim = 10;
        let exited = StatLine {
            state: 'Z',
            ..entry(12, shim)
        };
        let table = [entry(shim, 1), entry(11, shim), exited];
        assert_eq!(
            live_descendants(&table, shim),
            vec![ProcessId {
                pid: 11,
                start_time: 1100
            }]
        );
    }

    #[test]
    fn descendants_survive_a_cycle() {
        let table = [entry(5, 6), entry(6, 5)];
        assert!(descendants(&table, 10).is_empty());
    }
}
