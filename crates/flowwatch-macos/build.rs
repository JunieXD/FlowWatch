fn main() {
    println!("cargo:rerun-if-changed=native/interface_stats.c");
    cc::Build::new()
        .file("native/interface_stats.c")
        .warnings(true)
        .compile("flowwatch_interface_stats");
}
