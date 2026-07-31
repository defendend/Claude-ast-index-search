use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

const STDOUT_LIMIT: usize = 16 * 1024 * 1024;
const STDERR_LIMIT: usize = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vcs {
    Arc,
    Git,
}

impl Vcs {
    fn command_name(self) -> &'static str {
        match self {
            Self::Arc => "arc",
            Self::Git => "git",
        }
    }

    fn default_base(self) -> &'static str {
        match self {
            Self::Arc => "trunk",
            Self::Git => "origin/main",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ChangeStatus {
    A,
    M,
    D,
    R,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChangedFile {
    pub status: ChangeStatus,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChangedResult {
    pub schema_version: u32,
    pub vcs: Vcs,
    pub base: String,
    pub head: String,
    pub scope: Option<String>,
    pub changes: Vec<ChangedFile>,
}

#[derive(Debug)]
struct VcsRoot {
    vcs: Vcs,
    path: PathBuf,
}

#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug)]
struct ProcessOutput {
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
}

#[derive(Clone, Copy, Debug)]
struct Deadline {
    started: Instant,
    timeout: Duration,
}

impl Deadline {
    fn new(timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            timeout,
        }
    }

    fn expired(self) -> bool {
        self.started.elapsed() >= self.timeout
    }

    fn remaining(self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    fn timeout_error(self) -> anyhow::Error {
        anyhow!("timed out after {}ms", self.timeout.as_millis())
    }
}

pub fn cmd_changed(
    invocation_cwd: &Path,
    base: Option<&str>,
    timeout_ms: u64,
    verbose: bool,
    format: &str,
) -> Result<()> {
    let started = Instant::now();
    let vcs_root = discover_vcs_root(invocation_cwd)?;
    let scope = invocation_scope(invocation_cwd, &vcs_root.path)?;
    let executable = vcs_executable(vcs_root.vcs);
    let deadline = Deadline::new(Duration::from_millis(timeout_ms));

    if verbose {
        eprintln!(
            "changed: vcs={} root={:?} scope={:?} timeout={}ms",
            vcs_root.vcs.command_name(),
            vcs_root.path,
            scope.as_deref().unwrap_or("."),
            timeout_ms
        );
    }

    let base = resolve_base(
        vcs_root.vcs,
        base,
        &executable,
        &vcs_root.path,
        deadline,
        verbose,
    )?;
    let args = diff_args(vcs_root.vcs, &base, scope.as_deref());
    let command_dir = if vcs_root.vcs == Vcs::Arc && scope.is_some() {
        invocation_cwd
    } else {
        &vcs_root.path
    };

    let output =
        run_bounded(&executable, &args, command_dir, deadline, verbose).with_context(|| {
            format!(
                "{} changed-files command failed",
                vcs_root.vcs.command_name()
            )
        })?;

    if !output.status.success() {
        let stderr = render_stderr(&output.stderr);
        bail!(
            "{} changed-files command exited with {}{}",
            vcs_root.vcs.command_name(),
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        );
    }
    if output.stdout.truncated {
        bail!(
            "{} changed-files output exceeded {} bytes",
            vcs_root.vcs.command_name(),
            STDOUT_LIMIT
        );
    }

    let changes = match vcs_root.vcs {
        Vcs::Arc => restore_arc_repo_paths(
            parse_arc_name_status(&output.stdout.bytes)?,
            scope.as_deref(),
        )?,
        Vcs::Git => parse_git_name_status(&output.stdout.bytes)?,
    };
    let changes = filter_scope(changes, scope.as_deref());
    let result = ChangedResult {
        schema_version: 1,
        vcs: vcs_root.vcs,
        base,
        head: "HEAD".to_string(),
        scope,
        changes,
    };
    let rendered = render_result(&result, format)?;

    std::io::stdout()
        .lock()
        .write_all(rendered.as_bytes())
        .context("failed to write changed-files output")?;

    if verbose {
        eprintln!(
            "changed: {} change(s) in {}ms",
            result.changes.len(),
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

pub(crate) fn detect_vcs_compat(invocation_cwd: &Path) -> &'static str {
    discover_vcs_root(invocation_cwd)
        .map(|root| root.vcs.command_name())
        .unwrap_or("git")
}

pub(crate) fn detect_git_default_branch_compat(root: &Path) -> &'static str {
    let executable = OsStr::new("git");
    let deadline = Deadline::new(Duration::from_secs(30));
    let symbolic_args = os_args(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]);
    if let Ok(output) = run_bounded(executable, &symbolic_args, root, deadline, false) {
        if output.status.success() && !output.stdout.truncated {
            if let Ok(base) = parse_utf8(&output.stdout.bytes, "git symbolic-ref output") {
                return match base.trim_end_matches(['\r', '\n']) {
                    "origin/main" => "origin/main",
                    "origin/master" => "origin/master",
                    "origin/trunk" => "origin/trunk",
                    "origin/develop" => "origin/develop",
                    _ => "origin/main",
                };
            }
        }
    }

    for candidate in ["origin/main", "origin/master", "origin/trunk"] {
        let args = vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from(candidate),
        ];
        if let Ok(output) = run_bounded(executable, &args, root, deadline, false) {
            if output.status.success() {
                return candidate;
            }
        }
    }
    "origin/main"
}

fn discover_vcs_root(invocation_cwd: &Path) -> Result<VcsRoot> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|path| path.canonicalize().unwrap_or(path));
    for ancestor in invocation_cwd.ancestors() {
        let is_home = home.as_deref() == Some(ancestor);
        if !is_home && is_arc_root(ancestor) {
            return Ok(VcsRoot {
                vcs: Vcs::Arc,
                path: ancestor.to_path_buf(),
            });
        }
        if is_git_root(ancestor) {
            return Ok(VcsRoot {
                vcs: Vcs::Git,
                path: ancestor.to_path_buf(),
            });
        }
    }
    bail!(
        "no Git or Arc working tree found from {}",
        invocation_cwd.display()
    )
}

