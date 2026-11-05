fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=app.manifest");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().expect("embedding app.rc failed");
}
