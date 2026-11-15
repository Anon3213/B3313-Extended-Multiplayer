//! Crashes Rust cannot catch, such as an access violation in a graphics driver or a stack
//! overflow. Windows gives these to the process's unhandled-exception filter, on the failing
//! thread, which may have no stack left; so a thread started beforehand writes the minidump
//! while the failing one waits. Then, unless its stack ran out, the failing thread takes its own
//! stack trace, and the writer adds the report.

use super::report::{What, describe_exception};
use std::backtrace::Backtrace;
use std::fs::File;
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal,
    MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
    MiniDumpWriteDump, RaiseException, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};

/// Lets Windows go on to end the process, as it would without the filter.
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const STATUS_ACCESS_VIOLATION: u32 = 0xC000_0005;
const STATUS_STACK_OVERFLOW: u32 = 0xC000_00FD;
/// How long either thread waits on the other before giving up; writing a minidump of a large
/// process takes a few seconds at most.
const PATIENCE: Duration = Duration::from_secs(20);

/// Where reporting a crash has got to, as the two threads hand it between them.
const IDLE: u8 = 0;
const CRASHED: u8 = 1;
const DUMPED: u8 = 2;
const TRACED: u8 = 3;
const REPORTED: u8 = 4;

static STEP: AtomicU8 = AtomicU8::new(IDLE);
static WRITER: OnceLock<Thread> = OnceLock::new();
/// The failing thread's exception, valid while it waits in the filter.
static POINTERS: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// The failing thread's name and stack trace, if it could take them.
static TRACE: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn install() {
    let started = thread::Builder::new()
        .name("crash reporter".to_owned())
        .spawn(report_when_crashed);
    let Ok(writer) = started else {
        return;
    };
    if WRITER.set(writer.thread().clone()).is_err() {
        return;
    }
    // SAFETY: the filter is a function of the program's, valid for its whole life; replacing
    // the previous filter, Windows' default, only stops Windows reporting the crash first.
    unsafe { SetUnhandledExceptionFilter(Some(on_unhandled_exception)) };
}

