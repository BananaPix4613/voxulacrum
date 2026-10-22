//! CPU time the whole process has used, across every thread.
//!
//! The frame metric counts only the schedule thread's work, so work that moves
//! onto the worker pool would look free there. Process CPU seconds per wall
//! second catch it.
//!
//! Read through `GetProcessTimes` on Windows, the platform the baseline is
//! recorded on. Other platforms report none rather than pulling in a dependency
//! before a reading is taken on one.

/// User plus kernel CPU seconds used by this process so far, or `None` where
/// the platform's figure isn't read.
#[cfg(windows)]
pub fn process_cpu_seconds() -> Option<f64> {
    use std::ffi::c_void;

    /// `FILETIME`: a count of 100 ns intervals, split into two halves.
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn seconds(&self) -> f64 {
            ((u64::from(self.high) << 32) | u64::from(self.low)) as f64 * 1e-7
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    let (mut creation, mut exit, mut kernel, mut user) =
        (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing,
    // and the four out-pointers are live, writable `FILETIME`s.
    let ok = unsafe {
        GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user)
    };
    (ok != 0).then(|| kernel.seconds() + user.seconds())
}

/// User plus kernel CPU seconds used by this process so far, or `None` where
/// the platform's figure isn't read.
#[cfg(not(windows))]
pub fn process_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn process_cpu_time_advances_with_work() {
        let Some(before) = process_cpu_seconds() else {
            assert!(!cfg!(windows), "process CPU time is read on Windows");
            return;
        };
        // Spin well past the ~16 ms granularity of Windows' CPU accounting.
        let start = Instant::now();
        let mut x = 1u64;
        while start.elapsed() < Duration::from_millis(100) {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let after = process_cpu_seconds().expect("read once, so readable again");
        assert!(after > before, "{before} s before 100 ms of work, {after} s after");
    }
}
