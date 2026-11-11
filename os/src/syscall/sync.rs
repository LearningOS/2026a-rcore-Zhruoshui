use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore, UPSafeCell};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;
use lazy_static::lazy_static;

lazy_static! {
    /// global switch of deadlock detection
    static ref DEADLOCK_DETECT_ENABLED: UPSafeCell<bool> =
        unsafe { UPSafeCell::new(false) };
}

/// get the tid of the current thread
fn current_tid() -> usize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid
}

/// whether deadlock detection is enabled
fn deadlock_detect_enabled() -> bool {
    *DEADLOCK_DETECT_ENABLED.exclusive_access()
}

/// the unified resource id of mutex `mutex_id`
fn mutex_res(mutex_id: usize) -> usize {
    mutex_id * 2
}

/// the unified resource id of semaphore `sem_id`
fn sem_res(sem_id: usize) -> usize {
    sem_id * 2 + 1
}
/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() as isize - 1
    }
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap())
    };
    if deadlock_detect_enabled() {
        let tid = current_tid();
        if mutex.is_locked() {
            // this lock would block the current thread: run detection first
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_record_wait(tid, mutex_res(mutex_id));
            if process_inner.deadlock_detect(tid) {
                process_inner.deadlock_clear_wait(tid);
                return -0xdead as isize;
            }
            drop(process_inner);
            mutex.lock();
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_clear_wait(tid);
            process_inner.deadlock_record_alloc(tid, mutex_res(mutex_id));
        } else {
            mutex.lock();
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_record_alloc(tid, mutex_res(mutex_id));
        }
    } else {
        mutex.lock();
    }
    drop(process);
    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    if deadlock_detect_enabled() {
        let tid = current_tid();
        let mut process_inner = process.inner_exclusive_access();
        process_inner.deadlock_release_alloc(tid, mutex_res(mutex_id));
    }
    let mutex = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap())
    };
    drop(process);
    mutex.unlock();
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    if deadlock_detect_enabled() {
        let tid = current_tid();
        let mut process_inner = process.inner_exclusive_access();
        process_inner.deadlock_release_alloc(tid, sem_res(sem_id));
        process_inner.deadlock_record_up(sem_res(sem_id), tid);
    }
    let sem = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    drop(process);
    sem.up();
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let sem = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    if deadlock_detect_enabled() {
        let tid = current_tid();
        if sem.inner.exclusive_access().count <= 0 {
            // this down would block the current thread: run detection first
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_record_wait(tid, sem_res(sem_id));
            if process_inner.deadlock_detect(tid) {
                process_inner.deadlock_clear_wait(tid);
                return -0xdead as isize;
            }
            drop(process_inner);
            sem.down();
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_clear_wait(tid);
            process_inner.deadlock_record_alloc(tid, sem_res(sem_id));
        } else {
            sem.down();
            let mut process_inner = process.inner_exclusive_access();
            process_inner.deadlock_record_alloc(tid, sem_res(sem_id));
        }
    } else {
        sem.down();
    }
    drop(process);
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
///
/// The detection itself runs inside sys_mutex_lock / sys_semaphore_down:
/// when the current thread is about to block, a wait-for graph is built
/// from the allocation/wait tables of the process; if the thread lies on
/// a cycle, the acquire returns -0xdead instead of blocking.
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!("kernel: sys_enable_deadlock_detect");
    *DEADLOCK_DETECT_ENABLED.exclusive_access() = enabled != 0;
    0
}
