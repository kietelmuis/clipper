fn main() {
    // Tell Rust where to find the vcpkg libs
    println!("cargo:rustc-link-search=native=D:\\vcpkg\\installed\\x64-windows\\lib");

    // Link the FFmpeg libs (example, add more if needed)
    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=avformat");
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=static=swscale");
    println!("cargo:rustc-link-lib=static=swresample");
    println!("cargo:rustc-link-lib=static=avfilter");
    println!("cargo:rustc-link-lib=static=avdevice");

    // Link the Windows media libs as dylibs (COM stuff)
    println!("cargo:rustc-link-lib=dylib=mfplat");
    println!("cargo:rustc-link-lib=dylib=mfreadwrite");
    println!("cargo:rustc-link-lib=dylib=mfuuid");
    println!("cargo:rustc-link-lib=dylib=wmcodecdspuuid");
}
