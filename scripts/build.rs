fn main() {
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "qemu-virt-riscv64".to_string());
    let arch = std::env::var("ARCH").unwrap();
    let arch_bits = std::env::var("ARCH_BITS").unwrap();
    
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

    // Link vdso
    let vdso_path = format!("vdso/build/{}{}/vdso.o", arch, arch_bits);
    println!("cargo:rustc-link-arg={}", vdso_path);
    println!("cargo:rerun-if-changed={}", vdso_path);

    // Copy vDSO symbols
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let symbols_src = format!("vdso/build/{}{}/symbols.inc", arch, arch_bits);
    let symbols_dst = format!("{}/symbols.inc", out_dir);
    std::fs::copy(&symbols_src, &symbols_dst).expect("Failed to copy vDSO symbols");
    println!("cargo:rerun-if-changed={}", symbols_src);

    // Linker script
    let linker = format!("scripts/linker/{}.ld", platform);
    println!("cargo:rustc-link-arg=-T{}", linker);
    println!("cargo:rustc-link-arg=-Map=link.map");
    println!("cargo:rerun-if-changed=scripts/linker/{}", linker);
}
