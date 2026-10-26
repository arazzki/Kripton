fn main() {
    slint_build::compile("ui/appwindow.slint").expect("failed to compile Slint UI");

    #[cfg(target_os = "windows")]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("ProductName", "Kripton")
            .set("FileDescription", "Kripton — Native Windows Living Notch")
            .set("CompanyName", "Kripton")
            .set("LegalCopyright", "Copyright © 2026 Kripton")
            .set("OriginalFilename", "kripton.exe");
        resource
            .compile()
            .expect("failed to embed Windows executable resources");
    }

    println!("cargo:rerun-if-changed=ui/");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
