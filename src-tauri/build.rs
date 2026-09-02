fn main() {
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-framework=ApplicationServices");
    tauri_build::build()
}
