# 使用 KVM example 启动 Linux guest

本文说明如何使用 `usertests/kvm` example 构建一个 KernelX 根文件系统，并在 KernelX 中运行
`/host/kxemu-kvm` 启动 RISC-V Linux 6.1 guest。

## 前置条件

- KernelX 目标架构为 RISC-V 64，并在 `make menuconfig` 的 `Experimental Features` 中打开
  `Enable KVM`。
- 已安装 RISC-V Linux 交叉编译工具链，例如 `riscv64-linux-gnu-gcc`，以及 Rust target
  `riscv64gc-unknown-linux-gnu`。
- 已安装构建和打包工具：`make`、`cargo`、`wget`、`tar`、`cpio`、`gzip`、`readelf`、
  `mkfs.ext4`、`e2mkdir`、`e2cp`。
- 已安装 `qemu-system-riscv64`，用于启动外层 KernelX。

## 构建 example 镜像

在仓库根目录运行：

```bash
make -C usertests/kvm \
  HOST_IMPL=host-rs \
  GUEST_COMPONENTS=linux6.1 \
  CROSS_COMPILE=riscv64-linux-gnu- \
  package
```

这个命令会完成三件事：

1. 构建 Rust 版 KVM host 程序 `kxemu-kvm`。
2. 下载并构建 Linux 6.1 guest，同时生成 initramfs。
3. 打包 `usertests/kvm/build/riscv64/kvm.ext4`。

镜像中的关键路径如下：

```text
/host/kxemu-kvm
/guest/linux6.1/Image
/guest/linux6.1/initramfs.cpio.gz
```

如果只想预先下载 Linux 源码，可以单独运行：

```bash
make -C usertests/kvm download-linux6.1
```

## 配置外层 KernelX

运行：

```bash
make menuconfig
```

建议配置如下：

- `Experimental Features -> Enable KVM`: 打开。
- `QEMU Configuration -> Disk Image`: `usertests/kvm/build/riscv64/kvm.ext4`。
- `QEMU Configuration -> Root Device`: `virtio_mmio@10001000`。
- `QEMU Configuration -> Root FS Type`: `ext4`。
- `QEMU Configuration -> Init Path`: `/host/kxemu-kvm`。
- `QEMU Configuration -> Init Args`:

```text
--kernel /guest/linux6.1/Image --initrd /guest/linux6.1/initramfs.cpio.gz --append console=ttyS0,115200 --append earlycon=uart8250,mmio,0x10000000,115200 --memory-size 536870912
```

生成的 `config/.config` 中应至少包含类似配置：

```text
CONFIG_KVM=y
CONFIG_DISK_IMAGE="usertests/kvm/build/riscv64/kvm.ext4"
CONFIG_INITPATH="/host/kxemu-kvm"
CONFIG_INITARGS="--kernel /guest/linux6.1/Image --initrd /guest/linux6.1/initramfs.cpio.gz --append console=ttyS0,115200 --append earlycon=uart8250,mmio,0x10000000,115200 --memory-size 536870912"
CONFIG_ROOT_DEVICE="virtio_mmio@10001000"
CONFIG_ROOT_FS_TYPE="ext4"
```

随后启动外层 KernelX：

```bash
make run
```

KernelX 会把 `/host/kxemu-kvm` 作为 init 进程启动。`kxemu-kvm` 会打开 `/dev/kvm`，加载
`/guest/linux6.1/Image` 和 `/guest/linux6.1/initramfs.cpio.gz`，然后进入 Linux guest。进入
guest 后会先执行 initramfs 中的 `/init`，最后落到 BusyBox shell。

## 在 KernelX shell 中手动启动

如果你希望先进入 KernelX shell，再手动启动 Linux guest，需要让外层 KernelX 的根文件系统同时包含
shell、`/host/kxemu-kvm` 和 `/guest/linux6.1/*`。进入 shell 后执行：

```bash
/host/kxemu-kvm \
  --kernel /guest/linux6.1/Image \
  --initrd /guest/linux6.1/initramfs.cpio.gz \
  --append console=ttyS0,115200 \
  --append earlycon=uart8250,mmio,0x10000000,115200 \
  --memory-size 536870912
```

`kxemu-kvm` 默认会构造 guest DTB；通常不需要手动传 `--dtb`。

## 挂载 guest 磁盘

上面的命令只使用 initramfs。如果需要给 Linux guest 暴露一个 virtio-blk 磁盘：

1. 在外层 QEMU 配置一个第二磁盘，例如 `QEMU Configuration -> Second Disk Image`。
2. 启动 `kxemu-kvm` 时增加 `--disk /dev/virtio_mmio@10002000`。
3. 给 Linux guest 增加根盘参数，例如：

```text
--disk /dev/virtio_mmio@10002000 --append root=/dev/vda --append rw
```

这里的 `/dev/virtio_mmio@10002000` 是外层 KernelX 看到的第二块 virtio 磁盘；Linux guest
内部会通过 `kxemu-kvm` 暴露出的 virtio-mmio block 设备看到它，通常对应 `/dev/vda`。

## 常见问题

- `open /dev/kvm` 失败：确认 `CONFIG_KVM=y`，并且已经用新内核重新 `make run`。
- 找不到 `/guest/linux6.1/Image` 或 `/guest/linux6.1/initramfs.cpio.gz`：确认使用
  `GUEST_COMPONENTS=linux6.1 package` 重新生成了 `usertests/kvm/build/riscv64/kvm.ext4`，
  并且外层 QEMU 的 `Disk Image` 指向这份镜像。
- Linux guest 没有串口输出：确认传入了 `console=ttyS0,115200`，或使用 defconfig 中的
  `earlycon=uart8250,mmio,0x10000000,115200`。
- Linux guest 内存不足或启动不稳定：优先使用 `--memory-size 536870912`，即 512 MiB。