/// Runs on the failing thread, with whatever stack it has left.
extern "system" fn on_unhandled_exception(pointers: *const EXCEPTION_POINTERS) -> i32 {
    // Only the first crash is reported: one while reporting must not wait on itself.
    let first = STEP
        .compare_exchange(IDLE, CRASHED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let Some(writer) = WRITER.get().filter(|_| first) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    POINTERS.store(pointers.cast_mut(), Ordering::SeqCst);
    // SAFETY: GetCurrentThreadId has no preconditions.
    THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    writer.unpark();
    if !wait_for(DUMPED, || thread::sleep(Duration::from_millis(10))) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // With the minidump safe, a stack trace, which needs the stack and heap a stack overflow
    // leaves without room.
    // SAFETY: Windows passes the filter valid exception pointers, kept until it returns.
    let exception = unsafe { read_exception(pointers) };
    if exception.as_ref().map(|exception| exception.0) != Some(STATUS_STACK_OVERFLOW) {
        let trace = (super::thread_name(), Backtrace::force_capture().to_string());
        if let Some(mut slot) = super::try_lock(&TRACE) {
            *slot = Some(trace);
        }
    }
    STEP.store(TRACED, Ordering::SeqCst);
    writer.unpark();
    wait_for(REPORTED, || thread::sleep(Duration::from_millis(10)));
    EXCEPTION_CONTINUE_SEARCH
}

/// Waits, as `pause` does, until reporting reaches `step`; false if it took too long.
fn wait_for(step: u8, pause: impl Fn()) -> bool {
    let started = Instant::now();
    while STEP.load(Ordering::SeqCst) < step {
        if started.elapsed() > PATIENCE {
            return false;
        }
        pause();
    }
    true
}

/// The crash reporter thread: waits for a crash, then writes its minidump and report.
fn report_when_crashed() {
    while STEP.load(Ordering::SeqCst) < CRASHED {
        thread::park();
    }
    let pointers = POINTERS.load(Ordering::SeqCst);
    let thread_id = THREAD_ID.load(Ordering::SeqCst);
    // SAFETY: the failing thread waits in the filter, keeping these valid, until this thread
    // says it has reported, or for longer than this thread takes.
    let exception = unsafe { read_exception(pointers) };
    let path = super::report_path(super::try_lock(&super::WRITTEN).as_ref());
    let dump = path.as_ref().map(|path| {
        let path = path.with_extension("dmp");
        write_minidump(&path, pointers, thread_id).map(|()| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    });
    STEP.store(DUMPED, Ordering::SeqCst);
    wait_for(TRACED, || thread::park_timeout(Duration::from_millis(10)));
    let (thread, backtrace) = match super::try_lock(&TRACE).and_then(|mut slot| slot.take()) {
        Some((name, backtrace)) => (name, Some(backtrace)),
        None => (thread_id.to_string(), None),
    };
    let exception = exception.map_or_else(
        || "an exception Windows gave no details of".to_owned(),
        |(code, address, information)| describe_exception(code, address, &information),
    );
    let dump = dump
        .as_ref()
        .map(|dump| dump.as_ref().map(String::as_str).map_err(String::as_str));
    super::save(
        &What::Native { thread, exception },
        backtrace.as_deref(),
        dump,
        path,
    );
    STEP.store(REPORTED, Ordering::SeqCst);
}

/// The exception's code, address, and further values, which for an access violation say what
/// was tried and where.
///
/// # Safety
/// `pointers` must be null or point to valid exception pointers, as Windows gives the filter,
/// for the duration of the call.
unsafe fn read_exception(pointers: *const EXCEPTION_POINTERS) -> Option<(u32, usize, Vec<usize>)> {
    // SAFETY: the caller promises `pointers` is null or valid.
    let pointers = unsafe { pointers.as_ref() }?;
    // SAFETY: valid exception pointers point to a valid exception record, or none.
    let record = unsafe { pointers.ExceptionRecord.as_ref() }?;
    let count = usize::try_from(record.NumberParameters).unwrap_or(0);
    let information = record
        .ExceptionInformation
        .get(..count)
        .unwrap_or_default()
        .to_vec();
    Some((
        record.ExceptionCode.0.cast_unsigned(),
        record.ExceptionAddress.addr(),
        information,
    ))
}

fn write_minidump(
    path: &Path,
    pointers: *mut EXCEPTION_POINTERS,
    thread_id: u32,
) -> Result<(), String> {
    // The first crash comes before any report has made the folder.
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let file = File::create(path).map_err(|error| error.to_string())?;
    let exception = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: thread_id,
        ExceptionPointers: pointers,
        ClientPointers: false.into(),
    };
    // Every thread's stack and registers, the modules loaded, and the memory the stacks point
    // to, which shows what the failing code was working on: a few megabytes.
    let kind = MiniDumpNormal
        | MiniDumpWithThreadInfo
        | MiniDumpWithUnloadedModules
        | MiniDumpWithIndirectlyReferencedMemory;
    // SAFETY: GetCurrentProcess returns this process's pseudo-handle; it has no preconditions.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `process` is this process; the file's handle is open for writing, as `file`
    // outlives the call; `exception` is a valid record, pointing to exception pointers in this
    // process that stay valid while the failing thread waits in the filter.
    let written = unsafe {
        MiniDumpWriteDump(
            process,
            std::process::id(),
            HANDLE(file.as_raw_handle()),
            kind,
            Some(&raw const exception),
            None,
            None,
        )
    };
    written.map_err(|error| error.message())
}

/// Raises an access violation, reading address 0x10, for the crash test.
pub fn raise_access_violation() {
    // SAFETY: RaiseException only raises the exception described; reading address 0x10 is only
    // what it says happened. No handler takes it, so it reaches the filter and ends the process.
    unsafe { RaiseException(STATUS_ACCESS_VIOLATION, 0, Some(&[0, 0x10])) };
}
