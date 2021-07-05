extern crate bindgen;

use std::env;
use std::path::PathBuf;

fn main() {
    // Tell cargo to tell rustc to link the trdpap library
    println!("cargo:rustc-link-lib=trdpap");
    println!("cargo:rustc-link-lib=uuid");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=wrapper.h");

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header("wrapper.h")
        // Mock LINUX_X86_64_config
        .clang_arg("-D_GNU_SOURCE")
        .clang_arg("-DPOSIX")
        .clang_arg("-DL_ENDIAN")
        .clang_arg("-DHAS_UUID")
        .clang_arg("-I../trdp/include")
        // Special handling (fd_set is already defined in libc crate)
        .blacklist_type("fd_set")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
