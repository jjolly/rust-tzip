//! Issue #1: non-ASCII filenames must carry the UTF-8 (EFS) flag bit 11,
//! while ASCII archives must remain byte-identical to the legacy flag.

use tzip::{TorrentZipValidator, TorrentZipWriter};

fn lfh_flag(buf: &[u8]) -> u16 {
    assert_eq!(&buf[0..4], &[0x50, 0x4b, 0x03, 0x04], "first local file header");
    u16::from_le_bytes([buf[6], buf[7]])
}

fn first_cdh_flag(buf: &[u8]) -> u16 {
    let eocd = buf
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .expect("EOCD");
    let cd_offset =
        u32::from_le_bytes([buf[eocd + 16], buf[eocd + 17], buf[eocd + 18], buf[eocd + 19]]) as usize;
    assert_eq!(&buf[cd_offset..cd_offset + 4], &[0x50, 0x4b, 0x01, 0x02], "first central dir header");
    u16::from_le_bytes([buf[cd_offset + 8], buf[cd_offset + 9]])
}

#[test]
fn non_ascii_filename_sets_utf8_flag_in_both_headers() {
    let mut buffer = Vec::new();
    let mut tz = TorrentZipWriter::new(&mut buffer);
    tz.add_file("蝙蝠侠.bin", b"batman rom data").unwrap();
    tz.finish().unwrap();

    assert_eq!(lfh_flag(&buffer), 0x0802, "local header must set EFS (bit 11)");
    assert_eq!(first_cdh_flag(&buffer), 0x0802, "central dir must set EFS (bit 11)");

    let result = TorrentZipValidator::validate(&buffer).unwrap();
    assert!(result.is_valid, "validator must accept the EFS flag: {:?}", result.errors);
    assert_eq!(result.computed_crc32, result.torrentzip_crc32.unwrap());
    assert_eq!(result.files, vec!["蝙蝠侠.bin".to_string()]);
}

#[test]
fn ascii_filenames_keep_the_legacy_flag() {
    let mut buffer = Vec::new();
    let mut tz = TorrentZipWriter::new(&mut buffer);
    tz.add_file("game.bin", b"rom data").unwrap();
    tz.finish().unwrap();

    assert_eq!(lfh_flag(&buffer), 0x0002, "ASCII names must stay byte-identical to trrntzip");
    assert_eq!(first_cdh_flag(&buffer), 0x0002);
    assert!(TorrentZipValidator::validate(&buffer).unwrap().is_valid);
}

#[test]
fn mixed_ascii_and_non_ascii_archive_sorts_and_validates() {
    let mut buffer = Vec::new();
    let mut tz = TorrentZipWriter::new(&mut buffer);
    tz.add_file("zeta.bin", b"z").unwrap();
    tz.add_file("蝙蝠侠.bin", b"b").unwrap();
    tz.add_file("alpha.bin", b"a").unwrap();
    tz.finish().unwrap();

    let result = TorrentZipValidator::validate(&buffer).unwrap();
    assert!(result.is_valid, "mixed archive must validate: {:?}", result.errors);
    // Sorted by lowercase name: alpha < zeta < 蝙蝠侠 (non-ASCII sorts last).
    assert_eq!(result.files, vec!["alpha.bin".to_string(), "zeta.bin".to_string(), "蝙蝠侠.bin".to_string()]);

    // The CJK entry (last) carries the flag; the ASCII entries do not.
    let eocd = buffer.windows(4).rposition(|w| w == [0x50, 0x4b, 0x05, 0x06]).unwrap();
    let cd_offset = u32::from_le_bytes([
        buffer[eocd + 16], buffer[eocd + 17], buffer[eocd + 18], buffer[eocd + 19],
    ]) as usize;
    let flag0 = u16::from_le_bytes([buffer[cd_offset + 8], buffer[cd_offset + 9]]);
    let name0_len = u16::from_le_bytes([buffer[cd_offset + 28], buffer[cd_offset + 29]]) as usize;
    let entry1 = cd_offset + 46 + name0_len;
    let flag1 = u16::from_le_bytes([buffer[entry1 + 8], buffer[entry1 + 9]]);
    assert_eq!(flag0, 0x0002, "alpha.bin keeps the legacy flag");
    assert_eq!(flag1, 0x0002, "zeta.bin keeps the legacy flag");
}
