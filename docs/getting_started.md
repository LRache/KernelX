# 快速开始

## 环境配置

KernelX 依赖于 C 和 Rust 开发环境。请确保你已经安装了以下工具：

1. C 语言的交叉编译器，例如 `riscv64-unknown-elf-gcc`或 `riscv64-linux-gnu-gcc`。

2. Rust 编译器和工具链，安装 Rust 并添加适用于目标架构的交叉编译支持，例如 `riscv64gc-unknown-none-elf`，需要 nightly 版本的 Rust。

3. CMake、 Make、 gcc、g++ 等工具，用于构建项目。

## 构建内核

在构建内核之前，你需要配置内核选项。可以通过以下命令启动配置界面：

```bash
make menuconfig
```

- 在 Platform Configuration 中，填写你的 CROSS_COMPILE，例如 `/path/to/riscv64-unknown-linux-gnu-`。同时选择目标架构（目前仅支持 RISC-V）。

- 在 Build Configuration 中，选择编译类型（Debug 或 Release）。

- 在 Debug Configuration 中，可以选择是否启用内核调试选项。

- 在 Experimental Features 中，可以选择是否启用实验性功能，例如页面交换功能。
配置完成后，运行以下命令构建内核：

```bash
make all
```

编译好的内核位于目录 `build/{arch}/` 下，其中 `Image` 是内核镜像文件，是原 elf 文件经过 objcopy 的产物，可以被直接复制到开发版内存中使用， `vmkernelx` 是包含调试信息的 ELF 文件。

## 运行内核

内核的参数由设备树（Device Tree）提供，内核参数应该在设备树的 `chosen` 节点中指定 `bootargs` 属性。例如：

支持的内核参数包括：

- `root=`: 指定根文件系统的设备，例如 `root=virtio_mmio@10001000`，注意设备名是基于设备树中的节点名称。

- `rootfstype=`: 指定根文件系统的类型，例如 `rootfstype=ext4`。

- `init=`: 指定 init 进程的路径，例如 `init=/bin/busybox`。

- `initcwd=`: 指定 init 进程的工作目录，例如 `initcwd=/`。

- `initarg=`: 指定传递给 init 进程的参数，例如 `initarg=sh`。

**内核在不同SoC上运行的行为完全依赖于设备树和是否有匹配的驱动，你无需为每种SoC重新编译内核。**

我们提供了在 QEMU 上运行 KernelX 的方法。确保你已经安装了 `qemu-system-riscv64`。然后，你可以在 menuconfig 中配置 QEMU 相关选项，例如根文件系统镜像路径和 init 程序路径。配置完成后，运行以下命令启动内核：

```bash
make run
```

在 init 进程退出后，内核会自动触发 panic 并停止运行。

我们推荐运行这个[2025年全国操作系统大赛内核实现赛道决赛的镜像](https://github.com/oscomp/testsuits-for-oskernel/releases/tag/on-site-final-2025-rv64-fs)，它包含了丰富的用户态程序，可以帮助你更好地体验 KernelX 的功能。你可以将 `/bin/busybox` 作为 init 程序，`sh` 作为初始参数来使用这个镜像。

## 代码提示工具

打开本项目后（几乎一定）你会发现你的 Rust 代码提示工具无法正常工作，你需要设置一些参数。

对于 vscode

```json
// .vscode/settings.json
{
    "rust-analyzer.check.extraArgs": [
        "--target",
        "riscv64gc-unknown-none-elf",
    ],
    "rust-analyzer.cargo.features": [
        "log-trace",
        "swap-memory",
    ],
    "rust-analyzer.cargo.extraArgs": [
        "--target",
        "riscv64gc-unknown-none-elf",
    ],
    "rust-analyzer.server.extraEnv": {
        "KERNELX_HOME": "${workspaceFolder}",
        "ARCH": "riscv",
        "ARCH_BITS": "64",
        "CROSS_COMPILE": "/path/to/your/cross_compiler-"
    }
}
```
