//! Per-session confinement: what a session's process tree may touch inside the shared pod.
//!
//! Every session in a runtime pod runs as the same uid, so without this layer one session's
//! harness could read another session's workspace, credentials and temp files. The supervisor
//! confines each session's shim between `fork` and `exec`, so the shim, its harness and
//! everything they start inherit it. Two independent layers:
//!
//! - **Filesystem** (Landlock): the session's own directory read-write (its workspace, and its
//!   `HOME` with a private `TMPDIR` inside) and, when the pod has a scratch area, its own scratch
//!   directory read-write, the image's system paths read-only, a few device nodes, nothing else.
//! - **Syscalls** (seccomp-bpf): a short denylist tightened over the pod's `RuntimeDefault`
//!   profile: the calls that reach into another process, and kernel facilities a coding harness
//!   never needs that are common privilege-escalation entry points.
//!
//! Each layer has its own switch, [`FS_ENV`] and [`SYSCALLS_ENV`], both on by default, so the
//! shared-pod measurement can price each one. A switch that is on while the kernel cannot honour
//! it stops the supervisor at startup, never a silent fallback: a measurement that ran unconfined
//! would report a cost for isolation it never had.
//!
//! What neither layer gives: a uid, cgroup or network namespace per session, which need
//! privileges the pod does not have. Sessions still see each other's process list
//! (`/proc/<pid>/status`, `cmdline`), can signal each other's processes by pid, can connect to a
//! Unix socket another session binds (Landlock controls that from ABI 9, Linux 7.1; abstract
//! sockets and signals are scoped from ABI 6, which this policy does not request yet), and share
//! the pod's CPU, memory and network.

use std::fmt;
use std::path::Path;

use anyhow::{Result, bail};
use tokio::process::Command;

/// Switches the filesystem layer (`0`/`false`/`off` turns it off).
pub(super) const FS_ENV: &str = "HARNESS_SUPERVISOR_CONFINE_FS";
/// Switches the syscall layer (`0`/`false`/`off` turns it off).
pub(super) const SYSCALLS_ENV: &str = "HARNESS_SUPERVISOR_CONFINE_SYSCALLS";

/// Which layers confine each session's process tree. Both are on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confinement {
    /// The Landlock filesystem view: the session's own directory and the image's system paths.
    pub fs: bool,
    /// The seccomp denylist over the pod's `RuntimeDefault` profile.
    pub syscalls: bool,
}

impl Default for Confinement {
    fn default() -> Self {
        Self {
            fs: true,
            syscalls: true,
        }
    }
}

impl fmt::Display for Confinement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = |on: bool| if on { "on" } else { "off" };
        write!(
            f,
            "fs {}, syscalls {}",
            state(self.fs),
            state(self.syscalls)
        )
    }
}

impl Confinement {
    /// Reads both switches through `get`, which treats an empty value as unset.
    pub(super) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        Ok(Self {
            fs: switch(FS_ENV, get(FS_ENV))?,
            syscalls: switch(SYSCALLS_ENV, get(SYSCALLS_ENV))?,
        })
    }

    fn any(self) -> bool {
        self.fs || self.syscalls
    }
}

/// Unset, `1`, `true` or `on` is on; `0`, `false` or `off` is off. Anything else is refused
/// rather than guessed, since the guess would decide whether a measurement ran confined.
fn switch(key: &str, raw: Option<String>) -> Result<bool> {
    let Some(raw) = raw else {
        return Ok(true);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        _ => bail!("{key} must be 1, true or on, or 0, false or off; got `{raw}`"),
    }
}

/// Fails when an enabled layer cannot be enforced here, so the supervisor refuses to start
/// instead of running sessions unconfined.
pub(super) fn startup_check(confinement: Confinement) -> Result<()> {
    platform::startup_check(confinement)
}

