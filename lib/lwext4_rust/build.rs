use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let c_path = PathBuf::from("c/lwext4")
        .canonicalize()
        .expect("cannot canonicalize path");

    let arch = env::var("ARCH").unwrap();
    let arch_bits = env::var("ARCH_BITS").unwrap();
    let kernelx_home = env::var("KERNELX_HOME").unwrap();
    let sysroot = env::var("SYSROOT").unwrap();
    let lwext4_lib = &format!("lwext4-{arch}");
    {
        let status = Command::new("make")
            .args([
                "musl-generic",
                "-C",
                c_path.to_str().expect("invalid path of lwext4"),
                &format!("KERNELX_HOME={kernelx_home}"),
                &format!("ARCH={arch}"),
                &format!("ARCH_BITS={arch_bits}"),
                &format!("SYSROOT={sysroot}"),
            ])
            .arg(format!("ARCH={arch}"))
            .arg(format!(
                "ULIBC={}",
                if cfg!(feature = "std") { "OFF" } else { "ON" }
            ))
            .status()
            .expect("failed to execute process: make lwext4");
        assert!(status.success());
    }
    {
        println!("sysroot: {}", sysroot);
        generates_bindings_to_rust(&sysroot);
    }

    println!("cargo:rustc-link-lib=static={lwext4_lib}");
    println!(
        "cargo:rustc-link-search=native={}",
        c_path.to_str().unwrap()
    );
    println!("cargo:rerun-if-changed=c/wrapper.h");
    println!("cargo:rerun-if-changed={}/src", c_path.to_str().unwrap());
}

fn generates_bindings_to_rust(sysroot: &str) {
    let target = env::var("TARGET").unwrap();
    if target.ends_with("-softfloat") {
        // Clang does not recognize the `-softfloat` suffix
        unsafe { env::set_var("TARGET", target.replace("-softfloat", "")) };
    }

    println!("sysroot: {}", sysroot);

    let bindings = bindgen::Builder::default()
        .use_core()
        .wrap_unsafe_ops(true)
        // The input header we would like to generate bindings for.
        .header("c/wrapper.h")
        .clang_arg(&format!("--sysroot={sysroot}"))
        .clang_arg(&format!("-I{sysroot}/usr/include"))
        .clang_arg("-I./c/lwext4/include")
        .clang_arg("-I./c/lwext4/build_musl-generic/include/")
        .layout_tests(false)
        // Tell cargo to invalidate the built crate whenever any of the included header files changed.
        .parse_callbacks(Box::new(CustomCargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        .expect("Unable to generate bindings");

    // Restore the original target environment variable
    unsafe { env::set_var("TARGET", target) };

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

#[derive(Debug)]
struct CustomCargoCallbacks;
impl bindgen::callbacks::ParseCallbacks for CustomCargoCallbacks {
    fn header_file(&self, filename: &str) {
        add_include(filename);
    }

    fn include_file(&self, filename: &str) {
        add_include(filename);
    }

    fn read_env_var(&self, key: &str) {
        println!("cargo:rerun-if-env-changed={key}");
    }
}

fn add_include(filename: &str) {
    if !Path::new(filename).ends_with("ext4_config.h") {
        println!("cargo:rerun-if-changed={filename}");
    }
}
