fn main() {
    println!("cargo:rustc-link-lib=framework=System");

    let bindings = bindgen::Builder::default()
        .header("src/dispatch.h")
        .allowlist_var("_dispatch_main_q")
        .allowlist_var("DISPATCH_QUEUE_PRIORITY_HIGH")
        .allowlist_var("DISPATCH_QUEUE_PRIORITY_DEFAULT")
        .allowlist_var("DISPATCH_QUEUE_PRIORITY_LOW")
        .allowlist_var("DISPATCH_TIME_NOW")
        .allowlist_function("dispatch_get_global_queue")
        .allowlist_function("dispatch_async_f")
        .allowlist_function("dispatch_after_f")
        .allowlist_function("dispatch_time")
        .layout_tests(false)
        .generate()
        .expect("failed to generate dispatch bindings");

    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("dispatch_sys.rs"))
        .expect("failed to write dispatch bindings");
}
