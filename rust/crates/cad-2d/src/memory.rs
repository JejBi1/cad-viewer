//! Memory the computer has free, and what a scene may take of it.
//!
//! A scene is refused at `MAX_OUT_VERTS` output vertices (points of segments and corners of fill triangles). A drawing of
//! 2 KB (three dashed circles of a billion units, a text of 1.6 million characters, forty hatches of a pattern that is too
//! fine) used to reach the old limit of 300 million of them and took 5 to 7.5 GB, more than an 8 GB computer has. The limit
//! is therefore a fixed ceiling that suits such a computer, and lower still when less memory is free: the detail is
//! reduced first (dashes drawn solid, hatches filled, the outlines of letters and hatches simplified, and as the last level
//! a box of a few vertices instead of them, `scene::Lod`), the scene is refused last: only when what cannot be reduced
//! (strokes) and the boxes of everything else do not fit. Texts and fills therefore always have a drawing, at every size
//! of memory; strokes (lines, circles, arcs) are never made coarser.
use crate::scene::{MAX_OUT_VERTS, SOFT_OUT_VERTS};

/// Bytes one output vertex takes at the peak of making a scene: 8 for the batch (two `f32`), 12 on the graphics card, the
/// share of the recorded path elements (17 bytes each, one element per vertex for lines), the staging copy of one buffer.
/// Measured in the program: 25 000 texts (59 million vertices) peaked at 41 bytes each, 40 hatches (63 million vertices) at
/// 54; 45 is used.
pub const BYTES_PER_OUT_VERT: u64 = 45;
/// The scene may use up to this share of the memory that is free (one over this number): the program, the drawing itself,
/// the interface and the rest of the system need memory too. Half: a computer with 4 GB free draws what takes 2 GB.
pub const FREE_MEMORY_SHARE: u64 = 2;
/// The lowest limit, however little memory is reported: a drawing of ordinary size always has to open.
pub const OUT_VERTS_FLOOR: u64 = 8_000_000;

/// Physical memory that is free (Windows: `GlobalMemoryStatusEx`, Linux: `MemAvailable` of `/proc/meminfo`); `None` when
/// the system does not say.
pub fn available_physical_bytes() -> Option<u64> {
    sys::available()
}

/// Memory this process may still take under a limit that was put on it from outside: the memory limit of the job it runs in
/// (Windows: the limit of a process or of the whole job, which is what the sandbox of the Explorer thumbnails sets, less what the
/// process has committed so far) or its limit of address space (Linux: `RLIMIT_AS` of the `.thumbnailer`, less the address
/// space in use). `None` when there is no such limit, or the system does not say. Free physical memory does not tell: the
/// limit is far below it.
pub fn process_headroom_bytes() -> Option<u64> {
    sys::headroom()
}

/// Output vertices up to which a thumbnail keeps every shape in full detail: the letters of 500 texts, and more than an image
/// of at most 1024 pixels shows of them.
pub const THUMB_SOFT_OUT_VERTS: u64 = 2_000_000;
/// The most output vertices of a thumbnail (8 million segments are eight for every pixel of the biggest thumbnail, and 720 MB at
/// the peak). The strokes of a drawing are never reduced, so a drawing above it gets no thumbnail; everything else (texts,
/// hatches) gets less detail first.
pub const THUMB_MAX_OUT_VERTS: u64 = 16_000_000;
/// The lowest limit of a thumbnail, however little memory it has left.
pub const THUMB_OUT_VERTS_FLOOR: u64 = 1_000_000;

/// (output vertices up to which the detail stays full, most output vertices) of the scene of a thumbnail. Fixed and small, for
/// an image of 1024 pixels at the most: it does not grow with the free memory of the computer (the sandbox of the thumbnails
/// limits the process to 2 GiB, a tenth of a computer that has 24 GB free, and the scene of a drawing of texts took the
/// whole of that), and `headroom` (`process_headroom_bytes`) lowers it: the scene may take half of what the process may still
/// commit, as `out_limits_for` does with the free memory.
pub fn thumbnail_out_limits(headroom: Option<u64>) -> (u64, u64) {
    let by_room = headroom.map(|h| h / FREE_MEMORY_SHARE / BYTES_PER_OUT_VERT).unwrap_or(THUMB_MAX_OUT_VERTS);
    let max = by_room.clamp(THUMB_OUT_VERTS_FLOOR, THUMB_MAX_OUT_VERTS);
    (THUMB_SOFT_OUT_VERTS.min(max / 2), max)
}