fn is_arc_root(path: &Path) -> bool {
    path.join(".arc").join("HEAD").is_file() || path.join(".arcconfig").is_file()
}

fn is_git_root(path: &Path) -> bool {
    let marker = path.join(".git");
    marker.is_dir() || marker.is_file()
}

fn validate_base(base: &str) -> Result<()> {
    if base.is_empty() || base.trim().is_empty() {
        bail!("--base must not be empty");
    }
    if base.starts_with('-') {
        bail!("--base must not start with '-'");
    }
    if base.chars().any(char::is_control) {
        bail!("--base must not contain control characters");
    }
    Ok(())
}

fn invocation_scope(invocation_cwd: &Path, vcs_root: &Path) -> Result<Option<String>> {
    let relative = invocation_cwd.strip_prefix(vcs_root).with_context(|| {
        format!(
            "invocation directory {} is outside VCS root {}",
            invocation_cwd.display(),
            vcs_root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(None);
    }
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("invocation scope is not repository-relative"),
        }
    }
    Ok(Some(components.join("/")))
}

fn vcs_executable(vcs: Vcs) -> OsString {
    std::env::var_os("AST_INDEX_VCS_BIN")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from(vcs.command_name()))
}

fn resolve_base(
    vcs: Vcs,
    explicit_base: Option<&str>,
    executable: &OsStr,
    vcs_root: &Path,
    deadline: Deadline,
    verbose: bool,
) -> Result<String> {
    if let Some(base) = explicit_base {
        let normalized = if vcs == Vcs::Arc {
            base.strip_prefix("origin/").unwrap_or(base)
        } else {
            base
        };
        validate_base(normalized)?;
        return Ok(normalized.to_string());
    }

    if vcs == Vcs::Arc {
        return Ok(vcs.default_base().to_string());
    }

    resolve_git_default_base(executable, vcs_root, deadline, verbose)
}

fn resolve_git_default_base(
    executable: &OsStr,
    vcs_root: &Path,
    deadline: Deadline,
    verbose: bool,
) -> Result<String> {
    let symbolic_args = os_args(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]);
    let symbolic = run_bounded(executable, &symbolic_args, vcs_root, deadline, verbose)
        .context("failed to resolve refs/remotes/origin/HEAD")?;
    if symbolic.status.success() {
        if symbolic.stdout.truncated {
            bail!("git symbolic-ref output exceeded {STDOUT_LIMIT} bytes");
        }
        let base = parse_utf8(&symbolic.stdout.bytes, "git symbolic-ref output")?
            .trim_end_matches(['\r', '\n']);
        validate_base(base)?;
        return Ok(base.to_string());
    }

    for candidate in ["origin/main", "origin/master", "main", "master", "trunk"] {
        let commit = format!("{candidate}^{{commit}}");
        let args = vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(commit),
        ];
        let output = run_bounded(executable, &args, vcs_root, deadline, verbose)
            .with_context(|| format!("failed to check Git base {candidate}"))?;
        if output.status.success() {
            return Ok(candidate.to_string());
        }
    }

    bail!(
        "could not determine Git base branch (tried origin/HEAD, origin/main, origin/master, main, master, trunk)"
    )
}

