fn main() {
    #[cfg(windows)]
    {
        let version = env!("CARGO_PKG_VERSION");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("ProductName", "Cedar")
            .set("FileDescription", "Cedar Cloudflare Operations Console")
            .set("InternalName", "cedar.exe")
            .set("OriginalFilename", "cedar.exe")
            .set("FileVersion", version)
            .set("ProductVersion", version)
            .set("LegalCopyright", "Copyright © 2026 Cedar");
        resource.compile().expect("failed to embed Cedar resources");
    }
}
