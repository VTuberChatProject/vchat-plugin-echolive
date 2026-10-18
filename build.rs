//! 为当前构建目标生成内置插件的平台清单。
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=manifest.json");
    let template = fs::read_to_string("manifest.json").expect("读取内置插件清单模板");
    let manifest = template
        .replace("__PLATFORM__", &env::var("CARGO_CFG_TARGET_OS").unwrap())
        .replace(
            "__ARCHITECTURE__",
            &env::var("CARGO_CFG_TARGET_ARCH").unwrap(),
        );
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(output.join("echolive.internal.json"), manifest).expect("写入内置插件目标平台清单");
}
