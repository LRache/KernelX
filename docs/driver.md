# 驱动和驱动管理

驱动层包括了各种设备驱动的实现，例如块设备驱动、字符设备驱动、网络设备驱动等。驱动层通过统一的接口与内核的其他部分进行交互，提供对硬件设备的访问和控制。

## 驱动层抽象

我们将未匹配驱动的设备称为“设备”，将已匹配驱动的设备称为“驱动”。每个驱动都实现了 `DriverOps` 接口，驱动层提供了最基础的设备操作接口。

```rust
// src/driver/device.rs
pub enum DeviceType {
    Block,
    Char,
    Rtc,
}
// src/driver/driver.rs
pub trait DriverOps: Send + Sync {
    /// 获取驱动名称
    fn name(&self) -> &str;
    /// 获取设备名称
    fn device_name(&self) -> String;
    /// 获取设备类型
    fn device_type(&self) -> DeviceType;
    /// 将驱动转换为具体设备驱动接口
    fn as_block_driver(self: Arc<Self>) -> Option<Arc<dyn BlockDriverOps>>;
    fn as_char_driver(self: Arc<Self>) -> Option<Arc<dyn CharDriverOps>>;
    fn as_rtc_driver(self: Arc<Self>) -> Option<Arc<dyn RTCDriverOps>>;
    /// 处理中断
    fn handle_interrupt(&self);
}
```

`DriverOps` 定义了设备驱动的基本接口，包括获取驱动名称、设备名称、设备类型等方法。同时，提供了将通用驱动转换为具体设备驱动接口的方法，例如块设备驱动、字符设备驱动等。每个具体的设备驱动需要实现相应的接口，以便内核能够通过统一的方式访问和控制不同类型的设备。

```rust
// src/driver/driver.rs
pub trait BlockDriverOps: DriverOps + Downcast {
    /// 读取和写入块设备
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()>;
    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()>;
    fn read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> ;
    fn write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()>;
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()>;
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()>;

    /// 刷新缓存
    fn flush(&self) -> Result<(), ()>;

    /// 获取块设备信息
    fn get_block_size(&self) -> u32;
    fn get_block_count(&self) -> u64;
}

pub trait CharDriverOps: DriverOps + Downcast {
    /// 读写字符设备
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn read(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize>;
    /// 等待事件
    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>>;
    fn wait_event_cancel(&self);
    /// 控制操作
    fn ioctl(&self, _request: usize, _arg: usize, _addrspace: &AddrSpace) -> SysResult<usize>;
}
```

具体的驱动只要实现了相应的接口，就可以被内核识别和使用，接入 `devtmpfs` 并暴露给用户程序。例如，块设备驱动需要实现 `BlockDriverOps` 接口，而字符设备驱动需要实现 `CharDriverOps` 接口。实现了 `CharDriverOps` 接口的设备可以直接被 `CharFile` 使用，从而实现对设备的读写操作。

## 驱动注册与匹配

各种驱动的匹配逻辑主要依赖于匹配器，匹配器输入一个设备信息，如果匹配成功，则返回对应的驱动实例。所有的匹配器都由具体的驱动实现提供，驱动层在初始化的时候，会静态注册这些内核支持的驱动匹配器。匹配器通常通过传入的设备的兼容字符串（`compatible`）来判断是否匹配成功。

```rust
// src/driver/device.rs
pub struct Device<'a> {
    mmio_base: usize,
    mmio_size: usize,
    name: &'a str,
    compatible: &'a str,
    interrupt_number: Option<u32>,
}

// src/driver/matcher.rs
pub trait DriverMatcher: Send + Sync {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>>;
}
```

驱动匹配的具体流程：

1. 体系结构相关代码发现设备后，构造 `Device` 结构体，包含设备的 MMIO 地址、名称、兼容字符串、中断号等信息，调用 `found_device` 接口将设备传递给驱动子系统。

2. 驱动子系统遍历所有已注册的驱动匹配器，调用每个匹配器的 `try_match` 方法，传入设备信息。如果某个匹配器成功匹配到设备，则返回对应的驱动实例，并初始化设备。

3. 驱动子系统将驱动实例添加到内核的设备列表中，如果有，则标记这个驱动的中断号，同时向 `devtmpfs` 注册对应的设备文件，`devtmpfs` 会根据驱动的设备类型创建相应的设备文件节点。

## 驱动设备管理和接口

驱动层主要维护了三个数据结构：

1. 匹配器列表：用于存储所有已注册的驱动匹配器，驱动子系统在初始化时会将各个驱动的匹配器添加到这个列表中。

2. 中断号映射表：用于将设备的中断号映射到对应的驱动实例，当中断发生时，内核可以通过这个映射表找到对应的驱动并调用其中断处理方法。

3. 驱动名称-驱动映射表：用于根据驱动名称快速查找对应的驱动实例，方便内核和用户程序访问设备。

```rust
// src/driver/manager.rs
static MATCHERS: RwLock<Vec<&'static dyn DriverMatcher>> = RwLock::new(Vec::new());
static INTERRUPT_MAP: RwLock<BTreeMap<u32, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());
static DRIVERS: RwLock<BTreeMap<String, Arc<dyn DriverOps>>> = RwLock::new(BTreeMap::new());

/// 用于体系结构层发现设备时调用
pub fn found_device(device: &Device);
/// 注册已匹配的驱动，跳过匹配过程
pub fn register_matched_driver(driver: Arc<dyn DriverOps>);
/// 获取驱动
pub fn get_block_driver(name: &str) -> Option<Arc<dyn BlockDriverOps>>;
pub fn get_char_driver(name: &str) -> Option<Arc<dyn CharDriverOps>>;
pub fn get_rtc_driver(name: &str) -> Option<Arc<dyn RTCDriverOps>>;
/// 处理中断，用于上层发生中断时调用
pub fn handle_interrupt(interrupt_number: u32);
```

## 设备中断处理

设备中断是实现事件驱动的基础。当设备发生中断时，内核会调用驱动子系统的 `handle_interrupt` 方法，传入中断号。驱动子系统会查找中断号映射表，找到对应的驱动实例，并调用驱动的 `handle_interrupt` 方法进行处理。

以 ns16550a 串口驱动为例，当串口接收到数据时，会触发中断，内核调用驱动子系统的 `handle_interrupt` 方法，驱动子系统找到 ns16550a 驱动实例，并调用其 `handle_interrupt` 方法。驱动会读取接收到的数据，并将数据存入内部缓冲区，同时唤醒等待数据的用户进程。

通过这种方式，驱动层实现了对设备中断的统一管理和处理，使得内核能够高效地响应硬件事件，提高系统的性能和响应速度。

## 支持的驱动

1. 块设备驱动

- virtio-blk

- starfive-sdio (`compatible = "snps,dw-mshc"`)

2. 字符设备驱动

- ns16550 串口 (`compatible = "ns16550a"`)

- RISCV openSBI 调用

3. RTC 设备驱动

- goldfish RTC (`compatible = "google,goldfish-rtc"`) KernelX 支持获取当前日期和时间
