# usertests 构建说明

`usertests` 用来构建用户态测试程序，并把测试产物和运行时库打包进一个 ext4 镜像。构建入口保持为 `build.sh`，实际逻辑在 `build.py` 中。

## 构建入口

在仓库根目录或 `usertests` 目录下都可以调用：

```sh
./usertests/build.sh riscv64
./usertests/build.sh loongarch64
./usertests/build.sh riscv64 kmodule filesystem
./usertests/build.sh riscv64 --tests kmodule,filesystem
./usertests/build.sh riscv64 race/ipc
./usertests/build.sh riscv64 race/ipc/case03_semtimedop_blocked_leak.c
./usertests/build.sh riscv64 basic-glibc/open.c swap/bugs
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

也可以通过命令行选择本次要导入镜像的 suite、目录或源文件，不需要修改源码：

```sh
./usertests/build.sh riscv64 kmodule
./usertests/build.sh riscv64 kmodule filesystem
./usertests/build.sh riscv64 --tests kmodule,filesystem
./usertests/build.sh riscv64 race/ipc
./usertests/build.sh riscv64 race/ipc/case03_semtimedop_blocked_leak.c
./usertests/build.sh riscv64 basic-glibc/open.c
./usertests/build.sh riscv64 kmodule/hello/module.c
./usertests/build.sh riscv64 all
./usertests/build.sh riscv64 kmodule --no-autorun
./usertests/build.sh riscv64 race/ipc --dry-run
./usertests/build.sh --list-tests
```

其中 `all` 表示导入所有带 `Makefile` 的顶层 suite。脚本会校验测试名，拼错或不存在的 suite 会直接报错。

路径选择器相对于 `usertests/` 解析，并支持任意深度：

| 选择器 | 含义 |
| --- | --- |
| `race` | 构建 `race` 暴露的全部 case |
| `race/ipc` | 构建该目录下的全部 case |
| `race/ipc/case03_semtimedop_blocked_leak.c` | 只构建该源文件所属的 case |
| `race/ipc/smp_check.h` | 构建所有依赖该共享头文件的 case |
| `kmodule/hello/module.c` | 构建完整 `hello` case，包括 loader 和内核模块 |
| `tokio/Cargo.toml` | 构建所有声明依赖该文件的 Tokio case |

同一次调用可以混合多个 suite、目录和文件。脚本会合并同一 suite 的选择结果并去重，因此同时指定父目录和其中的文件不会重复构建。`--dry-run` 只显示最终解析出的 case，不执行编译、打包或镜像生成。

每个测试套件必须是 `usertests/<suite>/` 下的一个目录，并且通常提供自己的 `Makefile`。构建脚本会对每个 suite 执行：

```sh
make all ISA=<isa> ARCH=<isa> CASES="<case-id> ..."
```

不同架构会自动传入对应的工具链变量。suite 还必须提供无编译副作用的 `list-cases` target，每行使用：

```text
<case-id>|<source-path> ...|<command-or->
```

其中 case ID 是相对于 suite 的无扩展名逻辑路径；一个 case 可以声明多个源文件或依赖目录。第三列是产物目录内的可执行文件名，`-` 表示该 case 只提供数据或组合产物，不加入 autorun。

普通 suite 可包含 `../mk/case-selection.mk`，复用 `CASES` 校验和 `list-cases` 输出逻辑。

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

每个 suite 的 Makefile 会把每个逻辑 case 单独输出到一个目录。case ID 保留源码目录层次：

```text
<suite>/build/<isa>/output/<case-id>/<artifact>
```

例如：

```text
stdio/build/riscv64/output/key/key
basic-glibc/build/riscv64/output/fork/fork
filesystem/build/riscv64/output/write/write
race/build/riscv64/output/ipc/case03_semtimedop_blocked_leak/case03_semtimedop_blocked_leak
kmodule/build/riscv64/output/hello/hello.ko
```

如果测试需要附带文件，也放在同一个测试目录下。例如：

```text
basic-glibc/build/riscv64/output/unlink/to_unlink.txt
filesystem/build/riscv64/output/write/a.txt
basic-ulib/build/riscv64/output/read/test.txt
```

可执行 case 会将产物目录中的文件安装到其 case ID 的父目录，因此已有顶层测试路径保持不变，多级测试则自然保留目录：

```text
/tests/<suite>/<case-id>
```

例如：

```text
/tests/stdio/key
/tests/basic-glibc/unlink
/tests/basic-glibc/to_unlink.txt
/tests/race/ipc/case03_semtimedop_blocked_leak
/tests/kmodule/hello
/tests/kmodule/hello.ko
```

`list-cases` 中 command 为 `-` 的非运行 case 会保留 case 目录本身，例如 KVM 组合产物放在 `/tests/kvm/guest/hello_sbi/` 下。

## 产物收集

`build.py` 只清理本次选中 case 的旧输出目录：

```text
<suite>/build/<isa>/output/<case-id>
```

然后为同一 suite 一次性执行带 `CASES` 的 `make all`。构建完成后，只收集选中 case 的产物。不同 case 安装到同一路径时会直接报冲突，不再静默覆盖。

任一 suite 编译失败、case 未生成约定的输出目录、命令产物缺失或发生文件冲突，整个构建都会立即失败，不会继续生成不完整镜像。

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

1. 选择整个 suite 且 suite 下存在 `run.list` 时，按其中的非空非注释行生成命令。相对路径解释为 `/tests/<suite>/<line>`，绝对路径保持不变。
2. 显式选择目录或文件时，按 `list-cases` 中匹配 case 的 command 生成命令；这样显式选择的手工或压力用例也可以直接运行。
3. command 为 `-` 的 case 只打包产物，不加入清单。

例如 kmodule suite 中的 `hello` case 会自动生成：

```text
/tests/kmodule/hello
```

新增 case 时需要把它加入 suite 的 `ALL_CASES` 和 source/command 映射，并继续遵循统一输出布局；只有需要特殊参数、顺序或默认过滤时，才需要增加 `run.list`。

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

`build.py` 结束时会递归打印 staging 目录中每个 suite 的文件列表，便于确认多级测试是否被正确收集。
