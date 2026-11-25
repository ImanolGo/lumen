//! On Windows: the program's icon and version, as Explorer shows them. Nothing elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=build-aux/windows/lumen.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("build-aux/windows/lumen.ico");
        resource.set("ProductName", "Lumen");
        resource.set("FileDescription", "Lumen");
        resource.set("LegalCopyright", "GPL-3.0-or-later");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=no icon: {e}");
        }
    }
}
