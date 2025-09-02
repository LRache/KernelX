fn main() {
    println!("cargo:rustc-link-search=native=c/build/");
    println!("cargo:rustc-link-lib=static=kernelx_clib");

    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "qemu-virt-riscv64".to_string());
    let arch = std::env::var("ARCH").unwrap_or_else(|_| "riscv".to_string());
    
    match platform.as_str() {
        "qemu-virt-riscv64" => {
            println!("cargo:rustc-cfg=platform_riscv_common");
        }
        _ => {
            println!("cargo:warning=Unknown platform: {}", platform);
        }
    }

    println!("cargo:rustc-cfg=arch_{}", arch);

    let linker = format!("scripts/linker/{}.ld", platform);
    println!("cargo:rustc-link-arg=-T{}", linker);
    println!("cargo:rustc-link-arg=-Map=link.map");
    println!("cargo:rerun-if-changed=scripts/linker/{}", linker);
}
