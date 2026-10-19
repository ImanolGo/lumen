//! Compile the icons and stylesheet in `data/` into a GResource bundle that `main` includes.

use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir=data")
        .arg(format!(
            "--target={}",
            out.join("lumen.gresource").display()
        ))
        .arg("data/lumen.gresource.xml")
        .status()
        .expect("glib-compile-resources not found (it comes with the GLib development files)");
    assert!(status.success(), "glib-compile-resources failed");

    println!("cargo::rerun-if-changed=data/lumen.gresource.xml");
    println!("cargo::rerun-if-changed=data/style.css");
    println!("cargo::rerun-if-changed=data/icons");
}
