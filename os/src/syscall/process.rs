//! Process management syscalls
use crate::{
    config::{MAX_SYSCALL_NUM, MEMORY_END},
    task::{exit_current_and_run_next, get_syscall_times, suspend_current_and_run_next},
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// trace syscall: read/write a byte of the current app's memory, or query
/// how many times a syscall has been issued by the current task
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        // read a byte from the app's address `id`
        0 => {
            if !check_app_addr(id) {
                return -1;
            }
            unsafe { core::ptr::read_volatile(id as *const u8) as isize }
        }
        // write byte `data` to the app's address `id`
        1 => {
            if !check_app_addr(id) {
                return -1;
            }
            unsafe { core::ptr::write_volatile(id as *mut u8, data as u8) };
            0
        }
        // query the syscall counter of syscall `id`
        2 => {
            if id >= MAX_SYSCALL_NUM {
                return -1;
            }
            get_syscall_times(id) as isize
        }
        _ => -1,
    }
}

/// whether `addr` is a readable/writable physical address in RAM.
/// In ch3 there is no address space isolation: the app's code/data live in
/// the app area, while its user stack is a static array in the kernel
/// image, so we simply allow the whole RAM range.
fn check_app_addr(addr: usize) -> bool {
    // RAM starts at 0x80000000 on the QEMU virt machine
    (0x8000_0000..MEMORY_END).contains(&addr)
}
