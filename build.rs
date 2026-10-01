use std::env;
fn main() {
    let src = env::var("QATLIB_SRC").expect("set QATLIB_SRC to qatlib source root");
    let build = env::var("QATLIB_BUILD").unwrap_or_else(|_| format!("{src}/build"));
    println!("cargo:rustc-link-search=native={build}");
    println!("cargo:rustc-link-lib=dylib=qat_s");
    println!("cargo:rustc-link-lib=static=usdm_drv");
    println!("cargo:rustc-link-lib=static=osal");
    println!("cargo:rustc-link-lib=static=adf");
    println!("cargo:rustc-link-arg=-Wl,--enable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", build);
    for sys in ["pthread", "dl", "m", "rt", "numa", "crypto", "udev"] {
        println!("cargo:rustc-link-lib=dylib={sys}");
    }
}
