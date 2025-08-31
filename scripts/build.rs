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

    let target = std::env::var("TARGET").unwrap();
    let cross_compile = std::env::var("CROSS_COMPILE").unwrap_or_else(|_| "riscv64-unknown-elf-".to_string());
    let mut build = cc::Build::new();
    build.target(&target);

    if arch == "riscv" {
        build
            .compiler(&format!("{}gcc", cross_compile))
            .archiver(&format!("{}ar", cross_compile))
            .flag("-march=rv64gc")
            .flag("-mabi=lp64d")
            .flag("-nostdlib")
            .flag("-nostartfiles");
    }

    build.file("resource/riscv/resource.S");
    build.compile("asm");
}

// fn main() {
//     let target = std::env::var("TARGET").unwrap();
//     let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "qemu-virt-riscv64".to_string());
//     let arch = std::env::var("ARCH").unwrap_or_else(|_| "riscv".to_string());
//     let cross_compile = std::env::var("CROSS_COMPILE").unwrap_or_else(|_| "riscv64-unknown-elf-".to_string());
    
//     match platform.as_str() {
//         "qemu-virt-riscv64" => {
//             println!("cargo:rustc-cfg=platform_riscv_common");
//         }
//         _ => {
//             println!("cargo:warning=Unknown platform: {}", platform);
//         }
//     }
//     println!("cargo:rustc-cfg=arch_{}", arch);
    
//     let mut build = cc::Build::new();
//     build.target(&target);

//     if arch == "riscv" {
//         build
//             .compiler(&format!("{}gcc", cross_compile))
//             .archiver(&format!("{}ar", cross_compile))
//             .flag("-march=rv64gc")
//             .flag("-mabi=lp64d")
//             .flag("-nostdlib")
//             .flag("-nostartfiles");
//     }

//     let platform_dir = format!("csrc/platform/{}", platform);
//     let platform_dir = std::path::Path::new(&platform_dir);
//     if std::path::Path::new(&platform_dir).exists() {
//         add_files_from_dir(&mut build, &platform_dir);
//     }
    
//     let arch_dir = format!("csrc/arch/{}", arch);
//     let arch_dir = std::path::Path::new(&arch_dir);
//     if std::path::Path::new(&arch_dir).exists() {
//         add_files_from_dir(&mut build, &arch_dir);
//     }

//     build.file("resource/riscv/resource.S");
    
//     build.compile("asm");
    
//     println!("cargo:rerun-if-changed=csrc/platform/{}", platform);
//     println!("cargo:rerun-if-changed=csrc/arch/{}", arch);
// }

// fn add_files_from_dir(build: &mut cc::Build, path: &std::path::Path) {
//     if let Ok(entries) = fs::read_dir(path) {
//         for entry in entries.flatten() {
//             let path = entry.path();
                
//             if path.is_dir() {
//                 add_files_from_dir(build, &path);
//             } else if let Some(ext) = path.extension() {
//                 if ext == "S" || ext == "s" || ext == "c" {
//                     if let Some(path_str) = path.to_str() {
//                         println!("Adding file: {}", path_str);
//                         build.file(path_str);
//                         println!("cargo:rerun-if-changed={}", path_str);
//                     }
//                 }
//             }
//         }
//     }
// }
