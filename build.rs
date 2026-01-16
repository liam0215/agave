use std::env;
fn main() {
    let src = env::var("QATLIB_SRC").expect("set QATLIB_SRC to qatlib source root");
    let build = env::var("QATLIB_BUILD").unwrap_or_else(|_| format!("{src}/build"));
    println!("cargo:rustc-link-arg=-Wl,--enable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}/lib", build);
}
