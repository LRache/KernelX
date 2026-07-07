# usertests 构建说明

`usertests` 用来构建用户态测试程序，并把测试产物和运行时库打包进一个 ext4 镜像。构建入口保持为 `build.sh`，实际逻辑在 `build.py` 中。

## 构建入口

在仓库根目录或 `usertests` 目录下都可以调用：

```sh
./usertests/build.sh riscv64
./usertests/build.sh loongarch64
./usertests/build.sh riscv64 kmodule filesystem
./usertests/build.sh riscv64 --tests kmodule,filesystem
./usertests/build.sh riscv64 kmodule --no-autorun
```

`build.sh` 只负责定位脚本目录并执行 `build.py`。`build.py` 会自动切换到 `usertests` 目录作为工作目录，因此相对路径都以 `usertests/` 为基准。

当前支持的架构参数：

```text
riscv64
loongarch64
loongarch
la64
```

其中 `loongarch` 和 `la64` 会被归一化为 `loongarch64`。

## 测试列表

不额外传测试名时，要构建哪些测试由 `build.py` 顶部的 `DEFAULT_TESTS` 列表决定：

```python
DEFAULT_TESTS = [
    "pthread-musl",
    "kmodule",
]
```

也可以通过命令行选择本次要导入镜像的测试 suite，不需要修改源码：

```sh
./usertests/build.sh riscv64 kmodule
./usertests/build.sh riscv64 kmodule filesystem
./usertests/build.sh riscv64 --tests kmodule,filesystem
./usertests/build.sh riscv64 all
./usertests/build.sh riscv64 kmodule --no-autorun
./usertests/build.sh --list-tests
```

其中 `all` 表示导入所有带 `Makefile` 的 suite。脚本会校验测试名，拼错或不存在的 suite 会直接报错。

每个测试套件必须是 `usertests/<suite>/` 下的一个目录，并且通常提供自己的 `Makefile`。构建脚本会对每个 suite 执行：

```sh
make all ISA=<isa> ARCH=<isa> ...
```

不同架构会自动传入对应的工具链变量。

## 架构和工具链

`build.py` 为每个架构维护一组默认工具链配置。

`riscv64` 默认使用：

```text
CROSS_COMPILE=riscv64-unknown-linux-gnu-
ELF_CROSS_COMPILER=riscv64-unknown-elf-
MUSL_CROSS_COMPILE=riscv64-linux-musl-
GLIBC_CC 等效为 riscv64-unknown-linux-gnu-gcc
MUSL_CC 等效为 riscv64-linux-musl-gcc
CARGO_TARGET=riscv64gc-unknown-linux-gnu
```

`loongarch64` 默认使用：

```text
CROSS_COMPILE=loongarch64-linux-gnu-
ELF_CROSS_COMPILER=loongarch64-unknown-elf-
MUSL_CROSS_COMPILE=loongarch64-linux-musl-
GLIBC_CC 等效为 loongarch64-linux-gnu-gcc
MUSL_CC 等效为 loongarch64-linux-musl-gcc
CARGO_TARGET=loongarch64-unknown-linux-gnu
```

可以通过环境变量覆盖这些默认值：

```sh
GLIBC_CC=/path/to/toolchain/bin/loongarch64-linux-gnu-gcc \
MUSL_CC=/path/to/toolchain/bin/loongarch64-linux-musl-gcc \
./usertests/build.sh loongarch64
```

`basic-ulib` 和 `helloworld` 当前只有 riscv64 汇编入口或 syscall 封装，`loongarch64` 构建时会被跳过并打印提示。

## 单个测试目录结构

每个 suite 的 Makefile 会把每个测试单独输出到一个目录：

```text
<suite>/build/<isa>/output/<test>/<test>
```

例如：

```text
stdio/build/riscv64/output/key/key
basic-glibc/build/riscv64/output/fork/fork
filesystem/build/riscv64/output/write/write
```

如果测试需要附带文件，也放在同一个测试目录下。例如：

```text
basic-glibc/build/riscv64/output/unlink/to_unlink.txt
filesystem/build/riscv64/output/write/a.txt
basic-ulib/build/riscv64/output/read/test.txt
```

这样打包到镜像后，所有测试都会放在 `/tests/<suite>/` 下。suite 目录下直接放测试 ELF 和附带文件，不再为每个测试额外包一层目录：

```text
/tests/<suite>/<test>
/tests/<suite>/<data-file>
```

例如：

```text
/tests/stdio/key
/tests/basic-glibc/unlink
/tests/basic-glibc/to_unlink.txt
```

## 产物收集

`build.py` 会先清理当前 suite 的旧输出目录：

```text
<suite>/build/<isa>/output
```

然后执行 suite 的 `make all`。构建完成后，脚本会把输出目录中每个测试子目录里的文件复制到根文件系统 staging 目录的 `tests/<suite>/` 下：

```text
build/<isa>/tests/<suite>/<test>
build/<isa>/tests/<suite>/<data-file>
```

如果某个旧 Makefile 仍然输出平铺文件，脚本会直接复制这些文件到 `tests/<suite>/`。

## 自动运行 init