fn os_args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn diff_args(vcs: Vcs, base: &str, scope: Option<&str>) -> Vec<OsString> {
    match vcs {
        Vcs::Git => {
            let mut args = os_args(&[
                "diff",
                "--merge-base",
                "--name-status",
                "-z",
                "-M",
                "--no-ext-diff",
                "--no-textconv",
                base,
                "HEAD",
            ]);
            if let Some(scope) = scope {
                args.push(OsString::from("--"));
                args.push(OsString::from(scope));
            }
            args
        }
        Vcs::Arc => {
            let mut args: Vec<OsString> = ["diff", "-B", "--name-status", "--no-color"]
                .into_iter()
                .map(OsString::from)
                .collect();
            if scope.is_some() {
                args.push(OsString::from("--relative=."));
            }
            if base != Vcs::Arc.default_base() {
                args.push(OsString::from(base));
                args.push(OsString::from("HEAD"));
            }
            args
        }
    }
}

fn run_bounded(
    executable: &OsStr,
    args: &[OsString],
    current_dir: &Path,
    deadline: Deadline,
    verbose: bool,
) -> Result<ProcessOutput> {
    if deadline.expired() {
        return Err(deadline.timeout_error());
    }
    if verbose {
        eprintln!(
            "changed: cwd={:?} executable={:?} argv={:?}",
            current_dir, executable, args
        );
    }

    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut process_tree) = spawn_managed(&mut command, executable)?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture VCS stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture VCS stderr"))?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, STDOUT_LIMIT));
    let stderr_reader = thread::spawn(move || read_capped(stderr, STDERR_LIMIT));

    let mut status = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(found) => status = found,
                Err(error) => {
                    process_tree.terminate(&mut child);
                    drop(stdout_reader);
                    drop(stderr_reader);
                    return Err(error).context("failed while waiting for VCS command");
                }
            }
        }
        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break;
        }
        if deadline.expired() {
            process_tree.terminate(&mut child);
            drop(stdout_reader);
            drop(stderr_reader);
            return Err(deadline.timeout_error());
        }
        thread::sleep(POLL_INTERVAL.min(deadline.remaining()));
    }

    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    Ok(ProcessOutput {
        status: status.ok_or_else(|| anyhow!("VCS process ended without an exit status"))?,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn configure_process(command: &mut Command) {
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process(command: &mut Command) {
    command.creation_flags(windows_job::CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure_process(_command: &mut Command) {}

fn spawn_managed(
    command: &mut Command,
    executable: &OsStr,
) -> Result<(std::process::Child, ProcessTree)> {
    configure_process(command);

    #[cfg(windows)]
    {
        let job = windows_job::Job::new().context("could not create VCS process job")?;
        let mut child = command
            .spawn()
            .with_context(|| format!("could not start {}", Path::new(executable).display()))?;
        if let Err(error) = job.assign_and_resume(&child) {
            job.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("could not attach suspended VCS process to job");
        }
        return Ok((child, ProcessTree { job }));
    }

    #[cfg(not(windows))]
    {
        let child = command
            .spawn()
            .with_context(|| format!("could not start {}", Path::new(executable).display()))?;
        let process_tree = ProcessTree::for_child(&child);
        Ok((child, process_tree))
    }
}

struct ProcessTree {
    #[cfg(unix)]
    process_group: libc::pid_t,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl ProcessTree {
    #[cfg(not(windows))]
    fn for_child(child: &std::process::Child) -> Self {
        Self {
            #[cfg(unix)]
            process_group: child.id() as libc::pid_t,
        }
    }

    fn terminate(&mut self, child: &mut std::process::Child) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;

    pub(super) const CREATE_SUSPENDED: Dword = 0x0000_0004;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: Dword = 0x0000_2000;
    const TH32CS_SNAPTHREAD: Dword = 0x0000_0004;
    const THREAD_SUSPEND_RESUME: Dword = 0x0000_0002;
    const INVALID_HANDLE_VALUE: Handle = -1_isize as Handle;
    const INVALID_RESUME_COUNT: Dword = Dword::MAX;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: Dword,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: Dword,
        affinity: usize,
        priority_class: Dword,
        scheduling_class: Dword,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry32 {
        size: Dword,
        usage_count: Dword,
        thread_id: Dword,
        owner_process_id: Dword,
        base_priority: i32,
        priority_delta: i32,
        flags: Dword,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            information_class: i32,
            information: *const c_void,
            information_length: Dword,
        ) -> Bool;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        fn TerminateJobObject(job: Handle, exit_code: Dword) -> Bool;
        fn CreateToolhelp32Snapshot(flags: Dword, process_id: Dword) -> Handle;
        fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
        fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
        fn OpenThread(desired_access: Dword, inherit_handle: Bool, thread_id: Dword) -> Handle;
        fn ResumeThread(thread: Handle) -> Dword;
        fn CloseHandle(object: Handle) -> Bool;
    }

    pub(super) struct Job(Handle);

    impl Job {
        pub(super) fn new() -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits = ExtendedLimitInformation::default();
            limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    job.0,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    &limits as *const _ as *const c_void,
                    size_of::<ExtendedLimitInformation>() as Dword,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            resume_primary_thread(child.id())
        }

        pub(super) fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    fn resume_primary_thread(process_id: Dword) -> io::Result<()> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry = ThreadEntry32 {
            size: size_of::<ThreadEntry32>() as Dword,
            ..ThreadEntry32::default()
        };
        if unsafe { Thread32First(snapshot.0, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        loop {
            if entry.owner_process_id == process_id {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.thread_id) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = OwnedHandle(thread);
                if unsafe { ResumeThread(thread.0) } == INVALID_RESUME_COUNT {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            if unsafe { Thread32Next(snapshot.0, &mut entry) } == 0 {
                let error = io::Error::last_os_error();
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("could not find primary thread for process {process_id}: {error}"),
                ));
            }
        }
    }

    struct OwnedHandle(Handle);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn raw_ffi_layout_and_flags_match_windows_abi() {
            assert_eq!(CREATE_SUSPENDED, 4);
            assert_eq!(TH32CS_SNAPTHREAD, 4);
            assert_eq!(THREAD_SUSPEND_RESUME, 2);
            assert_eq!(size_of::<ThreadEntry32>(), 28);
            assert!(size_of::<ExtendedLimitInformation>() <= Dword::MAX as usize);
        }
    }
}

fn read_capped(mut reader: impl Read, limit: usize) -> std::io::Result<Captured> {
    let mut stored = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(stored.len());
        let kept = count.min(remaining);
        stored.extend_from_slice(&buffer[..kept]);
        truncated |= kept < count;
    }
    Ok(Captured {
        bytes: stored,
        truncated,
    })
}

fn join_reader(
    handle: thread::JoinHandle<std::io::Result<Captured>>,
    stream: &str,
) -> Result<Captured> {
    handle
        .join()
        .map_err(|_| anyhow!("VCS {stream} reader panicked"))?
        .with_context(|| format!("failed to read VCS {stream}"))
}

fn render_stderr(stderr: &Captured) -> String {
    let mut rendered = String::from_utf8_lossy(&stderr.bytes).trim().to_string();
    if stderr.truncated {
        rendered.push_str(" [stderr truncated]");
    }
    rendered
}

fn parse_git_name_status(output: &[u8]) -> Result<Vec<ChangedFile>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    if output.last() != Some(&0) {
        bail!("git returned malformed name-status output without a trailing NUL");
    }
    let fields: Vec<&[u8]> = output[..output.len() - 1]
        .split(|byte| *byte == 0)
        .collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = parse_utf8(fields[index], "git status")?;
        index += 1;
        let parsed_status = classify_status(status)?;
        match parsed_status {
            ParsedStatus::Added | ParsedStatus::Modified | ParsedStatus::Deleted => {
                let path = fields
                    .get(index)
                    .ok_or_else(|| anyhow!("git omitted a path after status {status}"))?;
                index += 1;
                changes.push(single_path_change(
                    parsed_status,
                    parse_utf8(path, "git path")?,
                )?);
            }
            ParsedStatus::Renamed | ParsedStatus::Copied => {
                let old_path = fields
                    .get(index)
                    .ok_or_else(|| anyhow!("git omitted the old path after status {status}"))?;
                let new_path = fields
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("git omitted the new path after status {status}"))?;
                index += 2;
                changes.push(two_path_change(
                    parsed_status,
                    parse_utf8(old_path, "git old path")?,
                    parse_utf8(new_path, "git new path")?,
                )?);
            }
        }
    }
    Ok(changes)
}

