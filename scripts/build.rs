fn main() {
    let arch = std::env::var("ARCH").unwrap();
    let arch_bits = std::env::var("ARCH_BITS").unwrap();
    let sysroot = std::env::var("SYSROOT").unwrap_or_default();

    track_kernelx_env_vars();

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

    // Declare the custom cfg names we emit so `rustc --check-cfg` stays happy.
    println!("cargo:rustc-check-cfg=cfg(platform_riscv_common)");
    println!("cargo:rustc-check-cfg=cfg(platform_loongarch_common)");
    println!("cargo:rustc-check-cfg=cfg(arch_riscv64)");
    println!("cargo:rustc-check-cfg=cfg(arch_loongarch64)");

    println!("cargo:rustc-cfg=arch_{}{}", arch, arch_bits);

    // Link C library
    println!("cargo:rustc-link-search=native=clib/build/{}{}", arch, arch_bits);
    println!("cargo:rustc-link-lib=static=kernelx_clib");
    println!(
        "cargo:rerun-if-changed=clib/build/{}{}/libkernelx_clib.a",
        arch, arch_bits
    );

    generate_ext4_bindings(&manifest_dir, &arch, &arch_bits, &sysroot);

    // vDSO symbols
    let symbols_src = format!("vdso/build/{}{}/symbols.inc", arch, arch_bits);
    println!("cargo:rerun-if-changed={}", symbols_src);

    // Linker script
    let linker = format!("scripts/linker/{}{}.ld", arch, arch_bits);
    println!("cargo:rustc-link-arg=-T{}", linker);
    println!("cargo:rustc-link-arg=-Map=link.map");
    println!("cargo:rerun-if-changed={}", linker);
}

fn track_kernelx_env_vars() {
    const ENV_VARS: &[&str] = &[
        "KERNELX_SECOND_DEVICE",
        "KERNELX_SECOND_FSTYPE",
        "KERNELX_SECOND_MOUNTPOINT",
    ];

    for key in ENV_VARS {
        println!("cargo:rerun-if-env-changed={key}");
        println!("cargo:rustc-env={key}={}", std::env::var(key).unwrap_or_default());
    }
}

fn generate_ext4_bindings(manifest_dir: &str, arch: &str, arch_bits: &str, sysroot: &str) {
    use std::path::{Path, PathBuf};

    let target = std::env::var("TARGET").unwrap();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let clang_target = match target_arch.as_str() {
        "riscv64" => "riscv64-unknown-elf",
        "loongarch64" => "loongarch64-unknown-elf",
        _ => panic!("unsupported clang target for target_arch={target_arch}"),
    };

    if target.ends_with("-softfloat") {
        unsafe {
            std::env::set_var("TARGET", target.replace("-softfloat", ""));
        }
    }

    let clib_include = Path::new(manifest_dir).join("clib/include");
    let lwext4_include = Path::new(manifest_dir).join("clib/lib/lwext4/lwext4/include");
    let generated_include =
        Path::new(manifest_dir).join(format!("clib/build/{}{}/lib/lwext4/include", arch, arch_bits));
    let wrapper = Path::new(manifest_dir).join("src/fs/ext4/wrapper.h");

    let mut builder = bindgen::Builder::default()
        .use_core()
        .wrap_unsafe_ops(true)
        .header(wrapper.to_string_lossy())
        .clang_arg(format!("-I{}", clib_include.display()))
        .clang_arg(format!("-I{}", lwext4_include.display()))
        .clang_arg(format!("-I{}", generated_include.display()))
        .clang_arg(format!("--target={clang_target}"))
        .allowlist_function("(ext4|kernelx_ext4)_.*")
        .allowlist_type("ext4_.*")
        .allowlist_type("jbd_.*")
        .allowlist_var("(EXT4|E[A-Z0-9_]+|CONFIG_).*")
        .layout_tests(false)
        .parse_callbacks(Box::new(CustomCargoCallbacks))
        .generate_comments(false);

    if !sysroot.is_empty() {
        builder = builder
            .clang_arg(format!("--sysroot={sysroot}"))
            .clang_arg(format!("-I{sysroot}/include"))
            .clang_arg(format!("-I{sysroot}/usr/include"));
    }
    println!("cargo:rerun-if-env-changed=SYSROOT");

    let bindings = builder.generate().expect("Unable to generate ext4 bindings");

    unsafe {
        std::env::set_var("TARGET", target);
    }

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("ext4_bindings.rs"))
        .expect("Couldn't write ext4 bindings");

    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!(
        "cargo:rerun-if-changed={}",
        generated_include.join("ext4_config.h").display()
    );
}

#[derive(Debug)]
struct CustomCargoCallbacks;

impl bindgen::callbacks::ParseCallbacks for CustomCargoCallbacks {
    fn header_file(&self, filename: &str) {
        println!("cargo:rerun-if-changed={filename}");
    }

    fn include_file(&self, filename: &str) {
        println!("cargo:rerun-if-changed={filename}");
    }

    fn read_env_var(&self, key: &str) {
        println!("cargo:rerun-if-env-changed={key}");
    }
}
