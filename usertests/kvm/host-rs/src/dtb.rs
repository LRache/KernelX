use std::collections::BTreeMap;
use std::mem;

#[derive(Default)]
pub struct DtbConfig {
    pub memory_base: usize,
    pub memory_size: usize,
    pub bootargs: Vec<String>,
    pub stdout_path: String,
    pub initrd: Option<DtbRange>,
    pub cpu_intc_phandle: u32,
    pub plic_phandle: u32,
    pub timebase_frequency: u32,
    pub riscv_isa: String,
    pub mmu_type: String,
}

#[derive(Clone, Copy)]
pub struct DtbRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub struct DtbBuilder {
    strings: StringTable,
    structure: Vec<u8>,
}

#[derive(Default)]
struct StringTable {
    offsets: BTreeMap<String, u32>,
    data: Vec<u8>,
}

impl StringTable {
    fn add(&mut self, name: &str) -> u32 {
        if let Some(offset) = self.offsets.get(name) {
            return *offset;
        }
        let offset = self.data.len() as u32;
        self.offsets.insert(name.to_string(), offset);
        self.data.extend_from_slice(name.as_bytes());
        self.data.push(0);
        offset
    }
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum FdtToken {
    BeginNode = 0x1,
    EndNode = 0x2,
    Prop = 0x3,
    End = 0x9,
}

impl FdtToken {
    fn value(self) -> u32 {
        self as u32
    }
}

#[repr(usize)]
#[derive(Clone, Copy)]
enum FdtHeaderOffset {
    Magic = 0,
    TotalSize = 4,
    OffDtStruct = 8,
    OffDtStrings = 12,
    OffMemRsvmap = 16,
    Version = 20,
    LastCompVersion = 24,
    BootCpuidPhys = 28,
    SizeDtStrings = 32,
    SizeDtStruct = 36,
}

impl DtbBuilder {
    pub fn begin_node(&mut self, name: &str) {
        self.push_token(FdtToken::BeginNode);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.align_structure();
    }

    pub fn end_node(&mut self) {
        self.push_token(FdtToken::EndNode);
    }

    pub fn prop_string(&mut self, name: &str, value: &str) {
        let mut data = value.as_bytes().to_vec();
        data.push(0);
        self.prop_raw(name, &data);
    }

    pub fn prop_string_list(&mut self, name: &str, values: &[&str]) {
        let mut data = Vec::new();
        for value in values {
            data.extend_from_slice(value.as_bytes());
            data.push(0);
        }
        self.prop_raw(name, &data);
    }

    pub fn prop_u32(&mut self, name: &str, value: u32) {
        self.prop_raw(name, &value.to_be_bytes());
    }

    fn prop_u64(&mut self, name: &str, value: u64) {
        self.prop_raw(name, &value.to_be_bytes());
    }

    pub fn prop_cells(&mut self, name: &str, cells: &[u32]) {
        let mut data = Vec::with_capacity(cells.len() * mem::size_of::<u32>());
        for cell in cells {
            data.extend_from_slice(&cell.to_be_bytes());
        }
        self.prop_raw(name, &data);
    }

    pub fn prop_bool(&mut self, name: &str) {
        self.prop_raw(name, &[]);
    }