/// The directories one session may write: its own, and its scratch directory when the pod has a
/// scratch area. Both must exist before the session is confined to them.
#[derive(Debug, Clone, Copy)]
// Only the Linux policy reads the fields; elsewhere confinement is refused before any path is
// looked at, so they are dead there by design.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) struct SessionDirs<'a> {
    /// The session's directory: its workspace, with its `HOME` inside.
    pub(super) dir: &'a Path,
    /// The session's scratch directory, if any.
    pub(super) scratch: Option<&'a Path>,
}

/// Makes `command` confine the process it starts to `dirs` (see the module docs). A layer that
/// cannot be applied in the child fails the spawn.
pub(super) fn apply_to(
    command: &mut Command,
    confinement: Confinement,
    dirs: SessionDirs<'_>,
) -> Result<()> {
    platform::apply_to(command, confinement, dirs)
}

#[cfg(target_os = "linux")]
mod platform {
    //! The policies themselves. Both are built in the parent and only applied in the child, so
    //! the code between `fork` and `exec` is the handful of syscalls that install them.

    use std::io;
    use std::path::Path;

    use anyhow::{Context, Result, anyhow, bail};
    use landlock::{
        ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
        RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus, make_bitflags,
        path_beneath_rules,
    };
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch, sock_filter,
    };
    use tokio::process::Command;

    use super::super::SESSION_HOME;
    use super::{Confinement, FS_ENV, SYSCALLS_ENV, SessionDirs};

    // ── Filesystem policy (Landlock) ──────────────────────────────────────────────────────

    /// The Landlock ABI whose access rights this policy is written against (Linux 6.10): the
    /// floor's rights plus `Truncate` (6.2) and `IoctlDev` (6.10), which a kernel that lacks them
    /// simply does not enforce. Rights of later ABIs are not requested until this policy has
    /// been reviewed against them.
    const FS_ABI: ABI = ABI::V5;

    /// The oldest ABI the layer runs on (Linux 5.19), a hard requirement. ABI 1 cannot handle
    /// `Refer`, and a ruleset that does not handle it refuses every rename or link across
    /// directories, which git and every build tool do constantly.
    const FS_FLOOR: ABI = ABI::V2;

    /// Read and execute for every session: the image's programs, libraries and configuration.
    /// `/etc` carries DNS, CA certificates and the passwd entries tools look up; `/sys` the
    /// cgroup limits and CPU topology runtimes size themselves by. Paths an image lacks are
    /// skipped.
    ///
    /// Deliberately absent: `/tmp` and `/var/tmp` (shared by every session; each gets its own
    /// `TMPDIR` instead), the workspace root (every other session's directory), `/home` and
    /// `/root`, and `/run` and `/var`, where the pod's mounted secrets live (a service-account
    /// token under `/var/run/secrets`).
    const SYSTEM_PATHS: &[&str] = &[
        "/bin", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/opt", "/sbin", "/sys", "/usr",
    ];

    /// Read and write files under `/proc`. Runtimes read their own `/proc/self/*`, and some
    /// write to it (`oom_score_adj`, a thread's `comm`). Another process's sensitive entries
    /// (`mem`, `environ`, `fd`, `root`) stay out of reach: Landlock refuses ptrace-mode access
    /// across domains, and every session is a domain of its own. `/proc/sys` is root's.
    ///
    /// A deliberate hole: `oom_score_adj` is not one of those ptrace-checked entries, so the
    /// write grant lets a session raise a sibling's (lowering it needs a capability) and steer
    /// the OOM killer at the sibling. That is the same availability class as killing a sibling
    /// by pid, which this layer does not prevent either.
    const PROC: &str = "/proc";

    /// Device nodes a shell and its tools open for writing: the sinks and sources (`null`,
    /// `zero`, `full`, the random devices) and the terminals a harness opens for a command it
    /// runs (`tty`, `ptmx`, `pts/*`). The rest of `/dev` is read-only.
    ///
    /// A deliberate hole: `/dev/pts` is open to the whole uid, so a session can open a sibling's
    /// pseudo-terminal and read from it or inject input. Ptys are numbered as they are
    /// allocated, so Landlock, which grants by path when the session starts, cannot scope one to
    /// the session that opened it; refusing `/dev/pts` would break every command a harness runs
    /// in a terminal.
    const DEVICES: &[&str] = &[
        "/dev/full",
        "/dev/null",
        "/dev/ptmx",
        "/dev/pts",
        "/dev/random",
        "/dev/tty",
        "/dev/urandom",
        "/dev/zero",
    ];

    /// POSIX shared memory and named semaphores (Python's `multiprocessing`, Chromium). The pod
    /// has one `/dev/shm`, so a session can see another session's segments there: a deliberate,
    /// narrow hole, because refusing it breaks common test suites, and what lives there is IPC
    /// scratch, not a session's files.
    const SHARED_MEMORY: &str = "/dev/shm";

    /// The session's private temporary directory, inside its `HOME` rather than beside it: the
    /// shim's clone check ignores only `HOME` at the top of the session's directory, so a `tmp/`
    /// there would make a fresh session skip its repository (and land in the user's git tree).
    /// `TMPDIR` points the harness and its tools at it.
    const PRIVATE_TMP: &str = ".tmp";

    /// The rules every session shares: the image read-only, `/proc`, `/dev` and its writable
    /// nodes. The session's own directory is added by [`session_ruleset`].
    fn shared_ruleset() -> Result<RulesetCreated> {
        let read = AccessFs::from_read(FS_ABI);
        let read_files = make_bitflags!(AccessFs::{ReadFile | ReadDir});
        let device = make_bitflags!(AccessFs::{ReadFile | WriteFile | Truncate | IoctlDev});
        let ruleset = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(FS_FLOOR))?
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(FS_ABI))?
            .create()?
            .add_rules(path_beneath_rules(SYSTEM_PATHS, read))?
            .add_rules(path_beneath_rules([PROC], read_files | AccessFs::WriteFile))?
            .add_rules(path_beneath_rules(["/dev"], read_files))?
            .add_rules(path_beneath_rules(DEVICES, device | AccessFs::ReadDir))?
            .add_rules(path_beneath_rules(
                [SHARED_MEMORY],
                AccessFs::from_all(FS_ABI),
            ))?;
        Ok(ruleset)
    }

    /// [`shared_ruleset`] plus everything on the session's directories, and read and execute on
    /// `program` (the shim, which a test build keeps outside the system paths). All must exist:
    /// a missing session directory is an error here, not a rule silently skipped.
    fn session_ruleset(dirs: SessionDirs<'_>, program: &Path) -> Result<RulesetCreated> {
        let binary =
            PathFd::new(program).with_context(|| format!("opening {}", program.display()))?;
        let executable: BitFlags<AccessFs> = make_bitflags!(AccessFs::{ReadFile | Execute});
        let mut ruleset = shared_ruleset()?
            .add_rule(PathBeneath::new(
                open(dirs.dir)?,
                AccessFs::from_all(FS_ABI),
            ))?
            .add_rule(PathBeneath::new(binary, executable))?;
        if let Some(scratch) = dirs.scratch {
            ruleset =
                ruleset.add_rule(PathBeneath::new(open(scratch)?, AccessFs::from_all(FS_ABI)))?;
        }
        Ok(ruleset)
    }

    fn open(dir: &Path) -> Result<PathFd> {
        PathFd::new(dir).with_context(|| format!("opening {}", dir.display()))
    }

    /// In the child: enforces `ruleset` on the process about to `exec`.
    fn restrict_fs(ruleset: RulesetCreated) -> io::Result<()> {
        match ruleset.restrict_self() {
            // Partially: a kernel between the floor and `FS_ABI` leaves the newer rights out.
            Ok(status) if status.ruleset != RulesetStatus::NotEnforced => Ok(()),
            // The build checked the floor, so this cannot happen; refuse to run unconfined.
            Ok(_) => Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)),
            Err(err) => Err(errno_of(&err)),
        }
    }

    // ── Syscall policy (seccomp-bpf) ──────────────────────────────────────────────────────
    //
    // A denylist, not an allowlist: a session runs whatever its user's project needs
    // (compilers, test suites, package managers), which no allowlist can anticipate, and a
    // broken harness is worse than a marginally wider filter. Everything not listed keeps the
    // pod's `RuntimeDefault` behaviour. Refused calls fail with `EPERM`, as `RuntimeDefault`'s
    // own refusals do, so tools already handle it.
    //
    // Left to `RuntimeDefault` and the pod's dropped capabilities rather than repeated here:
    // namespaces (`unshare`, `setns`, `clone` with namespace flags, `clone3`), mounts, module
    // loading, reboot and clock setting all need a capability the pod does not have, and
    // `clone3`'s flags sit behind a pointer a filter cannot read.
    //
    // With the filesystem layer off, `/proc/<pid>/mem` of a sibling stays reachable through an
    // ordinary `open`, which a syscall filter cannot tell apart from any other; the Landlock
    // layer is what refuses it.

    /// Calls refused outright, whatever their arguments.
    const DENIED: &[libc::c_long] = &[
        // Reaching into another process: the reason this layer exists. Sessions share a uid,
        // and `RuntimeDefault` allows `ptrace` and `process_vm_*` on current kernels.
        //
        // Attach to and steer a sibling's process. Costs gdb, strace and LeakSanitizer.
        libc::SYS_ptrace,
        // Read or write another process's memory directly, no attach needed.
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        // Advise the kernel on another process's memory.
        libc::SYS_process_madvise,
        // Compare two processes' kernel objects: reveals what a sibling holds open.
        libc::SYS_kcmp,
        // Copy a file descriptor out of another process.
        libc::SYS_pidfd_getfd,
        //
        // Kernel facilities a coding harness never needs, and common entry points for kernel
        // exploits. Some `RuntimeDefault` versions allow them, so they are refused here too.
        //
        // eBPF programs and maps.
        libc::SYS_bpf,
        // Performance counters: a side channel onto co-located sessions.
        libc::SYS_perf_event_open,
        // User-space page-fault handling: the classic primitive for widening a kernel race.
        libc::SYS_userfaultfd,
        // io_uring: a large, often-patched surface. libuv and Bun fall back to epoll and
        // threads when it is refused.
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        // The kernel keyring, which is per uid: one session could read another's keys.
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
    ];

    /// `kill(-1, …)` signals every process the uid owns: every session in the pod and the
    /// supervisor. A harness signals its own children, never everyone. (The pid is an `int`, so
    /// only the low 32 bits are compared.)
    const BROADCAST_PID: u64 = u32::MAX as u64;

    /// x32 calls on x86_64 carry the native architecture tag with bit 30 set in the number, so
    /// a denylist keyed by native numbers would not see them. No such numbers exist on other
    /// architectures, where the check is three idle instructions.
    const X32_SYSCALL_BIT: u32 = 0x4000_0000;

    /// The compiled denylist for this architecture. A call from a foreign architecture (a
    /// 32-bit binary on a 64-bit kernel) kills the process: its numbers mean other calls, so the
    /// denylist cannot judge them, and no image here ships such a binary.
    fn syscall_filter() -> Result<BpfProgram> {
        let arch = TargetArch::try_from(std::env::consts::ARCH)
            .map_err(|err| anyhow!("no seccomp filter for this architecture: {err}"))?;
        let mut rules: std::collections::BTreeMap<i64, Vec<SeccompRule>> =
            DENIED.iter().map(|&nr| (nr, Vec::new())).collect();
        let broadcast =
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, BROADCAST_PID)?;
        rules.insert(libc::SYS_kill, vec![SeccompRule::new(vec![broadcast])?]);
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM.unsigned_abs()),
            arch,
        )?;
        Ok(refuse_x32(filter.try_into()?))
    }

    /// Prepends to `program` a check that refuses any call number with [`X32_SYSCALL_BIT`] set.
    fn refuse_x32(program: BpfProgram) -> BpfProgram {
        // Classic BPF; a conditional jump counts from the next instruction.
        let load_nr = sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0, // offsetof(struct seccomp_data, nr)
        };
        let is_x32 = sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16,
            jt: 0, // to the refusal
            jf: 1, // past it, into `program`
            k: X32_SYSCALL_BIT,
        };
        let refuse = sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM.unsigned_abs(),
        };
        [load_nr, is_x32, refuse]
            .into_iter()
            .chain(program)
            .collect()
    }

    // ── Applying ──────────────────────────────────────────────────────────────────────────

    pub(super) fn startup_check(confinement: Confinement) -> Result<()> {
        if confinement.fs {
            let enforced = on_probe_thread(|| Ok(shared_ruleset()?.restrict_self()?.ruleset))
                .with_context(|| {
                    format!(
                        "{FS_ENV} is on, but this kernel cannot enforce the filesystem policy \
                         (Landlock ABI {FS_FLOOR} or later: Linux 5.19+ with `landlock` among \
                         the boot LSMs); set {FS_ENV}=0 to run without it"
                    )
                })?;
            match enforced {
                RulesetStatus::FullyEnforced => tracing::info!("filesystem confinement: enforced"),
                RulesetStatus::PartiallyEnforced => warn_partial(),
                RulesetStatus::NotEnforced => bail!("{FS_ENV} is on, but Landlock is not enforced"),
            }
        }
        if confinement.syscalls {
            let program = syscall_filter().with_context(|| format!("{SYSCALLS_ENV} is on"))?;
            on_probe_thread(move || Ok(seccompiler::apply_filter(&program)?)).with_context(
                || format!("{SYSCALLS_ENV} is on, but this kernel refused the seccomp filter"),
            )?;
            tracing::info!("syscall confinement: enforced");
        }
        Ok(())
    }

    /// Says which rights a kernel between the floor and [`FS_ABI`] leaves out, and what that
    /// lets a session do, so a measurement on such a node is not read as fully confined.
    fn warn_partial() {
        if handles(AccessFs::Truncate) {
            tracing::warn!(
                "filesystem confinement: partially enforced; this kernel (before Linux 6.10) \
                 does not control device ioctls, so a session may issue ioctls on the device \
                 nodes it can open"
            );
        } else {
            tracing::warn!(
                "filesystem confinement: partially enforced; this kernel (before Linux 6.2) does \
                 not control truncation, so a session can truncate another session's files by \
                 path (truncate(2)) though it cannot open them"
            );
        }
    }

    /// Whether this kernel's Landlock can handle `access`.
    fn handles(access: AccessFs) -> bool {
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(access)
            .is_ok()
    }

    /// Runs `probe` on a thread of its own. Landlock and seccomp restrict only the calling
    /// thread, so the probe leaves the supervisor itself unconfined, and it ends with the
    /// thread.
    fn on_probe_thread<T: Send + 'static>(
        probe: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        std::thread::Builder::new()
            .name("confine-probe".into())
            .spawn(probe)
            .context("starting the confinement probe thread")?
            .join()
            .map_err(|_| anyhow!("the confinement probe panicked"))?
    }

    pub(super) fn apply_to(
        command: &mut Command,
        confinement: Confinement,
        dirs: SessionDirs<'_>,
    ) -> Result<()> {
        if !confinement.any() {
            return Ok(());
        }
        let mut ruleset = None;
        if confinement.fs {
            let tmp = dirs.dir.join(SESSION_HOME).join(PRIVATE_TMP);
            std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            command.env("TMPDIR", &tmp);
            let program = Path::new(command.as_std().get_program()).to_owned();
            ruleset = Some(session_ruleset(dirs, &program)?);
        }
        let filter = confinement.syscalls.then(syscall_filter).transpose()?;
        // SAFETY: the hook runs in the forked child, where only async-signal-safe work is sound.
        // It allocates nothing: the ruleset and the filter were built above, in the parent, and
        // installing them is `prctl`, `landlock_restrict_self`, `seccomp` and `close`. Only the
        // errno of a failure reaches the parent, which fails the spawn with it. The child takes
        // the ruleset out of its own copy of the closure; the parent's copy still holds it.
        unsafe {
            command.pre_exec(move || {
                if let Some(ruleset) = ruleset.take() {
                    restrict_fs(ruleset)?;
                }
                if let Some(filter) = &filter {
                    seccompiler::apply_filter(filter).map_err(|err| errno_of(&err))?;
                }
                Ok(())
            });
        }
        Ok(())
    }

    /// The first OS error in `err`'s chain as a bare errno, `EPERM` if there is none. Only the
    /// errno crosses from the child back to the spawning parent anyway.
    fn errno_of(err: &(dyn std::error::Error + 'static)) -> io::Error {
        let mut current = Some(err);
        while let Some(err) = current {
            let code = err
                .downcast_ref::<io::Error>()
                .and_then(io::Error::raw_os_error);
            if let Some(code) = code {
                return io::Error::from_raw_os_error(code);
            }
            current = err.source();
        }
        io::Error::from_raw_os_error(libc::EPERM)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The calling thread's errno after a raw syscall that returned `ret`.
        fn outcome(ret: libc::c_long) -> Result<libc::c_long, i32> {
            if ret < 0 {
                Err(io::Error::last_os_error().raw_os_error().unwrap())
            } else {
                Ok(ret)
            }
        }

        /// Reads one byte of this process's own memory through `process_vm_readv`.
        fn read_own_memory() -> Result<libc::c_long, i32> {
            let source = [7u8];
            let mut target = [0u8];
            let local = libc::iovec {
                iov_base: target.as_mut_ptr().cast(),
                iov_len: 1,
            };
            let remote = libc::iovec {
                iov_base: source.as_ptr() as *mut libc::c_void,
                iov_len: 1,
            };
            // SAFETY: both iovecs point at live one-byte buffers of this frame. `syscall` is
            // variadic: the counts and flags are `unsigned long` to the kernel, so they are
            // passed at that width; a bare `1` or `0` is an `int`, leaving the register's upper
            // half undefined, and the kernel then sees a huge count or stray flags (EINVAL).
            outcome(unsafe {
                libc::syscall(
                    libc::SYS_process_vm_readv,
                    libc::c_long::from(libc::getpid()),
                    &local as *const libc::iovec,
                    1 as libc::c_ulong,
                    &remote as *const libc::iovec,
                    1 as libc::c_ulong,
                    0 as libc::c_ulong,
                )
            })
        }

        #[test]
        fn the_filter_refuses_the_denied_calls_and_allows_the_rest() {
            assert_eq!(read_own_memory(), Ok(1), "the baseline must allow it");
            let program = syscall_filter().unwrap();
            on_probe_thread(move || {
                seccompiler::apply_filter(&program)?;
                assert_eq!(read_own_memory(), Err(libc::EPERM));
                // SAFETY: signal 0 delivers nothing; it only asks whether delivery is allowed.
                let broadcast = outcome(unsafe { libc::syscall(libc::SYS_kill, -1, 0) });
                assert_eq!(broadcast, Err(libc::EPERM));
                // SAFETY: as above, aimed at this process alone.
                let own = outcome(unsafe { libc::syscall(libc::SYS_kill, libc::getpid(), 0) });
                assert_eq!(own, Ok(0));
                // SAFETY: getpid has no arguments; the x32 bit makes it an unknown number.
                let x32 = unsafe {
                    libc::syscall(libc::SYS_getpid | libc::c_long::from(X32_SYSCALL_BIT))
                };
                assert_eq!(outcome(x32), Err(libc::EPERM));
                Ok(())
            })
            .unwrap();
            assert_eq!(read_own_memory(), Ok(1), "the probe thread's filter leaked");
        }

        #[test]
        fn startup_check_passes_with_both_layers_on_a_supporting_kernel() {
            // SAFETY: a version query: no pointers are read.
            let abi =
                unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0usize, 0usize, 1u32) };
            let below_floor = i32::try_from(abi).map_or(true, |abi| ABI::from(abi) < FS_FLOOR);
            if below_floor {
                eprintln!("skipped: this kernel offers Landlock ABI {abi}, below the floor");
                return;
            }
            startup_check(Confinement::default()).unwrap();
        }

        #[test]
        fn a_missing_session_directory_is_an_error_not_a_skipped_rule() {
            let root = tempfile::tempdir().unwrap();
            let absent = root.path().join("absent");
            let dirs = SessionDirs {
                dir: &absent,
                scratch: None,
            };
            assert!(session_ruleset(dirs, Path::new("/bin/sh")).is_err());
            let missing_scratch = SessionDirs {
                dir: root.path(),
                scratch: Some(&absent),
            };
            assert!(session_ruleset(missing_scratch, Path::new("/bin/sh")).is_err());
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    //! Landlock and seccomp are Linux kernel features, and the supervisor ships only in Linux
    //! images. Elsewhere (a developer's machine) both switches must be off.

    use anyhow::{Result, bail};
    use tokio::process::Command;

    use super::{Confinement, FS_ENV, SYSCALLS_ENV, SessionDirs};

    pub(super) fn startup_check(confinement: Confinement) -> Result<()> {
        refuse(confinement)
    }

    /// Unreachable past [`startup_check`], and still refuses rather than run unconfined.
    pub(super) fn apply_to(
        _: &mut Command,
        confinement: Confinement,
        _: SessionDirs<'_>,
    ) -> Result<()> {
        refuse(confinement)
    }

    fn refuse(confinement: Confinement) -> Result<()> {
        if confinement.any() {
            bail!(
                "session confinement ({confinement}) needs Linux, and this is {}: set {FS_ENV}=0 \
                 and {SYSCALLS_ENV}=0",
                std::env::consts::OS
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(fs: Option<&str>, syscalls: Option<&str>) -> Result<Confinement> {
        Confinement::from_lookup(|key| match key {
            FS_ENV => fs.map(str::to_owned),
            SYSCALLS_ENV => syscalls.map(str::to_owned),
            _ => None,
        })
    }

    #[test]
    fn both_layers_default_on() {
        assert_eq!(read(None, None).unwrap(), Confinement::default());
        assert!(Confinement::default().fs && Confinement::default().syscalls);
    }

    #[test]
    fn each_switch_reads_its_own_variable() {
        for off in ["0", "false", "off", "OFF", " 0 "] {
            let fs_off = read(Some(off), None).unwrap();
            assert_eq!((fs_off.fs, fs_off.syscalls), (false, true), "{off:?}");
            let syscalls_off = read(None, Some(off)).unwrap();
            assert_eq!(
                (syscalls_off.fs, syscalls_off.syscalls),
                (true, false),
                "{off:?}"
            );
        }
        for on in ["1", "true", "on", "True"] {
            assert_eq!(read(Some(on), Some(on)).unwrap(), Confinement::default());
        }
    }

    #[test]
    fn an_unrecognised_switch_value_is_refused() {
        for bad in ["yes", "2", "disabled"] {
            assert!(read(Some(bad), None).is_err(), "{bad}");
            assert!(read(None, Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_names_both_layers() {
        let partial = Confinement {
            fs: true,
            syscalls: false,
        };
        assert_eq!(partial.to_string(), "fs on, syscalls off");
    }

    fn dirs(dir: &Path) -> SessionDirs<'_> {
        SessionDirs { dir, scratch: None }
    }

    #[test]
    fn both_layers_off_needs_nothing_from_the_kernel() {
        let off = Confinement {
            fs: false,
            syscalls: false,
        };
        startup_check(off).unwrap();
        let mut command = Command::new("true");
        apply_to(&mut command, off, dirs(Path::new("/nonexistent"))).unwrap();
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn outside_linux_either_layer_is_refused_naming_the_platform() {
        for confinement in [
            Confinement {
                fs: true,
                syscalls: false,
            },
            Confinement {
                fs: false,
                syscalls: true,
            },
        ] {
            let err = startup_check(confinement).unwrap_err().to_string();
            assert!(err.contains(std::env::consts::OS), "{err}");
            let mut command = Command::new("true");
            assert!(apply_to(&mut command, confinement, dirs(Path::new("/tmp"))).is_err());
        }
    }
}
