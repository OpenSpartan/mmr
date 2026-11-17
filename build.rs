//! Puts `assets/icon.ico` into `mmr.exe` as its icon, for Explorer and the taskbar.
//! The MSVC linker takes a compiled resource file as an input, so this writes one
//! itself instead of needing the Windows SDK's resource compiler.

use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const LANG_EN_US: u16 = 0x0409;

/// One resource with a numbered type and name, padded to four bytes.
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    for v in [0xffff, kind, 0xffff, name] {
        out.extend(v.to_le_bytes());
    }
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(LANG_EN_US.to_le_bytes());
    out.extend([0; 8]);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let ico = std::fs::read("assets/icon.ico").expect("can't read assets/icon.ico");
    let word = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let long = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    let count = word(4);
    // A resource file starts with an empty entry.
    let mut res = vec![0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0];
    res.extend([0; 16]);
    // The group lists the images as the icon file does, by resource number instead of offset.
    let mut group: Vec<u8> = [0u16, 1, count].iter().flat_map(|v| v.to_le_bytes()).collect();
    for i in 0..count {
        let e = 6 + 16 * i as usize;
        let (size, offset) = (long(e + 8), long(e + 12));
        resource(&mut res, RT_ICON, i + 1, 0x1010, &ico[offset..offset + size]);
        group.extend(&ico[e..e + 12]);
        group.extend((i + 1).to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    let path = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("icon.res");
    std::fs::write(&path, res).expect("can't write the icon resource");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
