//! Another process's memory, through the Win32 API.

use crate::ram::{ProcessMemory, Region};
use std::ffi::c_void;
use std::io;
use std::mem::{self, MaybeUninit};
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_GUARD,
    PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQueryEx,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_BLOCK, PSAPI_WORKING_SET_EX_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
};

const READABLE_PROTECTIONS: u32 =
    PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE;
/// What marks a page as not to be read whatever else it allows. A guard page is the end of a
/// thread's stack: the first touch of it, a read from another process among them, takes the
/// guard away, and the thread then crashes where its stack would have grown.
const UNREADABLE_PROTECTIONS: u32 = PAGE_GUARD | PAGE_NOACCESS;

/// Whether memory under `protection` may be read without changing anything for its owner.
const fn is_readable(protection: u32) -> bool {
    protection & READABLE_PROTECTIONS != 0 && protection & UNREADABLE_PROTECTIONS == 0
}
/// RDRAM is 8 MB, so regions this large only waste time and memory to scan.
const MAX_SCANNED_REGION: usize = 1 << 31;
/// The size of a page of memory, on every processor Windows runs on that this is built for.
const PAGE_SIZE: usize = 4096;
/// How many pages Windows is asked about at once: 64 MiB of them, in a list of 256 KiB.
const PAGES_PER_QUERY: usize = 1 << 14;
/// The flag, of those Windows gives for a page, that says the process has it in memory.
const PAGE_IS_VALID: usize = 1;

/// A handle to another process, closed on drop.
#[derive(Debug)]
pub struct WindowsProcess {
    handle: HANDLE,
}

impl WindowsProcess {
    /// Opens process `pid` for reading and writing its memory.
    ///
    /// # Errors
    /// Fails if the process does not exist or access is denied.
    pub fn open(pid: u32) -> io::Result<Self> {
        let access =
            PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION;
        // SAFETY: takes no pointers; failure is reported through the returned handle.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle })
    }

    /// Whether the process has ended. Its memory then reads as holding nothing at all, which
    /// alone looks like a game that has not started yet.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        let mut code = 0_u32;
        // SAFETY: `code` is valid for the one write of a `u32` this makes, and the handle was
        // opened with the right to ask.
        let ok = unsafe { GetExitCodeProcess(self.handle, &raw mut code) };
        ok != 0 && code != STILL_ACTIVE
    }
}

/// What Windows gives as the exit code of a process that is still running.
const STILL_ACTIVE: u32 = 259;

/// The emulator's program, as Parallel Launcher starts it.
pub const EMULATOR: &str = "retroarch.exe";

/// The process IDs of every running program named `exe`, as `retroarch.exe`, ignoring case.
///
/// # Errors
/// Fails if Windows cannot list the running processes.
pub fn find_processes(exe: &str) -> io::Result<Vec<u32>> {
    // SAFETY: takes no pointers; failure is reported through the returned handle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let _closing = OwnedHandle(snapshot);
    let size = u32::try_from(mem::size_of::<PROCESSENTRY32W>())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size,
        cntUsage: 0,
        th32ProcessID: 0,
        th32DefaultHeapID: 0,
        th32ModuleID: 0,
        cntThreads: 0,
        th32ParentProcessID: 0,
        pcPriClassBase: 0,
        dwFlags: 0,
        szExeFile: [0; 260],
    };
    let mut found = Vec::new();
    // SAFETY: `snapshot` is a live process snapshot and `entry` a PROCESSENTRY32W whose `dwSize`
    // gives its size, as the call requires; both outlive it.
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(entry.szExeFile.get(..length).unwrap_or_default());
        if name.eq_ignore_ascii_case(exe) {
            found.push(entry.th32ProcessID);
        }
        // SAFETY: as for `Process32FirstW` above.
        more = unsafe { Process32NextW(snapshot, &raw mut entry) } != 0;
    }
    Ok(found)
}

/// A handle closed when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is open, and closed only here. There is nothing useful to do if
        // closing fails.
        unsafe { CloseHandle(self.0) };
    }
}