fn parse_arc_name_status(output: &[u8]) -> Result<Vec<ChangedFile>> {
    let output = std::str::from_utf8(output).context("arc name-status output is not UTF-8")?;
    let mut changes = Vec::new();
    for (line_index, raw_line) in output.lines().enumerate() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (status, paths) = line
            .split_once('\t')
            .ok_or_else(|| anyhow!("arc returned malformed name-status line {}", line_index + 1))?;
        let parsed_status = classify_status(status)?;
        match parsed_status {
            ParsedStatus::Added | ParsedStatus::Modified | ParsedStatus::Deleted => {
                changes.push(single_path_change(parsed_status, paths)?);
            }
            ParsedStatus::Renamed | ParsedStatus::Copied => {
                let (old_path, new_path) = paths.split_once('\t').ok_or_else(|| {
                    anyhow!(
                        "arc omitted the new path after status {status} on line {}",
                        line_index + 1
                    )
                })?;
                changes.push(two_path_change(parsed_status, old_path, new_path)?);
            }
        }
    }
    Ok(changes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParsedStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

fn classify_status(status: &str) -> Result<ParsedStatus> {
    match status {
        "A" => Ok(ParsedStatus::Added),
        "M" | "T" => Ok(ParsedStatus::Modified),
        "D" => Ok(ParsedStatus::Deleted),
        "R" | "C" => Ok(if status == "R" {
            ParsedStatus::Renamed
        } else {
            ParsedStatus::Copied
        }),
        value if value.starts_with('R') && score_is_valid(&value[1..]) => Ok(ParsedStatus::Renamed),
        value if value.starts_with('C') && score_is_valid(&value[1..]) => Ok(ParsedStatus::Copied),
        value if value.contains('U') => bail!("unresolved VCS status '{value}'"),
        value => bail!("unsupported VCS status '{value}'"),
    }
}

fn score_is_valid(score: &str) -> bool {
    !score.is_empty() && score.len() <= 3 && score.bytes().all(|byte| byte.is_ascii_digit())
}

fn single_path_change(status: ParsedStatus, path: &str) -> Result<ChangedFile> {
    let path = validate_repo_path(path)?;
    let status = match status {
        ParsedStatus::Added => ChangeStatus::A,
        ParsedStatus::Modified => ChangeStatus::M,
        ParsedStatus::Deleted => ChangeStatus::D,
        ParsedStatus::Renamed | ParsedStatus::Copied => {
            bail!("internal error: two-path status used as a single-path change")
        }
    };
    Ok(ChangedFile {
        status,
        path,
        old_path: None,
    })
}

fn two_path_change(status: ParsedStatus, old_path: &str, new_path: &str) -> Result<ChangedFile> {
    let old_path = validate_repo_path(old_path)?;
    let path = validate_repo_path(new_path)?;
    match status {
        ParsedStatus::Renamed => Ok(ChangedFile {
            status: ChangeStatus::R,
            path,
            old_path: Some(old_path),
        }),
        ParsedStatus::Copied => Ok(ChangedFile {
            status: ChangeStatus::A,
            path,
            old_path: None,
        }),
        _ => bail!("internal error: single-path status used as a two-path change"),
    }
}

fn validate_repo_path(path: &str) -> Result<String> {
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() {
        bail!("VCS returned an empty path");
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("VCS returned a non-repository-relative path '{path}'");
    }
    Ok(path.to_string())
}

fn parse_utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))
}

