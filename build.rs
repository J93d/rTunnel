fn main() {
    slint_build::compile("ui/main.slint")
        .expect("Failed to compile Slint UI — check ui/main.slint for syntax errors");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        res.compile().expect("Failed to run winres to set icon");
    }
}
