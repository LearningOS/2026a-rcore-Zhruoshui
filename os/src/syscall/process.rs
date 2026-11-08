//! Process management syscalls
use crate::mm::{translated_byte_buffer, MapPermission, VirtAddr};
use crate::task::{
    change_program_brk, exit_current_and_run_next, suspend_current_and_run_next,
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
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
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let tv = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    // the TimeVal may be split across two pages, so translate page by page
    let token = crate::task::current_user_token();
    let buf = translated_byte_buffer(token, ts as *const u8, core::mem::size_of::<TimeVal>());
    let mut iter = tv.sec.to_le_bytes().into_iter().chain(tv.usec.to_le_bytes());
    for slice in buf {
        for byte in slice.iter_mut() {
            if let Some(b) = iter.next() {
                *byte = b;
            }
        }
    }
    0
}

/// trace syscall: read/write a byte of the current app's memory, or query
/// how many times a syscall has been issued by the current task
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        0 => {
            // read a byte from the user address `id`
            match translate_user_addr(id, false) {
                Some(pa) => unsafe { core::ptr::read_volatile(pa as *const u8) as isize },
                None => -1,
            }
        }
        1 => {
            // write byte `data` to the user address `id`
            match translate_user_addr(id, true) {
                Some(pa) => {
                    unsafe { core::ptr::write_volatile(pa as *mut u8, data as u8) };
                    0
                }
                None => -1,
            }
        }
        2 => {
            // query the syscall counter of syscall `id`
            crate::task::current_syscall_times(id).map(|t| t as isize).unwrap_or(-1)
        }
        _ => -1,
    }
}

/// translate a user virtual address through the current task's page table.
/// when `need_write`, the page must also be writable; return None on failure
fn translate_user_addr(va: usize, need_write: bool) -> Option<usize> {
    // only canonical low-half Sv39 addresses belong to the user; note that
    // VirtAddr::from() silently truncates a VA to 39 bits, so e.g. isize::MAX
    // would otherwise alias the trampoline page and look "mapped"
    if va >= 1usize << 39 {
        return None;
    }
    let token = crate::task::current_user_token();
    let pt = crate::mm::PageTable::from_token(token);
    let pte = pt.translate(VirtAddr::from(va).floor())?;
    if !pte.is_valid() {
        return None;
    }
    if need_write && !pte.writable() {
        return None;
    }
    if !need_write && !pte.readable() {
        return None;
    }
    let pa: usize = usize::from(crate::mm::PhysAddr::from(pte.ppn())) | (va & (crate::config::PAGE_SIZE - 1));
    Some(pa)
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap");
    // start must be page aligned, len must be positive
    if start % crate::config::PAGE_SIZE != 0 || len == 0 {
        return -1;
    }
    // port may only contain R(1) W(2) X(4), and must not be zero
    if port & !0x7 != 0 || port == 0 {
        return -1;
    }
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    // the target range must not overlap any existing mapping
    if !crate::task::current_area_is_free(start_va, end_va) {
        return -1;
    }
    let map_perm = MapPermission::from_bits(((port & 0x7) << 1) as u8).unwrap() | MapPermission::U;
    crate::task::insert_current_framed_area(start_va, end_va, map_perm);
    0
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    if start % crate::config::PAGE_SIZE != 0 || len == 0 {
        return -1;
    }
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    if crate::task::munmap_current(start_va, end_va) {
        0
    } else {
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
