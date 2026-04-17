fn main() {
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "qemu-virt-riscv64".to_string());
    let arch = std::env::var("ARCH").unwrap();
    let arch_bits = std::env::var("ARCH_BITS").unwrap();

    // Symbol table for stack backtrace (debug only)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let symbols_dir = format!("{}/build/{}{}", manifest_dir, arch, arch_bits);
    let symbols_path = format!("{}/symbols.bin", symbols_dir);
    // Create empty placeholder so include_bytes! doesn't fail on first build
    if !std::path::Path::new(&symbols_path).exists() {
        std::fs::create_dir_all(&symbols_dir).ok();
        std::fs::write(&symbols_path, []).ok();
    }
    println!("cargo:rustc-env=KERNELX_SYMBOLS_PATH={}", symbols_path);
    println!("cargo:rerun-if-changed={}", symbols_path);

    match platform.as_str() {
        "qemu-virt-riscv64" => {
            println!("cargo:rustc-cfg=platform_riscv_common");
        }
        _ => {
            println!("cargo:warning=Unknown platform: {}", platform);
        }
    }
    println!("cargo:rustc-cfg=arch_{}{}", arch, arch_bits);

    // Link C library
    println!("cargo:rustc-link-search=native=clib/build/{}{}", arch, arch_bits);
    println!("cargo:rustc-link-lib=static=kernelx_clib");
    println!(
        "cargo:rerun-if-changed=clib/build/{}{}/libkernelx_clib.a",
        arch, arch_bits
    );

    // vDSO symbols
    let symbols_src = format!("vdso/build/{}{}/symbols.inc", arch, arch_bits);
    println!("cargo:rerun-if-changed={}", symbols_src);

    // Linker script
    let linker = format!("scripts/linker/{}{}.ld", arch, arch_bits);
    println!("cargo:rustc-link-arg=-T{}", linker);
    println!("cargo:rustc-link-arg=-Map=link.map");
    println!("cargo:rerun-if-changed={}", linker);
}