/// (output vertices up to which the detail stays full, most output vertices) for `available` bytes of free memory (`None`:
/// unknown, only the fixed ceiling counts). The hard limit is what half of the free memory holds, but never above `MAX_OUT_VERTS`, the
/// ceiling that suits an 8 GB computer, and never below `OUT_VERTS_FLOOR`; the detail is reduced from half of it, or from
/// `SOFT_OUT_VERTS`, whichever is less.
pub fn out_limits_for(available: Option<u64>) -> (u64, u64) {
    let by_memory = available.map(|a| a / FREE_MEMORY_SHARE / BYTES_PER_OUT_VERT).unwrap_or(MAX_OUT_VERTS);
    let max = by_memory.clamp(OUT_VERTS_FLOOR, MAX_OUT_VERTS);
    (SOFT_OUT_VERTS.min(max / 2), max)
}

#[cfg(windows)]
mod sys {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` (x64: 144 bytes).
    #[repr(C)]
    struct JobLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
        io_counters: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    /// `PROCESS_MEMORY_COUNTERS`.
    #[repr(C)]
    struct ProcessCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x100;
    const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x200;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn QueryInformationJobObject(job: isize, class: i32, info: *mut JobLimits, len: u32, returned: *mut u32) -> i32;
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessCounters, cb: u32) -> i32;
    }

    /// The memory limit of the job of this process and what the process has committed: what is left under the limit.
    pub fn headroom() -> Option<u64> {
        // SAFETY: plain system calls on local structs of the layouts that the system documents, with their sizes; a job handle of
        // zero means the job the calling process is in (and the call fails when there is none)
        unsafe {
            let mut j: JobLimits = std::mem::zeroed();
            let mut returned = 0u32;
            let size = std::mem::size_of::<JobLimits>() as u32;
            if QueryInformationJobObject(0, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, &mut j, size, &mut returned) == 0 {
                return None;
            }
            let mut limit = u64::MAX;
            if j.limit_flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0 {
                limit = limit.min(j.process_memory_limit as u64);
            }
            if j.limit_flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0 {
                limit = limit.min(j.job_memory_limit as u64);
            }
            if limit == u64::MAX {
                return None;
            }
            let mut c: ProcessCounters = std::mem::zeroed();
            c.cb = std::mem::size_of::<ProcessCounters>() as u32;
            let used = if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 { c.pagefile_usage as u64 } else { 0 };
            Some(limit.saturating_sub(used))
        }
    }

    pub fn available() -> Option<u64> {
        let mut m = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: `m` is a valid `MEMORYSTATUSEX` with its `dwLength` set, as the function requires.
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
        (ok != 0).then_some(m.avail_phys)
    }
}

/// The limit of address space (`RLIMIT_AS`, `ulimit -v`) of `/proc/self/limits` less the address space in use (`VmSize`) of
/// `/proc/self/status`; `None` without a limit ("unlimited") or when the texts are not what they should be.
#[cfg_attr(windows, allow(dead_code))]
fn headroom_from_proc(limits: &str, status: &str) -> Option<u64> {
    let line = limits.lines().find(|l| l.starts_with("Max address space"))?;
    // "Max address space        4294967296           4294967296           bytes": soft limit, hard limit, unit
    let soft = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    let kb: u64 = status.lines().find(|l| l.starts_with("VmSize:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(soft.saturating_sub(kb.saturating_mul(1024)))
}

#[cfg(not(windows))]
mod sys {
    /// What the process may still take under its limit of address space; `None` without a limit.
    pub fn headroom() -> Option<u64> {
        let limits = std::fs::read_to_string("/proc/self/limits").ok()?;
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        super::headroom_from_proc(&limits, &status)
    }

    pub fn available() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb.saturating_mul(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn the_limit_is_a_ceiling_for_an_8_gb_computer_and_lower_when_less_memory_is_free() {
        // the ceiling is the same on a computer with 64 GB and with an unknown amount: nothing is taken for granted
        assert_eq!(out_limits_for(None).1, MAX_OUT_VERTS);
        assert_eq!(out_limits_for(Some(64 * GB)), out_limits_for(None));
        // at the ceiling a scene takes less than 3 GB: it fits an 8 GB computer next to the system and the program
        assert!(MAX_OUT_VERTS * BYTES_PER_OUT_VERT < 3 * GB, "{}", MAX_OUT_VERTS * BYTES_PER_OUT_VERT / GB);
        // 4 GB free: half of it is 2 GB, 47 million vertices
        let (soft, max) = out_limits_for(Some(4 * GB));
        assert!((47_000_000..48_000_000).contains(&max), "{max}");
        assert_eq!(soft, max / 2);
        // 6 GB or more: the ceiling
        assert_eq!(out_limits_for(Some(6 * GB)).1, MAX_OUT_VERTS);
        // 1 GB free: 11 million vertices; 256 MB free: the floor, not a refusal of every drawing
        assert!((11_000_000..12_000_000).contains(&out_limits_for(Some(GB)).1));
        assert_eq!(out_limits_for(Some(GB / 4)).1, OUT_VERTS_FLOOR);
        assert_eq!(out_limits_for(Some(0)), (OUT_VERTS_FLOOR / 2, OUT_VERTS_FLOOR));
        // the detail is reduced before the refusal, never after: soft is always below the hard limit
        for a in [0, GB, 3 * GB, 6 * GB, 16 * GB, 128 * GB] {
            let (soft, max) = out_limits_for(Some(a));
            assert!(soft < max && soft <= SOFT_OUT_VERTS && max <= MAX_OUT_VERTS, "{a}: {soft} {max}");
        }
    }

    #[test]
    fn a_thumbnail_has_small_fixed_limits_that_the_memory_of_its_process_lowers() {
        // nothing is known (no job, no limit): the fixed numbers, whatever the memory of the computer
        assert_eq!(thumbnail_out_limits(None), (THUMB_SOFT_OUT_VERTS, THUMB_MAX_OUT_VERTS));
        const _: () = assert!(THUMB_MAX_OUT_VERTS < MAX_OUT_VERTS && THUMB_SOFT_OUT_VERTS < SOFT_OUT_VERTS);
        // far more than that: the same
        assert_eq!(thumbnail_out_limits(Some(64 * GB)), thumbnail_out_limits(None));
        // the sandbox of the Explorer thumbnails (2 GiB) after the drawing has been read (400 MB): the scene may take half of the
        // rest, which is the limit of a thumbnail, and its peak fits the job
        let (soft, max) = thumbnail_out_limits(Some(2 * GB - (400 << 20)));
        assert!(soft <= THUMB_SOFT_OUT_VERTS && max <= THUMB_MAX_OUT_VERTS && max * BYTES_PER_OUT_VERT < GB, "{soft} {max}");
        // a big drawing that has taken 1.3 GB of the 2 GiB: less, and again about half of what is left
        let room = 2 * GB - (1300 << 20);
        let (soft, max) = thumbnail_out_limits(Some(room));
        assert!((5_000_000..10_000_000).contains(&max) && soft == THUMB_SOFT_OUT_VERTS.min(max / 2), "{soft} {max}");
        assert!(max * BYTES_PER_OUT_VERT <= room / 2, "{max}");
        // nothing left: the floor, which is a refusal of the scene and not a crash of the process
        assert_eq!(thumbnail_out_limits(Some(0)), (THUMB_OUT_VERTS_FLOOR / 2, THUMB_OUT_VERTS_FLOOR));
        // the detail is reduced before the refusal, never after
        for h in [0, GB / 8, GB, 2 * GB, 16 * GB] {
            let (soft, max) = thumbnail_out_limits(Some(h));
            assert!(soft < max && soft <= THUMB_SOFT_OUT_VERTS && (THUMB_OUT_VERTS_FLOOR..=THUMB_MAX_OUT_VERTS).contains(&max), "{h}: {soft} {max}");
        }
    }

    /// The limit of address space of a `.thumbnailer` (`prlimit`, 4 GiB) less what is in use, from the files of `/proc`.
    #[test]
    fn the_headroom_is_read_from_the_proc_files_of_linux() {
        let status = "Name:\tStepViewer\nVmPeak:\t 1900000 kB\nVmSize:\t 1048576 kB\nVmRSS:\t  200000 kB\n";
        let limited = "Limit                     Soft Limit           Hard Limit           Units     \nMax cpu time              120                  125                  seconds   \nMax address space         4294967296           4294967296           bytes     \nMax locked memory         65536                65536                bytes     \n";
        assert_eq!(headroom_from_proc(limited, status), Some(3 * GB));
        // more address space in use than the limit allows: nothing is left, not a wrap-around
        let tight = limited.replace("4294967296           4294967296", "1048576              1048576   ");
        assert_eq!(headroom_from_proc(&tight, status), Some(0));
        // no limit, or texts that are not those of Linux: no headroom
        let unlimited = limited.replace("4294967296           4294967296", "unlimited            unlimited ");
        assert_eq!(headroom_from_proc(&unlimited, status), None);
        assert_eq!(headroom_from_proc("", status), None);
        assert_eq!(headroom_from_proc(limited, ""), None);
    }

    /// This test process is in no job with a memory limit (and if it is, the headroom is a sensible number).
    #[test]
    fn the_headroom_of_the_process_is_none_without_a_limit_and_a_number_with_one() {
        if let Some(h) = process_headroom_bytes() {
            assert!(h < 1 << 50, "{h}");
        }
    }

    #[test]
    fn the_system_tells_how_much_memory_is_free() {
        let a = available_physical_bytes().expect("the memory status of this system");
        assert!(a > 16 << 20 && a < 64 * 1024 * GB, "{a}");
    }
}
