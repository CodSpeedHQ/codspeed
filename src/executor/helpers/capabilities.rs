use std::path::Path;

/// Decode the permitted capability set from a `security.capability` xattr value
/// as a [`caps::Capability::bitmask`]-compatible mask.
///
/// Layout (little-endian `vfs_cap_data`): a `magic_etc` word encoding the
/// revision, followed by one (v1) or two (v2/v3) `{permitted, inheritable}`
/// 32-bit word pairs. The permitted words are concatenated low-to-high into the
/// returned 64-bit mask. Returns `None` for an unknown revision or truncated data.
fn permitted_caps_from_xattr(data: &[u8]) -> Option<u64> {
    const VFS_CAP_REVISION_MASK: u32 = 0xFF00_0000;
    const VFS_CAP_REVISION_1: u32 = 0x0100_0000;
    const VFS_CAP_REVISION_2: u32 = 0x0200_0000;
    const VFS_CAP_REVISION_3: u32 = 0x0300_0000;

    let magic = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
    let words = match magic & VFS_CAP_REVISION_MASK {
        VFS_CAP_REVISION_1 => 1,
        VFS_CAP_REVISION_2 | VFS_CAP_REVISION_3 => 2,
        _ => return None,
    };

    let mut permitted: u64 = 0;
    for word in 0..words {
        let offset = 4 + word * 8; // skip magic, then {permitted, inheritable} pairs
        let value = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?);
        permitted |= (value as u64) << (32 * word);
    }
    Some(permitted)
}

/// Whether `binary` holds every capability in `required` (a mask built from
/// [`caps::Capability::bitmask`]) in its permitted file-capability set.
pub fn binary_has_capabilities(binary: &Path, required: u64) -> bool {
    let Ok(Some(value)) = xattr::get(binary, "security.capability") else {
        return false;
    };
    permitted_caps_from_xattr(&value).is_some_and(|permitted| permitted & required == required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caps::Capability;
    use std::io::Write;

    fn mask(caps: &[Capability]) -> u64 {
        caps.iter().fold(0, |acc, c| acc | c.bitmask())
    }

    /// Build a `VFS_CAP_REVISION_2` xattr value granting `caps` in the permitted set.
    fn vfs_cap_data_v2(caps: &[Capability]) -> Vec<u8> {
        let permitted = mask(caps);
        let mut data = Vec::new();
        data.extend_from_slice(&0x0200_0000u32.to_le_bytes()); // magic_etc
        data.extend_from_slice(&(permitted as u32).to_le_bytes()); // permitted lo
        data.extend_from_slice(&0u32.to_le_bytes()); // inheritable lo
        data.extend_from_slice(&((permitted >> 32) as u32).to_le_bytes()); // permitted hi
        data.extend_from_slice(&0u32.to_le_bytes()); // inheritable hi
        data
    }

    #[test]
    fn decodes_permitted_caps_across_both_words() {
        // CAP_DAC_READ_SEARCH/CAP_SYS_ADMIN live in the low word, CAP_PERFMON/CAP_BPF in the high word.
        let required = mask(&[
            Capability::CAP_BPF,
            Capability::CAP_PERFMON,
            Capability::CAP_SYS_ADMIN,
            Capability::CAP_DAC_READ_SEARCH,
        ]);
        let permitted = permitted_caps_from_xattr(&vfs_cap_data_v2(&[
            Capability::CAP_BPF,
            Capability::CAP_PERFMON,
            Capability::CAP_SYS_ADMIN,
            Capability::CAP_DAC_READ_SEARCH,
        ]))
        .unwrap();
        assert_eq!(permitted, required);
    }

    #[test]
    fn rejects_unknown_revision_and_truncated_data() {
        assert!(permitted_caps_from_xattr(&[]).is_none());
        assert!(permitted_caps_from_xattr(&0xDEAD_BEEFu32.to_le_bytes()).is_none());
        // Revision claims two words but only the magic is present.
        assert!(permitted_caps_from_xattr(&0x0200_0000u32.to_le_bytes()).is_none());
    }

    #[test]
    fn plain_binary_has_no_capabilities() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"\x7fELF").unwrap();
        assert!(!binary_has_capabilities(
            file.path(),
            Capability::CAP_BPF.bitmask()
        ));
    }

    #[test]
    fn missing_path_has_no_capabilities() {
        assert!(!binary_has_capabilities(
            Path::new("/nonexistent/codspeed-memtrack"),
            Capability::CAP_BPF.bitmask()
        ));
    }
}