默认情况下，`build.py` 会生成用于启动后自动运行测试的 `/init`。如果只想构建和打包测试，不需要自动运行，
可以传入 `--no-autorun`：

```sh
./usertests/build.sh riscv64 kmodule
./usertests/build.sh riscv64 --tests kmodule,pthread-musl
./usertests/build.sh riscv64 kmodule --no-autorun
```

默认自动运行模式会额外生成：

```text
/init
/etc/kx-tests.list
/tmp
```

`/init` 是一个小的 C runner，启动后会创建并挂载 `/tmp` 为 tmpfs，然后按 `/etc/kx-tests.list`
逐个 `fork`、`exec`、`wait` 测试程序，最后打印汇总结果。

测试清单的生成规则是：

1. 如果 suite 下存在 `run.list`，按其中的非空非注释行生成命令。相对路径会解释为 `/tests/<suite>/<line>`，绝对路径保持不变。
2. 如果没有 `run.list`，则按现有输出目录约定自动推导：`build/<isa>/output/<case>/<case>` 会变成 `/tests/<suite>/<case>`。
3. 如果输出是平铺文件，则只把带可执行权限的文件加入清单。

例如 kmodule suite 中的 `hello` case 会自动生成：

```text
/tests/kmodule/hello
```

这让新增 case 时只需要继续遵循 `output/<case>/<case>` 的产物布局；只有需要特殊参数或特殊顺序时，才需要为 suite
增加 `run.list`。

## libc 和运行时库

glibc 和 musl 的位置通过环境变量传递。脚本优先使用环境变量，找不到时才使用架构默认路径或编译器查询结果。

常用变量：

```text
CROSS_COMPILE glibc 测试使用的交叉编译前缀；未设置 GLIBC_CC 时，也用它推导 gcc 和 sysroot
GLIBC_CC      glibc 测试使用的 C 编译器，同时作为 glibc 运行时查询编译器
MUSL_CC       musl 测试使用的 C 编译器，同时可用于查询 musl libc.so
LIBC_DIR       通用 libc 目录，glibc/musl 都可作为兜底使用
GLIBC_LIB_DIR  glibc 运行时库目录
GLIBC_LIBC     glibc libc.so.6 的完整路径
GLIBC_CRT_DIR  glibc crt1.o、crti.o、crtn.o 所在目录
MUSL_LIB_DIR   musl 库目录
MUSL_LIBC      musl libc.so 的完整路径
```

如果没有显式提供 `GLIBC_LIB_DIR` / `LIBC_DIR` / `GLIBC_LIBC`，脚本会优先使用 `GLIBC_CC`，否则使用
`${CROSS_COMPILE}gcc` 执行 `--print-sysroot`，并从这个 sysroot 下的 `lib`、`lib64`、`usr/lib`、
`usr/lib64` 和 multiarch 目录查找 glibc loader 与运行时库。

示例：

```sh
GLIBC_LIB_DIR=/usr/riscv64-linux-gnu/lib \
MUSL_LIB_DIR=/opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib \
./usertests/build.sh riscv64
```

使用自定义交叉工具链时：

```sh
CROSS_COMPILE=/opt/cross-tools/riscv64-unknown-linux-gnu/bin/riscv64-unknown-linux-gnu- \
./usertests/build.sh riscv64 kmodule
```

loongarch 示例：

```sh
GLIBC_LIB_DIR=/usr/loongarch64-linux-gnu/lib \
./usertests/build.sh loongarch64
```

打包 glibc 时，脚本会把动态链接器复制到对应架构 ELF interpreter 要求的位置，并把常见运行时库复制到镜像的 `/lib`。

动态链接器路径例如：

```text
/lib/ld-linux-riscv64-lp64d.so.1
/lib64/ld-linux-loongarch-lp64d.so.1
```

常见运行时库路径例如：

```text
libc.so.6
libm.so.6
libpthread.so.0
libdl.so.2
librt.so.1
libgcc_s.so.1
```

musl 测试需要的 loader 也会复制到 `/lib`，例如 riscv64 的：

```text
/lib/ld-musl-riscv64.so.1
```

## 镜像生成

测试和运行时库收集完成后，脚本会创建 ext4 镜像：

```text
build/<isa>.ext4
```

镜像大小由环境变量 `IMG_SIZE` 控制，单位是 MB，默认 1024：

```sh
IMG_SIZE=256 ./usertests/build.sh riscv64
```

镜像生成流程大致是：

1. 使用 `dd` 创建空文件。
2. 使用 `mke2fs -d build/<isa>/` 或兼容的 `mkfs.ext4 -d build/<isa>/` 格式化并填充镜像。

因此，完整构建镜像时只需要本机有 `dd` 和 `mke2fs`/`mkfs.ext4`，不再需要 `sudo` 或 loop mount 权限。

## 输出位置

一次完整构建后，主要输出如下：

```text
usertests/build/<isa>/        根文件系统 staging 目录
usertests/build/<isa>.ext4    最终 ext4 镜像
```

示例：

```text
usertests/build/riscv64/tests/stdio/key
usertests/build/riscv64/lib/libc.so.6
usertests/build/riscv64.ext4
```

`build.py` 结束时会打印 staging 目录中每个 suite 的顶层内容，便于确认测试是否被正确收集。
