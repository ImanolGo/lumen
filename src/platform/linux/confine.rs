//! Shutting the decoder process in: Landlock and seccomp.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitStatus;

use anyhow::{Result, anyhow};

/// The program to start as the worker. `current_exe()` names a deleted file once a package
/// upgrade has replaced the binary; this link still leads to the running one.
pub fn worker_exe() -> std::io::Result<PathBuf> {
    Ok("/proc/self/exe".into())
}

/// What a worker's end says about the sandbox, if anything.
pub fn explain(status: &ExitStatus) -> Option<&'static str> {
    use std::os::unix::process::ExitStatusExt;
    // A system call the sandbox does not allow. That is the sandbox doing its job (or a bug).
    (status.signal() == Some(libc::SIGSYS))
        .then_some("the picture tried something the decoder's sandbox does not allow, so it was not opened")
}

/// Tries something the sandbox must stop, for its tests; false if there is no such test.
pub fn escape(test: &str) -> bool {
    match test {
        "exec" => drop(std::process::Command::new("/bin/true").status()),
        "stat" => drop(rustix::fs::stat("/etc/hostname")),
        "read-other" => drop(rustix::io::read(std::io::stdout(), &mut [0u8; 1])),
        "dumpable" => drop(rustix::process::dumpable_behavior()),
        _ => return false,
    }
    true
}

/// Shuts the process in. Landlock removes every file system right (best effort: older
/// kernels lack it); seccomp then allows only what decoding needs. Seccomp is required.
pub fn enter(max_memory: u64) -> Result<()> {
    use landlock::{ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr};
    // No file system rights at all, whatever the kernel's ABI knows about.
    let landlock = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V5))
        .and_then(|r| r.create())
        .and_then(|r| r.restrict_self());
    if let Err(e) = landlock {
        // Seccomp below still stops file access; Landlock is a second wall.
        let _ = writeln!(std::io::stderr(), "lumen-rs worker: no Landlock ({e})");
    }
    // Best effort: the decoders check sizes themselves, this catches what they miss.
    let memory = rustix::process::Rlimit { current: Some(max_memory), maximum: Some(max_memory) };
    let _ = rustix::process::setrlimit(rustix::process::Resource::As, memory);
    seccomp()
}

/// The system calls a decoder needs, found by watching real decodes (see `tests/sandbox.rs`).
fn allowed_syscalls() -> Vec<i64> {
    use libc::*;
    vec![
        // memory
        SYS_mmap,
        SYS_munmap,
        SYS_mprotect,
        SYS_mremap,
        SYS_madvise,
        SYS_brk,
        // leaving descriptors alone but for their size and place
        SYS_close,
        SYS_fstat,
        SYS_lseek,
        // threads and waiting
        SYS_futex,
        // answered with ENOSYS, see `seccomp`
        SYS_clone3,
        SYS_set_robust_list,
        SYS_rseq,
        SYS_sched_yield,
        SYS_sched_getaffinity,
        SYS_nanosleep,
        SYS_clock_nanosleep,
        SYS_gettid,
        SYS_getpid,
        SYS_set_tid_address,
        // signals (Rust's runtime, panics)
        SYS_rt_sigaction,
        SYS_rt_sigprocmask,
        SYS_rt_sigreturn,
        SYS_sigaltstack,
        // time, randomness
        SYS_clock_gettime,
        SYS_getrandom,
        // leaving
        SYS_exit,
        SYS_exit_group,
    ]
}

fn seccomp() -> Result<()> {
    use seccompiler::SeccompCmpArgLen::{Dword, Qword};
    use seccompiler::{BpfProgram, SeccompAction, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule};
    use std::collections::BTreeMap;
    let arch: seccompiler::TargetArch =
        std::env::consts::ARCH.try_into().map_err(|_| anyhow!("no sandbox for this processor"))?;
    let first = |len, op, value: u64| -> Result<SeccompRule> {
        Ok(SeccompRule::new(vec![SeccompCondition::new(0, len, op, value)?])?)
    };
    let is = |value: u64| first(Dword, SeccompCmpOp::Eq, value);

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = allowed_syscalls().into_iter().map(|n| (n, vec![])).collect();
    // Reading is only for the picture, writing for the answer (and a last word on standard error).
    rules.insert(libc::SYS_read, vec![is(0)?]);
    rules.insert(libc::SYS_write, vec![is(1)?, is(2)?]);
    // Threads only: no new processes.
    let thread = libc::CLONE_THREAD as u64;
    rules.insert(libc::SYS_clone, vec![first(Qword, SeccompCmpOp::MaskedEq(thread), thread)?]);
    // Signals only to itself (`abort`).
    rules.insert(libc::SYS_tgkill, vec![is(u64::from(std::process::id()))?]);
    // Naming threads, and (newer C libraries) mappings.
    rules.insert(libc::SYS_prctl, vec![is(libc::PR_SET_NAME as u64)?, is(libc::PR_SET_VMA as u64)?]);
    let filter: BpfProgram =
        SeccompFilter::new(rules, SeccompAction::KillProcess, SeccompAction::Allow, arch)?.try_into()?;

    // `clone3` takes its flags in memory, where a filter cannot look. "Not implemented" makes
    // the C library use `clone`. Of two filters, the kernel takes the stricter answer.
    let no_clone3: BpfProgram = SeccompFilter::new(
        BTreeMap::from([(libc::SYS_clone3, vec![])]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        arch,
    )?
    .try_into()?;
    seccompiler::apply_filter(&no_clone3)?;
    seccompiler::apply_filter(&filter)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_is_the_running_program_even_if_its_file_was_replaced() {
        let exe = worker_exe().unwrap();
        assert_eq!(exe, PathBuf::from("/proc/self/exe"));
        assert_eq!(std::fs::canonicalize(exe).unwrap(), std::env::current_exe().unwrap());
    }
}