    pub fn config_dtb(&mut self, config: &DtbConfig) {
        self.begin_node("");
        self.prop_u32("#address-cells", 2);
        self.prop_u32("#size-cells", 2);
        self.prop_string_list("compatible", &["kernelx,kvm-guest", "riscv-virtio"]);
        self.prop_string("model", "KernelX KVM Guest");

        self.begin_node("chosen");
        let bootargs = config.bootargs.join(" ");
        if !bootargs.is_empty() {
            self.prop_string("bootargs", &bootargs);
        }
        if !config.stdout_path.is_empty() {
            self.prop_string("stdout-path", &config.stdout_path);
        }
        if let Some(initrd) = config.initrd {
            self.prop_u64("linux,initrd-start", initrd.start as u64);
            self.prop_u64("linux,initrd-end", initrd.end as u64);
        }
        self.end_node();

        self.begin_node(&dtb_node_name("memory", config.memory_base));
        self.prop_string("device_type", "memory");
        self.prop_cells("reg", &dtb_reg_cells(config.memory_base, config.memory_size));
        self.end_node();

        self.begin_node("cpus");
        self.prop_u32("#address-cells", 1);
        self.prop_u32("#size-cells", 0);
        self.prop_u32("timebase-frequency", config.timebase_frequency);

        self.begin_node("cpu@0");
        self.prop_string("device_type", "cpu");
        self.prop_u32("reg", 0);
        self.prop_string("status", "okay");
        self.prop_string("compatible", "riscv");
        self.prop_string("riscv,isa", &config.riscv_isa);
        self.prop_string("mmu-type", &config.mmu_type);

        self.begin_node("interrupt-controller");
        self.prop_u32("#interrupt-cells", 1);
        self.prop_bool("interrupt-controller");
        self.prop_string("compatible", "riscv,cpu-intc");
        self.prop_u32("phandle", config.cpu_intc_phandle);
        self.end_node();

        self.end_node();
        self.end_node();

        self.begin_node("soc");
        self.prop_u32("#address-cells", 2);
        self.prop_u32("#size-cells", 2);
        self.prop_string("compatible", "simple-bus");
        self.prop_bool("ranges");
    }

    pub fn finish_dtb(mut self) -> Vec<u8> {
        self.end_node();
        self.end_node();
        self.finish_blob()
    }

    fn finish_blob(mut self) -> Vec<u8> {
        self.push_token(FdtToken::End);
        self.align_structure();

        let mut blob = vec![0u8; 40];
        let mut reserve_map = Vec::new();
        reserve_map.extend_from_slice(&0u64.to_be_bytes());
        reserve_map.extend_from_slice(&0u64.to_be_bytes());

        let off_mem_rsvmap = blob.len() as u32;
        blob.extend_from_slice(&reserve_map);
        let off_dt_struct = blob.len() as u32;
        blob.extend_from_slice(&self.structure);
        let off_dt_strings = blob.len() as u32;
        blob.extend_from_slice(&self.strings.data);

        let total_size = blob.len() as u32;
        write_header(&mut blob, FdtHeaderOffset::Magic, 0xd00d_feed);
        write_header(&mut blob, FdtHeaderOffset::TotalSize, total_size);
        write_header(&mut blob, FdtHeaderOffset::OffDtStruct, off_dt_struct);
        write_header(&mut blob, FdtHeaderOffset::OffDtStrings, off_dt_strings);
        write_header(&mut blob, FdtHeaderOffset::OffMemRsvmap, off_mem_rsvmap);
        write_header(&mut blob, FdtHeaderOffset::Version, 17);
        write_header(&mut blob, FdtHeaderOffset::LastCompVersion, 16);
        write_header(&mut blob, FdtHeaderOffset::BootCpuidPhys, 0);
        write_header(
            &mut blob,
            FdtHeaderOffset::SizeDtStrings,
            self.strings.data.len() as u32,
        );
        write_header(&mut blob, FdtHeaderOffset::SizeDtStruct, self.structure.len() as u32);
        blob
    }

    fn prop_raw(&mut self, name: &str, data: &[u8]) {
        self.push_token(FdtToken::Prop);
        self.push_u32(data.len() as u32);
        let offset = self.strings.add(name);
        self.push_u32(offset);
        self.structure.extend_from_slice(data);
        self.align_structure();
    }

    fn push_u32(&mut self, value: u32) {
        self.structure.extend_from_slice(&value.to_be_bytes());
    }

    fn push_token(&mut self, token: FdtToken) {
        self.push_u32(token.value());
    }

    fn align_structure(&mut self) {
        while (self.structure.len() & 0x3) != 0 {
            self.structure.push(0);
        }
    }
}

fn write_header(blob: &mut [u8], offset: FdtHeaderOffset, value: u32) {
    let offset = offset as usize;
    blob[offset..offset + mem::size_of::<u32>()].copy_from_slice(&value.to_be_bytes());
}

pub fn dtb_reg_cells(addr: usize, size: usize) -> [u32; 4] {
    [
        (addr >> 32) as u32,
        (addr & 0xffff_ffff) as u32,
        (size >> 32) as u32,
        (size & 0xffff_ffff) as u32,
    ]
}

pub fn dtb_node_name(prefix: &str, addr: usize) -> String {
    format!("{prefix}@{addr:x}")
}
