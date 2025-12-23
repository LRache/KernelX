# KernelX

KernelX 是我们从0开始使用Rust编写的，用于学习的开源类Unix的操作系统宏内核。它的目标是提供一个简单的、易于理解的内核实现，适合初学者学习操作系统原理。现在可以运行 `gcc`、 `busybox`、`vim`、 `python` 等用户态程序，并且可以在开发版starfive-jh7100上运行。

## 特性

- 支持匿名页的换入换出，以应对嵌入式设备内存有限的情况，支持内核线程，采用kswapd定期扫描页面，减轻内存压力。

- 采用事件唤醒机制，而非轮询等待设备，降低系统功耗。

- 去除了板级抽象层，完全依赖于设备树完成内存映射和设备初始化，对于不同的硬件平台，无需重新编译内核。

## Quick Start

你可以在 `qemu-system-riscv64` 上运行 KernelX，你需要准备一个ext4格式的镜像用于根文件系统。

1. 确保你已经安装了如下工具: `qemu-system-riscv64`、`cmake`、用于riscv64的C交叉编译器、安装了交叉编译到riscv64目标的rust编译环境、`gcc`、`make`等。

2. 在根目录下运行

```bash
make menuconfig
```
配置 `Platform Configuration` 中的选项，填写交叉编译器的路径和其他相关设置。

配置 `QEMU-Configuration` 中的选项，设置要加载的根文件系统镜像以及init程序等等。

3. 运行 `make run` 即可启动系统。

## 项目结构

### 系统结构

![KernelX 结构](./docs/static/struct.svg)

- 文件系统FileSystem: 提供了VFS抽象和ext4、devfs、tmpfs等具体文件系统的实现

- 设备驱动Driver: 提供了设备驱动框架和具体设备驱动的实现

- 内核核心Kernel: 包含内核启动流程、内存管理、调度器、系统调用、任务管理、事件通知、进程间通信、内核线程和用户态同步原语等子系统。

- 架构抽象Arch: 提供了对不同硬件架构的支持，目前实现了RISC-V架构。

- 公用库Klib: 提供了内核所需的公用库功能，如日志记录、内存分配等。

### 代码结构

```text
KernelX/
├── src/                        # 内核 Rust 主代码
│   ├── main.rs                 # 内核入口
│   ├── arch/                   # 架构相关抽象与实现
│   │   ├── arch.rs
│   │   ├── mod.rs
│   │   └── riscv/              # RISC-V 实现
│   ├── driver/                 # 设备驱动框架与具体驱动
│   │   ├── device.rs
│   │   ├── driver.rs
│   │   ├── fdt.rs              # 设备树解析与驱动匹配
│   │   └── ...
│   ├── fs/                     # VFS 与各类文件系统
│   │   ├── filesystem.rs
│   │   ├── ext4/
│   │   ├── devfs/
│   │   ├── tmpfs/
│   │   └── ...
│   ├── kernel/                 # 内核核心子系统
│   │   ├── main.rs             # 内核启动流程
│   │   ├── config.rs           # 内核配置与编译期参数
│   │   ├── trap.rs             # 异常处理
│   │   ├── mm/                 # 内存管理
│   │   ├── scheduler/          # 调度器与调度策略
│   │   ├── syscall/            # 系统调用分发与实现
│   │   ├── task/               # 任务/进程/线程管理
│   │   ├── event/              # 事件与通知机制
│   │   ├── ipc/                # 进程间通信
│   │   ├── kthread/            # 内核线程框架
│   │   ├── usync/              # 用户态同步原语支持
│   │   └── ...
│   └── klib/                   # 内核公用库（日志、内存分配等）
├── clib/                       # 内核C代码
├── lib/                        # 内核Rust依赖库
├── vdso/                       # vDSO 相关代码
├── usertests/                  # 用户态测试程序与根文件系统构建脚本
├── scripts/                    # 构建、打包和 QEMU 启动脚本
├── linker/                     # 链接脚本
├── config/                     # Kconfig 与构建配置
└── Makefile, build.mk, Cargo.toml  # 顶层构建入口
```

## 功能实现

- 文件系统

    + 实现了VFS、Inode、dentry等抽象，使用dentry缓存加速路径查找。

    + 实现了Ext4、devfs、tmpfs、procfs等文件系统接口。

    + 实现了部分文件权限管理功能。

- 设备驱动

    + 提供了设备抽象和各种类型设备的驱动抽象。

    + 支持的设备包括了串口ns16650a、virtio块设备、sdcard等。

    + 对于串口提供了stty的抽象，用于配置串口参数。

    + 启动的时候通过设备树选择默认的RTC、串口等设备，支持传递启动参数给内核。

- 内核核心实现

    + 内存管理：实现了内核页面分配器，实现了匿名页、文件映射等用户页面，支持CoW机制。实现了ELF加载器，支持动态链接库。在内存紧张时，支持匿名页的换出到磁盘上。

    + 任务管理：对内核线程和用户线程做了统一抽象，实现了任务创建、销毁、调度等功能。

    + IPC和用户态同步：支持Futex、信号、管道等进程间通信和同步机制。

    + 事件机制：基于事件驱动模型实现了事件通知机制，支持文件描述符的可读写事件通知和内核线程等待事件机制，降低CPU轮询等待的开销。

- 体系结构层

    + 提供了统一的体系结构抽象接口，包括了页表、中断处理、上下文切换等功能。

    + 实现早期启动过程，早期通过读取设备树，完成内核地址映射，以支持内核可以被加载到对齐到页面的任意地址运行，不同平台无需再次编译内核。

- Klib

    + 提供了内核日志功能，支持不同级别的日志输出。

    + 提供了内核内存分配器接口和实现。

    + 提供了一些常用的数据结构和算法实现。

## 参考和引用

- 使用了Ext4文件系统实现库: https://github.com/elliott10/lwext4_rust

- 使用了tlsf内存分配器：https://github.com/mattconte/tlsf

- 参考了StarryX: https://gitlab.eduxiji.net/T202510336995486/oskernel2025-starryx

- 参考了NoAxiom: https://github.com/NoAxiom/NoAxiom-OS

- 参考了xv6-riscv: https://github.com/mit-pdos/xv6-riscv

## 贡献

欢迎任何形式的贡献！无论是报告问题、提出建议，还是提交代码，我们都非常感谢。对我们的项目有问题或建议，请通过GitHub Issues或者邮箱youzi_rache@foxmail.com与我们联系。
