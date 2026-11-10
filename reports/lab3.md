# 第五章实验报告（lab3）

## 简要总结（200 字以内）

本章完成了三项任务：

1. **syscall 迁移**：将 `sys_get_time`、`sys_mmap`、`sys_munmap` 迁移到进程模型下，用户指针统一经页表翻译（`translated_byte_buffer` 等），mmap/munmap 逻辑与第四章一致。
2. **spawn 系统调用（400）**：新增 `TaskControlBlock::spawn`，从 ELF 直接新建子进程并建立父子关系，不复制父地址空间，成功返回子进程 pid。
3. **stride 调度与 sys_set_priority（140）**：TCB 内新增 stride/pass/priority 字段（初始优先级 16），调度器每次选取 stride 最小的就绪进程并使其 stride 加 pass = BIG_STRIDE/priority；`sys_set_priority` 拒绝 prio < 2。

另外修复了物理帧回收后残留页表项导致的"幽灵映射"问题（分配帧时清零）。本地 CI 15/15 通过；ch5_stride 六进程公平性 max/min ≈ 1.04 < 1.5。

## 问答题

### 1. 两个 pass=10 的进程，stride 用 8bit 无符号数存储，p1.stride=255、p2.stride=250，p2 执行一个时间片后，实际是否轮到 p1？

不会轮到 p1，p2 会再次被调度。

p2 执行一个时间片后其 stride 增加 pass：250 + 10 = 260，但 8bit 无符号数最大只能表示 255，于是发生**回绕**：260 mod 256 = 4。此时 p1.stride = 255，p2.stride = 4，调度器按"选最小 stride"的规则比较 4 < 255，仍然选中 p2。按真实的步进语义，p2 的 stride（260）本应大于 p1 的 stride（255），理应轮到 p1 执行；正是无符号数回绕使比较结果失真，破坏了 stride 调度的公平性。这就是 stride 溢出必须处理的原因。

### 2. 在优先级全部 ≥ 2、不考虑溢出的情况下，为什么 STRIDE_MAX − STRIDE_MIN ≤ BigStride / 2？

优先级 ≥ 2 意味着任何进程的 pass = BigStride / priority ≤ BigStride / 2。

调度器每次选出 stride 最小的进程运行，运行后它的 stride 增加自己的 pass，其余进程的 stride 都不变。分两种情况（设运行前最小值为 min，最大值为 max，选中的进程 stride 为 min）：

- 若 min + pass ≤ max：运行后最大值仍是 max，而新的最小值是其余进程中原来的次小值（≥ min，且 ≤ max），max − min' 不会超过原来的差距；
- 若 min + pass > max：运行后该进程的 stride 成为新的最大值 min + pass ≤ min + BigStride / 2，而其余进程的 stride 都 ≥ min，所以差距 min + pass − (其余中的最小值) ≤ pass ≤ BigStride / 2。

归纳可知任意时刻任意两个就绪进程的 stride 之差都不超过 BigStride / 2：一个进程只有在自己 stride 最小时才会被调度前进，每次最多前进 BigStride / 2，它不可能把别人甩开超过这个距离。

### 3. 考虑溢出的情况下，为 Stride 设计特别的比较器

结合第 2 问的结论：任意两个有效 stride 的真实差值总在 (−BigStride/2, BigStride/2) 内。对 8bit 存储、BigStride = 255 的例子，差值范围是 (−127, 127)，恰好可以用"把无符号数按有符号数解释"的方式恢复正确的顺序：把最高位视为符号位，0x80~0xFF 解释为负数。提示中的两个例子验证了这一点：

- 125 与 255：真实含义是 125 与 −1，125 < −1 为假，即 `(125 < 255) == false`；
- 129 与 255：129 按位解释是 −127，−127 < −1 为真，即 `(129 < 255) == true`。

补全的比较器（用 wrapping 差值转为有符号数比较，两种写法等价）：

```rust
impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // 写法一：按有符号数重新解释后比较
        Some((self.0 as i8).cmp(&(other.0 as i8)))
        // 写法二（等价）：回绕差值转有符号
        // Some((self.0.wrapping_sub(other.0) as i8).cmp(&0))
    }
}

impl Ord for Stride {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}
```

我的内核实现中选用 usize 存储 stride 并直接用 64 位无符号比较，由于 64 位空间极大、测试时长内不会回绕，无需该技巧；若严格按题目用窄整数存储，则需要上述比较器。

## 荣誉准则

1. 在完成本次实验的过程（含此前所有章节）中，我曾与以下人就本实验相关的内容进行过讨论：无（独立完成）。
2. 我参考了以下资料（书籍、网页、代码等），并在相应位置注明：
   - rCore-Tutorial-Guide 第五章课程指导（learningos.cn）及 rCore-Tutorial-Book-v3 第五章正文，用于理解 stride 调度算法语义与 spawn 设计；
   - 课程提供的 rCore 框架代码（本章初始代码），在其基础上修改完成。
3. 我独立完成了本实验除上述引用之外的所有工作，包括代码编写与调试、实验报告撰写。
4. 我从未抄袭他人的代码，也不愿意将本实验代码提供给他人抄袭。

## 感想（optional）

本章最大的收获是理解了"调度公平性"如何用极简的数学（stride/pass）实现，以及整数回绕这类底层细节对算法正确性的影响——第 1 问的回绕问题在我最初用 usize 实现时完全没有暴露，直到推演 8bit 例子才意识到比较器语义的重要性。另外，第四章遇到的回收帧残留 PTE 问题在多进程频繁创建/退出的本章更容易复现，"分配时清零"这一防御性修复在本章得到了验证。
