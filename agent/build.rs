use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let web_root = manifest.parent().expect("web root");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let info_plist = manifest.join("macos/Info.plist");
        println!("cargo:rerun-if-changed={}", info_plist.display());
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            info_plist.display()
        );
    }
    println!("cargo:rerun-if-changed={}", web_root.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        web_root.join("index.html").display()
    );
    println!("cargo:rerun-if-changed={}", web_root.join("dist").display());
    println!("cargo:rerun-if-env-changed=EPD_AGENT_SKIP_WEB_BUILD");

    if env::var_os("EPD_AGENT_SKIP_WEB_BUILD").is_none() {
        let status = Command::new("bun")
            .args(["run", "build"])
            .current_dir(web_root)
            .status()
            .expect("failed to start bun; install Bun or set EPD_AGENT_SKIP_WEB_BUILD=1");
        assert!(status.success(), "web build failed");
    }

    let dist = web_root.join("dist");
    let output = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("embedded_assets.rs");
    let mut generated = String::from("pub static WEB_ASSETS: &[(&str, &[u8], &str)] = &[\n");
    collect(&dist, &dist, &mut generated);
    generated.push_str("];\n");
    fs::write(output, generated).expect("write embedded assets");
}

fn collect(root: &Path, directory: &Path, generated: &mut String) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, generated);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("relative asset")
            .to_string_lossy()
            .replace('\\', "/");
        let mime = mime(&path);
        generated.push_str(&format!(
            "    ({relative:?}, include_bytes!({absolute:?}), {mime:?}),\n",
            relative = relative,
            absolute = path.to_string_lossy(),
            mime = mime,
        ));
    }
}

fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
