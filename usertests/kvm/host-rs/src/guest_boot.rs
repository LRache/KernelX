use std::{env, fs};

use crate::dtb::{DtbConfig, DtbRange, dtb_node_name};
use crate::kvm::{GuestMapping, Kvm};

pub const GUEST_MEMORY_BASE: usize = 0x8000_0000;
const GUEST_MEMORY_SIZE: usize = 128 * 1024 * 1024;
const GUEST_KERNEL_LOAD_ADDR: usize = 0x8020_0000;
const GUEST_DTB_LOAD_ADDR: usize = 0x8220_0000;
const GUEST_INITRD_LOAD_ADDR: usize = 0x8420_0000;
const DEFAULT_GUEST_IMAGE: &str = "/guest/hello_sbi.bin";

pub const UART0_BASE: usize = 0x1000_0000;
pub const UART0_IRQ: u32 = 10;
pub const RTC_BASE: usize = 0x0010_1000;
pub const RTC_IRQ: u32 = 11;
pub const VIRTIO_BLK_BASE: usize = 0x1000_1000;
pub const VIRTIO_BLK_IRQ: u32 = 1;
pub const PLIC_BASE: usize = 0x0c00_0000;

pub struct GuestBootOptions {
    pub kernel_path: String,
    dtb_path: Option<String>,
    initrd_path: Option<String>,
    pub disk_path: Option<String>,
    bootargs: Vec<String>,
    pub memory_size: usize,
}

pub struct GuestEntry {
    pc: usize,
    a1: usize,
}

pub fn parse_args() -> Result<GuestBootOptions, String> {
    let mut args = env::args().collect::<Vec<_>>();
    let argv0 = args.remove(0);
    let mut kernel_path = None;
    let mut dtb_path = None;
    let mut initrd_path = Some(String::from(""));
    let mut disk_path = None;
    let mut bootargs = Vec::new();
    let mut memory_size = GUEST_MEMORY_SIZE;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage(&argv0);
                std::process::exit(0);
            }
            "-kernel" | "--kernel" => {
                i += 1;
                kernel_path = args.get(i).cloned();
                if kernel_path.is_none() {
                    return Err(format!("{} requires a path argument", args[i - 1]));
                }
            }
            "-dtb" | "--dtb" => {
                i += 1;
                dtb_path = args.get(i).cloned();
                if dtb_path.is_none() {
                    return Err(format!("{} requires a path argument", args[i - 1]));
                }
            }
            "-initrd" | "--initrd" | "--initramfs" => {
                i += 1;
                initrd_path = args.get(i).cloned();
                if initrd_path.is_none() {
                    return Err(format!("{} requires a path argument", args[i - 1]));
                }
            }
            "--no-initrd" | "--no-initramfs" => {
                initrd_path = None;
            }
            "-disk" | "--disk" => {
                i += 1;
                disk_path = args.get(i).cloned();
                if disk_path.is_none() {
                    return Err(format!("{} requires a path argument", args[i - 1]));
                }
            }
            "-append" | "--append" | "--bootargs" => {
                i += 1;
                bootargs.push(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| format!("{} requires a string argument", args[i - 1]))?,
                );
            }
            "--memory-size" => {
                i += 1;
                let value = args.get(i).ok_or_else(|| "invalid --memory-size value".to_string())?;
                memory_size = parse_usize_arg(value).ok_or_else(|| "invalid --memory-size value".to_string())?;
            }
            other if kernel_path.is_none() => {
                kernel_path = Some(other.to_string());
            }
            other => {
                print_usage(&argv0);
                return Err(format!("unexpected argument: {other}"));
            }
        }
        i += 1;
    }

    let kernel_path = kernel_path.unwrap_or_else(|| DEFAULT_GUEST_IMAGE.to_string());
    let initrd_path = initrd_path.and_then(|path| if path.is_empty() { None } else { Some(path) });
    Ok(GuestBootOptions {
        kernel_path,
        dtb_path,
        initrd_path,
        disk_path,
        bootargs,
        memory_size,
    })
}

pub fn prepare_guest(kvm: &Kvm, mapping: GuestMapping, options: &GuestBootOptions) -> Result<GuestEntry, String> {
    load_file_to_guest(
        kvm,
        &options.kernel_path,
        mapping,
        GUEST_KERNEL_LOAD_ADDR,
        &options.kernel_path,
    )?;

    let initrd = if let Some(path) = &options.initrd_path {
        let size = load_file_to_guest(kvm, path, mapping, GUEST_INITRD_LOAD_ADDR, path)?;
        Some(DtbRange {
            start: GUEST_INITRD_LOAD_ADDR,
            end: GUEST_INITRD_LOAD_ADDR + size,
        })
    } else {
        None
    };

    let dtb_blob = if let Some(path) = &options.dtb_path {
        fs::read(path).map_err(|err| format!("read {path}: {err}"))?
    } else {
        let config = DtbConfig {
            memory_base: GUEST_MEMORY_BASE,
            memory_size: options.memory_size,
            bootargs: options.bootargs.clone(),
            stdout_path: dtb_node_name("/soc/serial", UART0_BASE),
            initrd,
            cpu_intc_phandle: 1,
            plic_phandle: 2,
            timebase_frequency: 10_000_000,
            riscv_isa: "rv64imafdch".to_string(),
            mmu_type: "riscv,sv39".to_string(),
        };
        kvm.bus().borrow().build_dtb(&config)
    };

    kvm.copy_to_guest(
        mapping,
        GUEST_DTB_LOAD_ADDR,
        &dtb_blob,
        options.dtb_path.as_deref().unwrap_or("built-in dtb"),
    )?;

    Ok(GuestEntry {
        pc: GUEST_KERNEL_LOAD_ADDR,
        a1: GUEST_DTB_LOAD_ADDR,
    })
}

pub fn boot_guest(kvm: &Kvm, mapping: GuestMapping, entry: GuestEntry) -> Result<(), String> {
    let cpu = kvm.create_cpu()?;
    println!(
        "kvm host test ready: /dev/kvm fd={}, vcpu fd={}",
        kvm.raw_fd(),
        cpu.raw_fd()
    );
    println!(
        "mapped guest memory: guest=[0x{:x},0x{:x}) host={:p}",
        mapping.guest_base,
        mapping.guest_base + mapping.guest_size,
        mapping.host_base
    );
    cpu.init(entry.pc, entry.a1, 0)?;
    cpu.run()
}

fn parse_usize_arg(text: &str) -> Option<usize> {
    if text.is_empty() {
        return None;
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

fn print_usage(argv0: &str) {
    eprintln!(
        "Usage:\n  {argv0} [kernel.bin]\n  {argv0} [-kernel PATH] [-initrd PATH] [-dtb PATH] [-disk PATH] [-append STRING] [--memory-size BYTES]"
    );
}

fn load_file_to_guest(
    kvm: &Kvm,
    path: &str,
    mapping: GuestMapping,
    guest_addr: usize,
    label: &str,
) -> Result<usize, String> {
    let data = fs::read(path).map_err(|err| format!("read {path}: {err}"))?;
    if data.is_empty() {
        return Err(format!("guest image {path} is empty"));
    }
    let size = data.len();
    kvm.copy_to_guest(mapping, guest_addr, &data, label)?;
    Ok(size)
}
