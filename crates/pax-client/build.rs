fn main() {
    slint_build::compile("ui/client.slint").expect("compile client.slint");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon embed skipped: {e}");
        }
    }
}
