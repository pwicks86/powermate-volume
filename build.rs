//! Embeds a Win32 icon resource into the exe.
//!
//! The tray icon is loaded from src/icon.png at runtime, but Explorer, the
//! Start Menu, the taskbar and the UAC prompt all read the icon from the PE
//! resource table instead -- without this the app shows a blank default icon.
//!
//! The .ico is generated from the same icon.png rather than committed
//! alongside it, so the two can't drift out of sync.

use std::{env, fs::File, io::BufWriter, path::PathBuf};

use image::{
    ExtendedColorType,
    codecs::ico::{IcoEncoder, IcoFrame},
    imageops::FilterType,
};

/// Sizes Windows picks between. The source art is 16x16, so every one of
/// these is an exact integer multiple -- nearest-neighbour keeps the pixel
/// art sharp instead of smearing it.
const ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

const SOURCE_ICON: &str = "src/icon.png";

fn main() {
    println!("cargo::rerun-if-changed={SOURCE_ICON}");
    println!("cargo::rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let ico_path = out_dir.join("icon.ico");

    let source = image::open(SOURCE_ICON)
        .unwrap_or_else(|e| panic!("failed to read {SOURCE_ICON}: {e}"))
        .into_rgba8();

    // Keep the resized buffers alive: IcoFrame borrows the encoded bytes.
    let resized: Vec<_> = ICON_SIZES
        .iter()
        .map(|&size| image::imageops::resize(&source, size, size, FilterType::Nearest))
        .collect();

    let frames: Vec<IcoFrame> = resized
        .iter()
        .map(|img| {
            IcoFrame::as_png(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::Rgba8,
            )
            .expect("failed to encode icon frame")
        })
        .collect();

    let file = File::create(&ico_path)
        .unwrap_or_else(|e| panic!("failed to create {}: {e}", ico_path.display()));
    IcoEncoder::new(BufWriter::new(file))
        .encode_images(&frames)
        .expect("failed to write icon.ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("non-UTF-8 OUT_DIR path"));
    res.compile().expect("failed to compile Windows resources");
}