fn filter_scope(changes: Vec<ChangedFile>, scope: Option<&str>) -> Vec<ChangedFile> {
    let Some(scope) = scope else {
        return changes;
    };
    changes
        .into_iter()
        .filter(|change| {
            path_is_in_scope(&change.path, scope)
                || change
                    .old_path
                    .as_deref()
                    .is_some_and(|path| path_is_in_scope(path, scope))
        })
        .collect()
}

fn restore_arc_repo_paths(
    mut changes: Vec<ChangedFile>,
    scope: Option<&str>,
) -> Result<Vec<ChangedFile>> {
    let Some(scope) = scope else {
        return Ok(changes);
    };
    for change in &mut changes {
        change.path = validate_repo_path(&format!("{scope}/{}", change.path))?;
        if let Some(old_path) = &mut change.old_path {
            *old_path = validate_repo_path(&format!("{scope}/{old_path}"))?;
        }
    }
    Ok(changes)
}

fn path_is_in_scope(path: &str, scope: &str) -> bool {
    path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn render_result(result: &ChangedResult, format: &str) -> Result<String> {
    if format == "json" {
        let mut rendered = serde_json::to_string_pretty(result)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut rendered = format!(
        "Changed files against {} ({}):\n",
        result.base,
        result.changes.len()
    );
    for change in &result.changes {
        match change.status {
            ChangeStatus::R => {
                let old_path = change
                    .old_path
                    .as_deref()
                    .ok_or_else(|| anyhow!("rename is missing old_path"))?;
                rendered.push_str(&format!(
                    "  R  {} -> {}\n",
                    escape_text_path(old_path),
                    escape_text_path(&change.path)
                ));
            }
            ChangeStatus::A => {
                rendered.push_str(&format!("  A  {}\n", escape_text_path(&change.path)))
            }
            ChangeStatus::M => {
                rendered.push_str(&format!("  M  {}\n", escape_text_path(&change.path)))
            }
            ChangeStatus::D => {
                rendered.push_str(&format!("  D  {}\n", escape_text_path(&change.path)))
            }
        }
    }
    Ok(rendered)
}

fn escape_text_path(path: &str) -> String {
    path.chars().flat_map(char::escape_debug).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_argv_uses_one_merge_base_diff() {
        assert_eq!(
            diff_args(Vcs::Git, "origin/main", Some("nested")),
            [
                "diff",
                "--merge-base",
                "--name-status",
                "-z",
                "-M",
                "--no-ext-diff",
                "--no-textconv",
                "origin/main",
                "HEAD",
                "--",
                "nested",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn arc_argv_uses_native_branch_diff() {
        assert_eq!(
            diff_args(Vcs::Arc, "trunk", None),
            ["diff", "-B", "--name-status", "--no-color"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            diff_args(Vcs::Arc, "feature-base", Some("nested")),
            [
                "diff",
                "-B",
                "--name-status",
                "--no-color",
                "--relative=.",
                "feature-base",
                "HEAD",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn git_parser_normalizes_supported_statuses() {
        let parsed = parse_git_name_status(
            b"A\0a.rs\0M\0m.rs\0T\0t.rs\0D\0d.rs\0R087\0old.rs\0new.rs\0C100\0source.rs\0copy.rs\0",
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                ChangedFile {
                    status: ChangeStatus::A,
                    path: "a.rs".into(),
                    old_path: None,
                },
                ChangedFile {
                    status: ChangeStatus::M,
                    path: "m.rs".into(),
                    old_path: None,
                },
                ChangedFile {
                    status: ChangeStatus::M,
                    path: "t.rs".into(),
                    old_path: None,
                },
                ChangedFile {
                    status: ChangeStatus::D,
                    path: "d.rs".into(),
                    old_path: None,
                },
                ChangedFile {
                    status: ChangeStatus::R,
                    path: "new.rs".into(),
                    old_path: Some("old.rs".into()),
                },
                ChangedFile {
                    status: ChangeStatus::A,
                    path: "copy.rs".into(),
                    old_path: None,
                },
            ]
        );
    }

    #[test]
    fn arc_parser_preserves_spaces_and_extra_tabs_in_single_paths() {
        let parsed = parse_arc_name_status(
            b"M\tpath with spaces.rs\nA\tpath\twith\ttabs.rs\nR100\told name.rs\tnew name.rs\n",
        )
        .unwrap();
        assert_eq!(parsed[0].path, "path with spaces.rs");
        assert_eq!(parsed[1].path, "path\twith\ttabs.rs");
        assert_eq!(parsed[2].old_path.as_deref(), Some("old name.rs"));
        assert_eq!(parsed[2].path, "new name.rs");
    }

    #[test]
    fn unresolved_and_unknown_statuses_are_errors() {
        assert!(parse_git_name_status(b"U\0conflict.rs\0").is_err());
        assert!(parse_arc_name_status(b"X\tmystery.rs\n").is_err());
    }

    #[test]
    fn base_validation_rejects_unsafe_values() {
        assert!(validate_base("").is_err());
        assert!(validate_base(" \t").is_err());
        assert!(validate_base("-option").is_err());
        assert!(validate_base("main\nother").is_err());
        assert!(validate_base("origin/main").is_ok());
    }

    #[test]
    fn scope_includes_renames_crossing_the_boundary() {
        let changes = vec![
            ChangedFile {
                status: ChangeStatus::M,
                path: "other/file.rs".into(),
                old_path: None,
            },
            ChangedFile {
                status: ChangeStatus::R,
                path: "other/new.rs".into(),
                old_path: Some("nested/old.rs".into()),
            },
        ];
        let filtered = filter_scope(changes, Some("nested"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, ChangeStatus::R);
    }
}
