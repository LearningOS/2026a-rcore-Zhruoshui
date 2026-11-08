# lab1（ch3：多道程序与分时多任务）实验报告

## 一、功能总结

在 ch3 内核中实现了新的系统调用 `sys_trace`（syscall id = 410），用于追踪当前任务的系统调用历史。具体包括：

1. 在 `TaskControlBlock` 中新增 `syscall_times: [u32; MAX_SYSCALL_NUM]` 数组，记录当前任务每种系统调用的次数；
2. 在内核统一的 `syscall()` 分发函数入口处，对当前任务的 `syscall_times[syscall_id]` 计数，这样每次系统调用（包括 `sys_trace` 自身）在真正分发执行前就被计入统计；
3. 实现 `sys_trace` 的三种语义：
   - `trace_request == 0`：把 `id` 视为 `*const u8`，读取该地址处一个字节并返回；
   - `trace_request == 1`：把 `id` 视为 `*mut u8`，将 `data` 的最低字节写入该地址，返回 0；
   - `trace_request == 2`：返回当前任务调用编号为 `id` 的系统调用的次数（本次调用由于已在入口处计数，自然计入统计）；
   - 其他值返回 -1。

对读/写地址做了简单的 RAM 范围合法性检查（落在物理内存 `[0x8000_0000, MEMORY_END)` 内才执行，否则返回 -1），防止内核直接解引用完全非法的地址而崩溃。

本地通过 `make -C ci-user test CHAPTER=3` 全部 7 项检查（含 ch2b 基础测例）。

## 二、简答作业

### 1. bad 测例的出错行为

使用的 SBI 为仓库提供的 RustSBI jump 固件（rustsbi-qemu.bin，RustSBI 0.4.1，QEMU 8.2 兼容），QEMU 为 `qemu-system-riscv64`。

- `ch2b_bad_register.rs`：尝试访问 S 态寄存器 `sstatus`（`csrr sstatus`），U 态无权访问，触发 `IllegalInstruction` 异常，内核打印 `[kernel] IllegalInstruction in application, kernel killed it.` 并杀死该应用；
- `ch2b_bad_instruction.rs`：执行 S 态特权指令 `sret`，同样触发 `IllegalInstruction` 异常被内核杀死；
- `ch2b_bad_address.rs`：向非法地址（0 页）写入，触发 `StorePageFault`，内核打印 `[kernel] PageFault in application, bad addr = 0x0 ...` 并杀死该应用。

三个测例均不会影响内核与其他应用的运行，这正是批处理/多道程序系统隔离错误应用的基本手段。

### 2. trap.S 中 `__alltraps` 与 `__restore` 的理解

1. **L40（`__restore` 入口）`sp` 的含义与两种使用情景**：刚进入 `__restore` 时 `sp` 指向**内核栈**上保存的 `TrapContext`（尚未释放）。`__restore` 的两种使用情景：(a) `trap_handler` 处理完陷阱后正常返回用户态；(b) 任务第一次从内核进入用户态（第一次运行应用），此时直接把构造好的 TrapContext 地址作为 `sp` 跳到 `__restore`，借助它完成"进入用户态"的公共路径。

2. **L43–L48 特殊处理的寄存器**：
   - `t0 ← 32*8(sp)` 后 `csrw sstatus, t0`：恢复 `sstatus`，其中 SPP 位决定 `sret` 后的特权级（清 0 表示回到 U 态），SPIE/SIE 控制中断使能状态；
   - `t1 ← 33*8(sp)` 后 `csrw sepc, t1`：恢复 `sepc`，它是 `sret` 返回的用户地址（陷阱发生处的用户 PC，或首次运行时为应用入口）；
   - `t2 ← 2*8(sp)` 后 `csrw sscratch, t2`：恢复 `sscratch` 为用户栈指针，供下一次陷入/返回时交换 `sp` 使用。

3. **L50–L56 为何跳过 x2 与 x4**：`x2` 是 `sp`，用户栈指针已保存在 TrapContext 的 2 号槽位并通过 `sscratch` 机制单独恢复，不能在这里直接 `ld`（当前 `sp` 还指向内核栈上的 TrapContext）；`x4` 是 `tp`（线程指针），应用并未使用，无需保存恢复。

4. **L60（`csrrw sp, sscratch, sp`）之后**：`sp` 变为**用户栈指针**（TrapContext 已通过 `addi sp, sp, 34*8` 释放，`sscratch` 中存的正是用户栈），`sscratch` 变为**内核栈指针**，为下一次从用户态陷入时 `__alltraps` 的第一次 `csrrw sp, sscratch, sp` 做准备。

5. **状态切换发生在 `sret` 指令**：`sret` 硬件语义是 PC ← `sepc`，特权级按 `sstatus.SPP`（为 0 即 U 态）切换，并恢复 `sstatus.SIE ← SPIE`。因此 `sret` 执行后 CPU 回到用户态从 `sepc` 处继续执行。

6. **L13（`__alltraps` 中第一次 `csrrw sp, sscratch, sp`）之后**：`sp` 变为**内核栈指针**（可以在内核栈上分配 TrapContext 保存现场），`sscratch` 变为**用户栈指针**（保存用户 sp，随后存入 TrapContext 的 2 号槽位）。

7. **从 U 态进入 S 态的指令**：`ecall`（environment call）。用户程序执行 `ecall` 触发 `Environment call from U-mode` 异常，硬件将特权级切到 S 态并把 PC 置为 `stvec`（即 `__alltraps`）。

## 三、荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

   > 独立完成，未与其他人交流。

2. 此外，我也参考了 **以下资料**，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

   > 1. rCore-Tutorial-Guide 实验指导（chapter3 编程作业说明）：sys_trace 三种语义的定义。
   > 2. rCore-Tutorial-Book-v3 第三章（任务切换、Trap 上下文、trap.S 的保存/恢复流程）。
   > 3. 课程仓库自带代码框架（os/src 各模块骨架与注释）。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按"-100"分计。

## 四、实验感想（optional）

ch3 在框架完整度较高的基础上，主要考察对系统调用分发路径与任务控制块的理解。`sys_trace` 中"本次调用也计入统计"这一细节需要把计数放在分发之前，是个很容易踩坑的小细节；用户栈位于内核镜像 .bss 中（而非应用加载区），也提醒我们对 ch3 无地址空间隔离的内存布局要有清晰认识。
