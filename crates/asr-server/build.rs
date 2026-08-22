use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") | Ok("android") => {
            println!("cargo:rustc-link-arg-bin=otoa-asr-server=-Wl,-rpath,$ORIGIN");
        }
        Ok("macos") => {
            println!("cargo:rustc-link-arg-bin=otoa-asr-server=-Wl,-rpath,@loader_path");
        }
        _ => {}
    }
}
