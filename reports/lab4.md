# 第六章实验报告（lab4）

## 简要总结（200 字以内）

本章在 easy-fs 与内核侧实现了三个文件系统相关系统调用：

1. **sys_linkat（37）**：在根目录中找到 `old_name` 目录项的 inode_id，追加一个指向同一 inode 的新目录项 `new_name`，实现硬链接。
2. **sys_unlinkat（35）**：从根目录删除指定目录项（末项搬移填补空洞并收缩目录大小）；若该 inode 已无任何目录项引用，则释放其全部数据块。
3. **sys_fstat（80）**：经 fd 取 OSInode，返回 Stat{dev, ino, mode, nlink}；为 vfs 的 Inode 结构补充 inode_id 字段，nlink 通过扫描根目录统计同一 inode 的引用数。

同时迁移了 ch5 的 get_time/mmap/munmap/spawn/set_priority/stride 调度。本地 CI 31/31 通过。

## 问答题

### 1. 在我们的 easy-fs 中，root inode 起着什么作用？如果 root inode 中的内容损坏了，会发生什么？

**root inode 的作用**：root inode（inode 0）是整个文件系统唯一的目录（Directory 类型），所有用户程序与文件都作为它的目录项存在。内核启动时通过 `EasyFileSystem::root_inode` 加载它并保存在全局 `ROOT_INODE` 中，此后一切文件操作——`open`/`find` 按名查找、`create` 新建文件、`link`/`unlink` 增删硬链接、`ls` 列目录、本章 fstat 的 nlink 统计——都以它为入口。它实际上承担了"根目录 + 文件系统命名空间根"的双重角色。

**如果 root inode 内容损坏**：

- 若**目录数据块损坏**（目录项被破坏）：按名查找 `find` 会因名字比对失败而找不到文件，`open/exec` 将返回错误——最严重的情况是 initproc 与 user_shell 的 ELF 无法加载，内核启动即失败（INITPROC unwrap panic）；同时 nlink 统计、link/unlink 也会得到错误结果。
- 若**元数据损坏**（size、direct/indirect 索引被破坏）：size 变大时 `find_inode_id` 会按错误的 file_count 读到未初始化的"目录项"，可能返回垃圾 inode_id，内核拿着它去 `get_disk_inode_pos` 读写，轻则数据错乱，重则越界访问块缓存导致内核崩溃；size 变小则部分文件"消失"；若 `type_` 不再是 Directory，`find_inode_id` 中的 `assert!(disk_inode.is_dir())` 会直接 panic。
- 由于 easy-fs 中 root inode 的位置（inode 0）由 SuperBlock 的布局参数静态推算，若 SuperBlock 尚完好，系统仍能"找到"root inode，但解释出来的内容已不可信，整个文件系统的所有文件实际上都不可达/不可信了。

简言之：root inode 是文件系统的命名空间根与元数据枢纽，它损坏等于整个文件系统不可用。

### 2.（附：ch7 相关思考）管道是如何实现的？read/write 在管道空/满时的行为是什么？

（由于 ch7 不单独设实验，按要求把该问一并列入本章报告。）

我的内核中管道由一块 32×512 字节的环形缓冲区（`PipeRingBuffer`）实现，读写两端各持有一个 `Arc<Pipe>`（一端可写、一端可读）。读端所有 fd 关闭后 write 返回 -1（EPIPE），写端所有 fd 关闭后 read 返回 -1 而非阻塞；管道为空且写端仍在时 read 返回 0（等待/让出），管道写满时 write 丢弃剩余字节返回已写字节数。dup/传给子进程共享同一缓冲区，从而实现进程间字节流通信。

## 荣誉准则

1. 在完成本次实验的过程（含此前所有章节）中，我曾与以下人就本实验相关的内容进行过讨论：无（独立完成）。
2. 我参考了以下资料（书籍、网页、代码等），并在相应位置注明：
   - rCore-Tutorial-Guide 第六章课程指导（learningos.cn）及 rCore-Tutorial-Book-v3 第六章正文，用于理解 easy-fs 磁盘布局、目录项与硬链接语义；
   - 课程提供的 rCore 框架代码（本章初始代码），在其基础上修改完成。
3. 我独立完成了本实验除上述引用之外的所有工作，包括代码编写与调试、实验报告撰写。
4. 我从未抄袭他人的代码，也不愿意将本实验代码提供给他人抄袭。

## 感想（optional）

这一章把"文件 = inode + 若干目录项引用"这个 UNIX 抽象落地得非常清楚。实现中我选择不改动磁盘上 DiskInode 的布局，而是把 nlink 建模为"根目录中指向该 inode 的目录项数"，动态扫描得到——这让 link/unlink/fstat 的语义天然一致，也避免了为新增字段迁移磁盘镜像格式。另外，unlink 的"末项搬移 + 收缩目录 + 末引用释放数据块"三步必须在一个 fs 锁临界区内完成，spin Mutex 不可重入的约束在这里体现得很具体。
