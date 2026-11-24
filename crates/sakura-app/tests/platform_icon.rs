//! The executable icon contract can be checked without a Windows resource compiler.

const ICON: &[u8] = include_bytes!("../../../assets/icons/Sakura.ico");
const RESOURCE: &str = include_str!("../windows.rc");

#[test]
fn windows_icon_contains_every_supplied_size() {
    assert_eq!(&ICON[..6], &[0, 0, 1, 0, 7, 0]);
    let sizes: Vec<u16> = ICON[6..]
        .as_chunks::<16>()
        .0
        .iter()
        .take(7)
        .map(|entry| {
            assert_eq!(entry[0], entry[1], "square icon image");
            if entry[0] == 0 {
                256
            } else {
                u16::from(entry[0])
            }
        })
        .collect();
    assert_eq!(sizes, [16, 24, 32, 48, 64, 128, 256]);
}

#[test]
fn windows_resource_exposes_the_icon_as_the_id_gpui_loads() {
    let fields: Vec<&str> = RESOURCE.split_whitespace().collect();
    assert_eq!(&fields[..2], &["1", "ICON"]);
    let relative = fields[2].trim_matches('"');
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(std::fs::read(crate_root.join(relative)).unwrap(), ICON);
}
