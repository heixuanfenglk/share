fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "局域网文件分享");
        res.set("FileDescription", "局域网文件分享");
        if let Err(err) = res.compile() {
            println!("cargo:warning=嵌入 Windows 图标失败: {err}");
            println!("cargo:warning=请确认已安装 Windows SDK / MSVC 构建工具");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