impl Drop for WindowsProcess {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `OpenProcess` and is closed only here. There is nothing useful
        // to do if closing fails.
        unsafe { CloseHandle(self.handle) };
    }
}

fn whole_transfer(ok: i32, transferred: usize, expected: usize) -> io::Result<()> {
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if transferred != expected {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(())
}

impl ProcessMemory for WindowsProcess {
    fn read(&self, address: usize, buffer: &mut [u8]) -> io::Result<()> {
        let mut count = 0;
        // SAFETY: `buffer` is valid for `buffer.len()` writes, and `count` for one. The address is
        // in the other process, which the kernel checks.
        let ok = unsafe {
            ReadProcessMemory(
                self.handle,
                ptr::with_exposed_provenance(address),
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len(),
                &raw mut count,
            )
        };
        whole_transfer(ok, count, buffer.len())
    }

    fn write(&self, address: usize, data: &[u8]) -> io::Result<()> {
        let mut count = 0;
        // SAFETY: `data` is valid for `data.len()` reads, and `count` for one write. The address is
        // in the other process, which the kernel checks.
        let ok = unsafe {
            WriteProcessMemory(
                self.handle,
                ptr::with_exposed_provenance(address),
                data.as_ptr().cast::<c_void>(),
                data.len(),
                &raw mut count,
            )
        };
        whole_transfer(ok, count, data.len())
    }

    fn readable_regions(&self) -> Vec<Region> {
        let mut regions = Vec::new();
        let mut address = 0_usize;
        loop {
            let mut info = MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
            // SAFETY: `info` is valid for one write of the size passed. The address is in the other
            // process, which the kernel checks.
            let written = unsafe {
                VirtualQueryEx(
                    self.handle,
                    ptr::with_exposed_provenance(address),
                    info.as_mut_ptr(),
                    mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if written == 0 {
                return regions;
            }
            // SAFETY: a non-zero return means the kernel filled in `info`.
            let info = unsafe { info.assume_init() };
            let (base, size) = (info.BaseAddress.addr(), info.RegionSize);
            if info.State == MEM_COMMIT && is_readable(info.Protect) && size < MAX_SCANNED_REGION {
                regions.push(Region {
                    address: base,
                    size,
                });
            }
            let Some(next) = base.checked_add(size) else {
                return regions;
            };
            address = next;
        }
    }

    fn regions_in_use(&self) -> Vec<Region> {
        let mut in_use = Vec::new();
        let mut pages = Vec::new();
        for region in self.readable_regions() {
            for offset in (0..region.size).step_by(PAGES_PER_QUERY * PAGE_SIZE) {
                let start = region.address + offset;
                let length = (region.size - offset).min(PAGES_PER_QUERY * PAGE_SIZE);
                match self.pages_in_memory(start, length, &mut pages) {
                    Some(()) => {
                        let held = pages.iter().enumerate().filter(|(_, page)| {
                            // SAFETY: every form of this union is one word of flags, so any
                            // bits Windows wrote there are a valid `Flags`.
                            let flags = unsafe { page.VirtualAttributes.Flags };
                            flags & PAGE_IS_VALID != 0
                        });
                        for (index, _) in held {
                            add_run(&mut in_use, start + index * PAGE_SIZE, PAGE_SIZE);
                        }
                    }
                    // Windows would not say, so all of it is taken to be: the search then
                    // costs what it did, and misses nothing.
                    None => add_run(&mut in_use, start, length),
                }
            }
        }
        in_use
    }
}

/// Adds the range to `runs`, as part of the last one if it begins where that ends.
fn add_run(runs: &mut Vec<Region>, address: usize, size: usize) {
    match runs.last_mut() {
        Some(last) if last.address + last.size == address => last.size += size,
        _ => runs.push(Region { address, size }),
    }
}

impl WindowsProcess {
    /// Asks Windows about each page of the `length` bytes at `start`, leaving in `pages` what
    /// it says of each, in order. None if it will not say.
    fn pages_in_memory(
        &self,
        start: usize,
        length: usize,
        pages: &mut Vec<PSAPI_WORKING_SET_EX_INFORMATION>,
    ) -> Option<()> {
        pages.clear();
        pages.extend((0..length.div_ceil(PAGE_SIZE)).map(|page| {
            PSAPI_WORKING_SET_EX_INFORMATION {
                VirtualAddress: ptr::with_exposed_provenance_mut(start + page * PAGE_SIZE),
                VirtualAttributes: PSAPI_WORKING_SET_EX_BLOCK { Flags: 0 },
            }
        }));
        let bytes = u32::try_from(mem::size_of_val(pages.as_slice())).ok()?;
        // SAFETY: `pages` is valid for reads and writes of the `bytes` passed, which is its
        // whole length. The addresses in it are in the other process, and only asked about.
        let ok = unsafe { K32QueryWorkingSetEx(self.handle, pages.as_mut_ptr().cast(), bytes) };
        (ok != 0).then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PAGE_SIZE, WindowsProcess, find_processes, is_readable};
    use crate::ram::ProcessMemory;
    use windows_sys::Win32::System::Memory::{PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE};

    /// A full search reads every region found readable. Reading the guard page at the end of
    /// a thread's stack takes its guard away for good, in the emulator's own threads.
    #[test]
    fn a_stacks_guard_page_is_not_read_though_it_allows_reading() {
        assert!(is_readable(PAGE_READWRITE));
        assert!(!is_readable(PAGE_READWRITE | PAGE_GUARD));
        assert!(!is_readable(PAGE_NOACCESS));
    }

    #[test]
    fn a_running_process_has_not_exited_and_one_that_ended_has() -> std::io::Result<()> {
        assert!(!WindowsProcess::open(std::process::id())?.has_exited());

        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()?;
        // Opened while it may still run: the handle keeps the process known after it ends.
        let opened = WindowsProcess::open(child.id())?;
        child.wait()?;
        assert!(opened.has_exited());
        Ok(())
    }

    #[test]
    fn memory_set_aside_and_never_touched_is_not_in_use_and_a_touched_page_is()
    -> std::io::Result<()> {
        // Windows gives this no real memory until it is written to: it is only set aside.
        let size = 64 << 20;
        let mut set_aside = vec![0_u8; size];
        let touched = size / 2;
        if let Some(byte) = set_aside.get_mut(touched) {
            *byte = 1;
        }
        let start = set_aside.as_ptr().addr();

        let this_process = WindowsProcess::open(std::process::id())?;
        let in_use = this_process.regions_in_use();
        let overlap = |address: usize, length: usize| {
            let end = (address + length).min(start + size);
            end.saturating_sub(address.max(start))
        };
        let held: usize = in_use.iter().map(|r| overlap(r.address, r.size)).sum();
        // The page written to, and little else of the 64 MiB: a page or two at its ends that
        // the allocator shares with what is beside it.
        let page = start + touched;
        assert!(
            in_use
                .iter()
                .any(|r| (r.address..r.address + r.size).contains(&page))
        );
        assert!(held < 16 * PAGE_SIZE, "{held} bytes of {size} are in use");

        // All of it is still among the regions that can be read, as a full search needs.
        let readable = this_process.readable_regions();
        let whole: usize = readable.iter().map(|r| overlap(r.address, r.size)).sum();
        assert_eq!(whole, size);
        // Kept until here, so that it is not freed and handed to something else meanwhile.
        assert_eq!(set_aside.get(touched), Some(&1));
        Ok(())
    }

    #[test]
    fn running_programs_are_found_by_name() -> std::io::Result<()> {
        let me = std::env::current_exe()?;
        let name = me
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_uppercase();
        assert!(find_processes(&name)?.contains(&std::process::id()));
        assert_eq!(find_processes("no such program.exe")?, Vec::<u32>::new());
        Ok(())
    }
}
