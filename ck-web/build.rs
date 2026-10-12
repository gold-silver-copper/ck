// CEF's library is copied next to the binary (by cef-dll-sys): look for it there. (Windows
// looks there by itself.) On Windows the program carries the manifest CEF's own programs do.
fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
        Ok("windows") => {
            let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ck-web.exe.manifest");
            println!("cargo::rerun-if-changed={}", manifest.display());
            println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
            println!("cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        }
        _ => {}
    }
}
