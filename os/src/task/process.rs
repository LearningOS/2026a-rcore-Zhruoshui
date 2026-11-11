//! Implementation of  [`ProcessControlBlock`]

use super::id::RecycleAllocator;
use super::manager::insert_into_pid2process;
use super::TaskControlBlock;
use super::{add_task, SignalFlags};
use super::{pid_alloc, PidHandle};
use crate::fs::{File, Stdin, Stdout};
use crate::mm::{translated_refmut, MemorySet, KERNEL_SPACE};
use crate::sync::{Condvar, Mutex, Semaphore, UPSafeCell};
use crate::trap::{trap_handler, TrapContext};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;

/// Process Control Block
pub struct ProcessControlBlock {
    /// immutable
    pub pid: PidHandle,
    /// mutable
    inner: UPSafeCell<ProcessControlBlockInner>,
}

/// Inner of Process Control Block
pub struct ProcessControlBlockInner {
    /// is zombie?
    pub is_zombie: bool,
    /// memory set(address space)
    pub memory_set: MemorySet,
    /// parent process
    pub parent: Option<Weak<ProcessControlBlock>>,
    /// children process
    pub children: Vec<Arc<ProcessControlBlock>>,
    /// exit code
    pub exit_code: i32,
    /// file descriptor table
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    /// signal flags
    pub signals: SignalFlags,
    /// tasks(also known as threads)
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    /// task resource allocator
    pub task_res_allocator: RecycleAllocator,
    /// mutex list
    pub mutex_list: Vec<Option<Arc<dyn Mutex>>>,
    /// semaphore list
    pub semaphore_list: Vec<Option<Arc<Semaphore>>>,
    /// condvar list
    pub condvar_list: Vec<Option<Arc<Condvar>>>,
    /// deadlock detection: thread tid -> (resource id -> held count)
    pub deadlock_alloc: BTreeMap<usize, BTreeMap<usize, usize>>,
    /// deadlock detection: thread tid -> the resource id it is waiting for
    pub deadlock_wait: BTreeMap<usize, usize>,
    /// deadlock detection: resource id -> tids that have ever released(up) it
    pub deadlock_up_history: BTreeMap<usize, Vec<usize>>,
    /// current priority of the process
    pub priority: usize,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    /// get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// allocate a new file descriptor
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
    /// allocate a new task id
    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }
    /// deallocate a task id
    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }
    /// the count of tasks(threads) in this process
    pub fn thread_count(&self) -> usize {
        self.tasks.len()
    }
    /// get a task with tid in this process
    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }
    /// (deadlock detection) record that `tid` is waiting for resource `res`
    pub fn deadlock_record_wait(&mut self, tid: usize, res: usize) {
        self.deadlock_wait.insert(tid, res);
    }
    /// (deadlock detection) clear the wait record of `tid`
    pub fn deadlock_clear_wait(&mut self, tid: usize) {
        self.deadlock_wait.remove(&tid);
    }
    /// (deadlock detection) record that `tid` has acquired resource `res`
    pub fn deadlock_record_alloc(&mut self, tid: usize, res: usize) {
        *self
            .deadlock_alloc
            .entry(tid)
            .or_default()
            .entry(res)
            .or_insert(0) += 1;
    }
    /// (deadlock detection) record that `tid` has released resource `res`
    pub fn deadlock_release_alloc(&mut self, tid: usize, res: usize) {
        if let Some(allocs) = self.deadlock_alloc.get_mut(&tid) {
            if let Some(cnt) = allocs.get_mut(&res) {
                if *cnt > 0 {
                    *cnt -= 1;
                }
            }
        }
    }
    /// (deadlock detection) drop all records of a thread that exited
    pub fn deadlock_clear_thread(&mut self, tid: usize) {
        self.deadlock_wait.remove(&tid);
        self.deadlock_alloc.remove(&tid);
    }
    /// (deadlock detection) record that `tid` has released (up) the
    /// semaphore resource `res`
    pub fn deadlock_record_up(&mut self, res: usize, tid: usize) {
        self.deadlock_up_history.entry(res).or_default().push(tid);
    }
    /// (deadlock detection) decide whether the requesting thread `tid`
    /// can eventually make progress:
    ///  - a thread that is not waiting can always run (and may release);
    ///  - waiting for a mutex can only be satisfied by ANOTHER holder of
    ///    the mutex that can itself make progress;
    ///  - waiting for a semaphore can also be satisfied by a thread that
    ///    has ever released (up) that semaphore and can make progress,
    ///    because semaphore `up` creates new instances.
    /// The judgement is propagated iteratively from the non-waiting
    /// threads until a fixed point; deadlock iff `tid` stays unsatisfied.
    pub fn deadlock_detect(&self, tid: usize) -> bool {
        let tids: Vec<usize> = {
            let mut v: Vec<usize> = Vec::new();
            for t in self
                .deadlock_alloc
                .keys()
                .chain(self.deadlock_wait.keys())
                .chain(core::iter::once(&tid))
            {
                if !v.contains(t) {
                    v.push(*t);
                }
            }
            v
        };
        // sat[i] == true means tids[i] can make progress eventually
        let mut sat: Vec<bool> = tids
            .iter()
            .map(|t| !self.deadlock_wait.contains_key(t))
            .collect();
        loop {
            let mut progressed = false;
            for i in 0..tids.len() {
                if sat[i] {
                    continue;
                }
                let wait_res = match self.deadlock_wait.get(&tids[i]) {
                    Some(r) => *r,
                    None => continue,
                };
                // candidate threads whose progress can satisfy the request
                let mut candidates: Vec<usize> = Vec::new();
                // holders of the resource
                let mut has_holder = false;
                for (h, allocs) in self.deadlock_alloc.iter() {
                    if allocs.get(&wait_res).map_or(false, |c| *c > 0) {
                        has_holder = true;
                        if *h != tids[i] {
                            candidates.push(*h);
                        }
                    }
                }
                // A locked mutex without any holder record is being handed
                // over to a woken waiter whose allocation is not recorded
                // yet; treat the wait as satisfiable to avoid false
                // positives in this hand-over window.
                if wait_res % 2 == 0 && !has_holder {
                    sat[i] = true;
                    progressed = true;
                    continue;
                }
                // for semaphores, threads that have ever up'ed it as well
                if wait_res % 2 == 1 {
                    if let Some(ups) = self.deadlock_up_history.get(&wait_res) {
                        for u in ups.iter() {
                            if *u != tids[i] && !candidates.contains(u) {
                                candidates.push(*u);
                            }
                        }
                    }
                }
                if candidates
                    .iter()
                    .any(|c| match tids.iter().position(|t| t == c) {
                        Some(j) => sat[j],
                        None => true, // not tracked -> not waiting -> can run
                    })
                {
                    sat[i] = true;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        match tids.iter().position(|t| *t == tid) {
            Some(i) => !sat[i],
            None => false,
        }
    }
}

impl ProcessControlBlock {
    /// inner_exclusive_access
    pub fn inner_exclusive_access(&self) -> RefMut<'_, ProcessControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// new process from elf file
    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        trace!("kernel: ProcessControlBlock::new");
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        // allocate a pid
        let pid_handle = pid_alloc();
        let process = Arc::new(Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    deadlock_alloc: BTreeMap::new(),
                    deadlock_wait: BTreeMap::new(),
                    deadlock_up_history: BTreeMap::new(),
                    priority: crate::config::DEFAULT_TASK_PRIORITY,
                })
            },
        });
        // create a main thread, we should allocate ustack and trap_cx here
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_base,
            true,
        ));
        // prepare trap_cx of main thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();
        drop(task_inner);
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            ustack_top,
            KERNEL_SPACE.exclusive_access().token(),
            kstack_top,
            trap_handler as usize,
        );
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        drop(process_inner);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Only support processes with a single thread.
    pub fn exec(self: &Arc<Self>, elf_data: &[u8], args: Vec<String>) {
        trace!("kernel: exec");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        trace!("kernel: exec .. MemorySet::from_elf");
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        let new_token = memory_set.token();
        // substitute memory_set
        trace!("kernel: exec .. substitute memory_set");
        self.inner_exclusive_access().memory_set = memory_set;
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        trace!("kernel: exec .. alloc user resource for main thread again");
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;
        task_inner.res.as_mut().unwrap().alloc_user_res();
        task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        // push arguments on user stack
        trace!("kernel: exec .. push arguments on user stack");
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();
        user_sp -= (args.len() + 1) * core::mem::size_of::<usize>();
        let argv_base = user_sp;
        let mut argv: Vec<_> = (0..=args.len())
            .map(|arg| {
                translated_refmut(
                    new_token,
                    (argv_base + arg * core::mem::size_of::<usize>()) as *mut usize,
                )
            })
            .collect();
        *argv[args.len()] = 0;
        for i in 0..args.len() {
            user_sp -= args[i].len() + 1;
            *argv[i] = user_sp;
            let mut p = user_sp;
            for c in args[i].as_bytes() {
                *translated_refmut(new_token, p as *mut u8) = *c;
                p += 1;
            }
            *translated_refmut(new_token, p as *mut u8) = 0;
        }
        // make the user_sp aligned to 8B for k210 platform
        user_sp -= user_sp % core::mem::size_of::<usize>();
        // initialize trap_cx
        trace!("kernel: exec .. initialize trap_cx");
        let mut trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            task.kstack.get_top(),
            trap_handler as usize,
        );
        trap_cx.x[10] = args.len();
        trap_cx.x[11] = argv_base;
        *task_inner.get_trap_cx() = trap_cx;
    }

    /// create a brand-new child process from `elf_data` whose parent is
    /// `self`; unlike fork it does not copy the parent address space
    pub fn spawn(self: &Arc<Self>, elf_data: &[u8]) -> Arc<Self> {
        trace!("kernel: spawn");
        let process = Self::new(elf_data);
        // establish the parent-child relationship
        let mut inner = self.inner_exclusive_access();
        process.inner_exclusive_access().parent = Some(Arc::downgrade(self));
        inner.children.push(Arc::clone(&process));
        process
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        trace!("kernel: fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        let memory_set = MemorySet::from_existed_user(&parent.memory_set);
        // alloc a pid
        let pid = pid_alloc();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    deadlock_alloc: BTreeMap::new(),
                    deadlock_wait: BTreeMap::new(),
                    deadlock_up_history: BTreeMap::new(),
                    priority: crate::config::DEFAULT_TASK_PRIORITY,
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        // create main thread of child process
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            parent
                .get_task(0)
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base(),
            // here we do not allocate trap_cx or ustack again
            // but mention that we allocate a new kstack here
            false,
        ));
        // attach task to child process
        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        drop(child_inner);
        // modify kstack_top in trap_cx of this thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        add_task(task);
        child
    }
    /// get pid
    pub fn getpid(&self) -> usize {
        self.pid.0
    }
}
