//! Random-operation harness shared by the libFuzzer target, the native-backend driver and
//! `tests/random_ops.rs` of the main crate.
pub mod ops;

/// Resident memory (KiB) and open file descriptors (Windows: kernel handles) of this process,
/// to spot leaks across seeds. Zeros where the platform offers neither.
#[cfg(not(windows))]
pub fn resources() -> (u64, usize) {
    let rss_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<u64>().ok())
        })
        .unwrap_or(0);
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count());
    (rss_pages * 4, fds)
}

/// See the non-Windows version: working set in KiB and the process's kernel handles plus GDI and
/// USER objects (a leaked HFONT, HBRUSH, HMENU or HWND shows up there, not as a kernel handle).
#[cfg(windows)]
pub fn resources() -> (u64, usize) {
    use std::ffi::c_void;
    /// `PROCESS_MEMORY_COUNTERS` (psapi), the leading fields only.
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        rest: [usize; 6],
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32GetProcessMemoryInfo(p: *mut c_void, c: *mut Counters, cb: u32) -> i32;
        fn GetProcessHandleCount(p: *mut c_void, n: *mut u32) -> i32;
        fn GetGuiResources(p: *mut c_void, flags: u32) -> u32;
    }
    unsafe {
        let p = GetCurrentProcess();
        let mut c = Counters {
            cb: size_of::<Counters>() as u32,
            page_faults: 0,
            peak_working_set: 0,
            working_set: 0,
            rest: [0; 6],
        };
        let mut handles = 0u32;
        if K32GetProcessMemoryInfo(p, &mut c, c.cb) == 0 {
            c.working_set = 0;
        }
        if GetProcessHandleCount(p, &mut handles) == 0 {
            handles = 0;
        }
        // GR_GDIOBJECTS = 0, GR_USEROBJECTS = 1
        let gui = GetGuiResources(p, 0) + GetGuiResources(p, 1);
        let mut n = (handles + gui) as usize;
        if n == 0 {
            // Wine stubs these counters; the process is also an ordinary Linux process whose
            // descriptors (one per kernel handle) can be listed through the Z: drive
            n = std::fs::read_dir("Z:\\proc\\self\\fd").map_or(0, |d| d.count());
        }
        ((c.working_set / 1024) as u64, n)
    }
}

/// Counting global allocator for the drivers: `#[global_allocator] static A: CountingAlloc = CountingAlloc;`
/// then `live_heap_bytes()` shows whether growth of the resident size is Rust-side state (it climbs
/// here too) or memory the toolkit holds (it does not).
pub struct CountingAlloc;

static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Bytes currently allocated through [`CountingAlloc`].
pub fn live_heap_bytes() -> usize {
    LIVE.load(std::sync::atomic::Ordering::Relaxed)
}

// SAFETY: forwards every call to the system allocator unchanged and only adds bookkeeping.
unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        LIVE.fetch_add(l.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        LIVE.fetch_sub(l.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        LIVE.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        LIVE.fetch_sub(l.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(p, l, n) }
    }
}
