fn main() {
    println!("cargo:rustc-link-lib=framework=System");

    let target = std::env::var("TARGET").unwrap();

    let mut builder = bindgen::Builder::default()
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
        .layout_tests(false);

    if target.contains("apple-ios") {
        let sdk = if target.ends_with("-sim") {
            "iphonesimulator"
        } else {
            "iphoneos"
        };

        let sdk_path = std::process::Command::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("failed to get SDK path")
            .stdout;
        let sdk_path = String::from_utf8(sdk_path).unwrap();
        let sdk_path = sdk_path.trim();

        let clang_target = if target.ends_with("-sim") {
            "arm64-apple-ios-simulator"
        } else if target.starts_with("aarch64") {
            "arm64-apple-ios"
        } else {
            "x86_64-apple-ios"
        };

        builder = builder
            .clang_arg(format!("--target={}", clang_target))
            .clang_arg(format!("-isysroot{}", sdk_path));
    }

    let bindings = builder
        .generate()
        .expect("failed to generate dispatch bindings");

    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("dispatch_sys.rs"))
        .expect("failed to write dispatch bindings");
}
