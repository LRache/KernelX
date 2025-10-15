fn main() {
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "qemu-virt-riscv64".to_string());
    // let arch = std::env::var("ARCH").unwrap_or_else(|_| "riscv".into());
    let arch = std::env::var("ARCH").unwrap();
    // let arch_bits = std::env::var("ARCH_BITS").unwrap_or_else(|_| "64".into());
    let arch_bits = std::env::var("ARCH_BITS").unwrap();
    
    match platform.as_str() {
        "qemu-virt-riscv64" => {
            println!("cargo:rustc-cfg=platform_riscv_common");
        }
        _ => {
            println!("cargo:warning=Unknown platform: {}", platform);
        }
    }

    println!("cargo:rustc-link-search=native=clib/build/{}{}", arch, arch_bits);
    println!("cargo:rustc-link-lib=static=kernelx_clib");

    println!("cargo:rustc-cfg=arch_{}", std::env::var("ARCH").unwrap_or_else(|_| "riscv".to_string()));

    // Link vdso
    let vdso_path = format!("vdso/build/{}{}/vdso.o", arch, arch_bits);
    println!("cargo:rustc-link-arg={}", vdso_path);
    println!("cargo:rerun-if-changed={}", vdso_path);

    // Linker script
    let linker = format!("scripts/linker/{}.ld", platform);
    println!("cargo:rustc-link-arg=-T{}", linker);
    println!("cargo:rustc-link-arg=-Map=link.map");
    println!("cargo:rerun-if-changed=scripts/linker/{}", linker);
}
