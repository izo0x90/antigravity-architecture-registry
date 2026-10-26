//! The kinds of install steps beyond downloading and running a program:
//! unpacking (zip, tar.gz, a Debian package's data, all or some members),
//! links, system groups, certificates from the system's store, and files
//! appended to members of an archive. Each knows only its kind; every value
//! comes from the pack.

use std::io::{BufReader, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use crate::manifest::Unpack;

/// Whether member `name` is one of `extract` (a path takes everything
/// under it; none: everything).
fn wanted(name: &str, extract: &[String]) -> bool {
    let name = name.trim_start_matches("./");
    extract.is_empty() || extract.iter().any(|e| name == e || name.strip_prefix(e.as_str()).is_some_and(|rest| rest.starts_with('/')))
}

/// A member path, if it stays inside the folder it's unpacked into.
fn safe(name: &str) -> Option<PathBuf> {
    let path = Path::new(name.trim_start_matches("./"));
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)).then(|| path.to_path_buf())
}

/// Unpacks `archive` into `into`, only the members `extract` names.
pub fn unpack(kind: Unpack, archive: &Path, into: &Path, extract: &[String], scratch: &Path) -> Result<(), String> {
    match kind {
        Unpack::None => Ok(()),
        Unpack::Zip => unzip(archive, into, extract),
        Unpack::TarGz => untar(flate2::read::GzDecoder::new(open(archive)?), into, extract),
        Unpack::Deb => undeb(archive, into, extract, scratch),
    }
}

fn open(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn unzip(archive: &Path, into: &Path, extract: &[String]) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(open(archive)?).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut member = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = member.name().map_err(|e| e.to_string())?.into_owned();
        if !wanted(&name, extract) {
            continue;
        }
        let Some(relative) = safe(&name) else { return Err(format!("{name}: outside the folder it's unpacked into")) };
        let target = into.join(relative);
        if member.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        std::io::copy(&mut member, &mut out).map_err(|e| format!("{name}: {e}"))?;
        if let Some(mode) = member.unix_mode() {
            set_mode(&target, mode)?;
        }
    }
    check_found(extract, |e| {
        (0..zip.len()).any(|i| zip.name_for_index(i).is_some_and(|n| n.is_ok_and(|n| wanted(&n, std::slice::from_ref(e)))))
    })
}

fn untar(reader: impl Read, into: &Path, extract: &[String]) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    // Modes as rwx only: no setuid, setgid or sticky bits from an archive.
    archive.set_preserve_permissions(false);
    let mut found: Vec<bool> = vec![false; extract.len()];
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let name = entry.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        if !wanted(&name, extract) {
            continue;
        }
        for (i, e) in extract.iter().enumerate() {
            found[i] |= wanted(&name, std::slice::from_ref(e));
        }
        // `false`: the member would land outside the folder: refused, said
        // so, never skipped.
        if !entry.unpack_in(into).map_err(|e| format!("{name}: {e}"))? {
            return Err(format!("{name}: outside the folder it unpacks into"));
        }
    }
    match extract.iter().zip(&found).find(|(_, f)| !**f) {
        Some((missing, _)) => Err(format!("the archive has no '{missing}'")),
        None => Ok(()),
    }
}

/// A Debian package: an `ar` archive whose `data.tar.*` holds the files.
fn undeb(archive: &Path, into: &Path, extract: &[String], scratch: &Path) -> Result<(), String> {
    let mut deb = ar::Archive::new(open(archive)?);
    while let Some(member) = deb.next_entry() {
        let mut member = member.map_err(|e| e.to_string())?;
        let name = String::from_utf8_lossy(member.header().identifier()).into_owned();
        let Some(compression) = name.strip_prefix("data.tar") else { continue };
        return match compression {
            ".xz" => {
                // Unpacked to a file first: the decoder writes, it isn't read from.
                let tar = scratch.join(".data.tar");
                let mut out = std::fs::File::create(&tar).map_err(|e| format!("{}: {e}", tar.display()))?;
                lzma_rs::xz_decompress(&mut BufReader::new(&mut member), &mut out).map_err(|e| format!("{name}: {e:?}"))?;
                let result = untar(open(&tar)?, into, extract);
                let _ = std::fs::remove_file(&tar);
                result
            }
            ".zst" => untar(ruzstd::decoding::StreamingDecoder::new(member).map_err(|e| format!("{name}: {e}"))?, into, extract),
            ".gz" => untar(flate2::read::GzDecoder::new(member), into, extract),
            "" => untar(member, into, extract),
            other => Err(format!("{name}: a data archive compressed as '{other}' isn't supported")),
        };
    }
    Err(format!("{}: no data.tar in the package", archive.display()))
}

fn check_found(extract: &[String], has: impl Fn(&String) -> bool) -> Result<(), String> {
    match extract.iter().find(|e| !has(e)) {
        Some(missing) => Err(format!("the archive has no '{missing}'")),
        None => Ok(()),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    // rwx only: no setuid, setgid or sticky bits from an archive.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777)).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

/// A relative symbolic link at `path` pointing to `target`.
pub fn link(path: &Path, target: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if path.symlink_metadata().is_ok() {
        std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, path).map_err(|e| format!("{}: {e}", path.display()));
    #[cfg(not(unix))]
    return Err(format!("{}: links aren't supported here", path.display()));
}

/// Whether a group exists, by the system's group file.
fn group_exists(groups: &str, name: &str) -> bool {
    groups.lines().any(|line| line.split(':').next() == Some(name))
}

/// Whether this runs as root (who may create system groups).
pub fn is_root() -> bool {
    #[cfg(unix)]
    return rustix::process::geteuid().is_root();
    #[cfg(not(unix))]
    return false;
}

/// The system group `name`, created when missing: only as root (`root`),
/// by the system's own tool, within `limit`, never through sudo.
pub fn ensure_group(name: &str, group_file: &Path, create: &[&str], root: bool, limit: std::time::Duration) -> Result<bool, String> {
    let groups = std::fs::read_to_string(group_file).map_err(|e| format!("{}: {e}", group_file.display()))?;
    if group_exists(&groups, name) {
        return Ok(false);
    }
    if !root {
        return Err(format!(
            "the system group '{name}' is missing, and creating it needs root (run the install as root, or create the group first)"
        ));
    }
    let (program, args) = create.split_first().ok_or("no command to create a group")?;
    let mut args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    args.push(name.to_owned());
    crate::engine::run(program, &args, group_file.parent().unwrap_or(Path::new(".")), limit)?;
    Ok(true)
}

/// The PEM certificates in `bundle`.
fn certificates(bundle: &[u8]) -> Vec<x509_parser::pem::Pem> {
    x509_parser::pem::Pem::iter_from_buffer(bundle).filter_map(Result::ok).collect()
}

/// The certificate in the PEM `store` whose subject contains `subject`,
/// as PEM text.
pub fn system_ca(store: &Path, subject: &str) -> Result<Option<String>, String> {
    let bundle = match std::fs::read(store) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", store.display())),
    };
    for pem in certificates(&bundle) {
        let Ok(cert) = pem.parse_x509() else { continue };
        if cert.subject().to_string().contains(subject) {
            return Ok(Some(pem_text(&pem.contents)));
        }
    }
    Ok(None)
}

fn pem_text(der: &[u8]) -> String {
    use base64_engine::encode;
    let body = encode(der);
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

/// Standard base64 (the PEM body).
mod base64_engine {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

/// Little-endian fields of zip records.
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// One central directory entry: its bytes, and where its fields are.
struct CdEntry {
    record: Vec<u8>,
    name: String,
    /// Where its record is in the file.
    at: u64,
}

impl CdEntry {
    const FLAGS: usize = 8;
    const METHOD: usize = 10;
    const CRC: usize = 16;
    const COMPRESSED: usize = 20;
    const SIZE: usize = 24;
    const OFFSET: usize = 42;

    fn offset(&self) -> u64 {
        u64::from(u32_at(&self.record, Self::OFFSET))
    }
}

/// Appends `addition` to the members of the zip `file` whose names match
/// `members`. A launcher in front of the archive is kept, however its
/// offsets count (from the file's start or the archive's). A member already
/// holding it is left alone. Members to change must be stored (not
/// compressed); every other member is copied byte for byte. Returns the
/// members changed.
pub fn patch_archive(file: &Path, members: &[glob::Pattern], addition: &[u8]) -> Result<Vec<String>, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    let mut source = std::io::BufReader::new(open(file)?);
    let len = source.get_ref().metadata().map_err(fail)?.len();
    let (base, _, mut entries, end_and_comment) = directory(&mut source, len, file)?;
    let patched_path = file.with_extension("patching");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&patched_path).map_err(|e| format!("{}: {e}", patched_path.display()))?);
    // The launcher, as it was: everything before the first member. Offsets
    // go on counting as the file counts them (from it, or from its start).
    let first = entries.iter().map(CdEntry::offset).min().unwrap_or(0);
    source.seek(std::io::SeekFrom::Start(0)).map_err(fail)?;
    std::io::copy(&mut (&mut source).take(base + first), &mut out).map_err(fail)?;
    let mut written: u64 = first;
    let mut changed = Vec::new();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| entries[i].offset());
    for i in order {
        let entry = &mut entries[i];
        source.seek(std::io::SeekFrom::Start(base + entry.offset())).map_err(fail)?;
        let mut local = [0u8; 30];
        source.read_exact(&mut local).map_err(fail)?;
        if local[..4] != *b"PK\x03\x04" {
            return Err(format!("{}: {}: no local header where the directory says", file.display(), entry.name));
        }
        let mut name_extra = vec![0; usize::from(u16_at(&local, 26)) + usize::from(u16_at(&local, 28))];
        source.read_exact(&mut name_extra).map_err(fail)?;
        let compressed = u64::from(u32_at(&entry.record, CdEntry::COMPRESSED));
        let descriptor = u16_at(&entry.record, CdEntry::FLAGS) & 0x08 != 0;
        put_u32(&mut entry.record, CdEntry::OFFSET, u32::try_from(written).map_err(|_| "the archive grew past 4 GiB")?);
        if members.iter().any(|m| m.matches(&entry.name)) {
            if u16_at(&entry.record, CdEntry::METHOD) != 0 || descriptor {
                return Err(format!("{}: {} isn't stored plainly, so it can't be appended to", file.display(), entry.name));
            }
            let mut content = vec![0; compressed as usize];
            source.read_exact(&mut content).map_err(fail)?;
            if !contains(&content, addition) {
                if !content.is_empty() && !content.ends_with(b"\n") {
                    content.push(b'\n');
                }
                content.extend_from_slice(addition);
                changed.push(entry.name.clone());
            }
            let crc = crc32fast::hash(&content);
            let size = u32::try_from(content.len()).map_err(|_| "a member past 4 GiB")?;
            for (record, crc_at, sizes) in
                [(&mut local[..], 14, [18, 22]), (&mut entry.record[..], CdEntry::CRC, [CdEntry::COMPRESSED, CdEntry::SIZE])]
            {
                put_u32(record, crc_at, crc);
                put_u32(record, sizes[0], size);
                put_u32(record, sizes[1], size);
            }
            for bytes in [&local[..], &name_extra[..], &content[..]] {
                out.write_all(bytes).map_err(fail)?;
            }
            written += (local.len() + name_extra.len() + content.len()) as u64;
        } else {
            out.write_all(&local).map_err(fail)?;
            out.write_all(&name_extra).map_err(fail)?;
            // The data, and its descriptor (with or without a signature).
            let mut rest = compressed;
            if descriptor {
                let mut peek = [0u8; 4];
                source.seek(std::io::SeekFrom::Current(compressed as i64)).map_err(fail)?;
                source.read_exact(&mut peek).map_err(fail)?;
                source.seek(std::io::SeekFrom::Current(-(compressed as i64) - 4)).map_err(fail)?;
                rest += if peek == *b"PK\x07\x08" { 16 } else { 12 };
            }
            let copied = std::io::copy(&mut (&mut source).take(rest), &mut out).map_err(fail)?;
            if copied != rest {
                return Err(format!("{}: {} is cut short", file.display(), entry.name));
            }
            written += (local.len() + name_extra.len()) as u64 + rest;
        }
    }
    // The directory, then the end record pointing at it.
    let cd_start = u32::try_from(written).map_err(|_| "the archive grew past 4 GiB")?;
    let mut cd_len: u32 = 0;
    for entry in &entries {
        out.write_all(&entry.record).map_err(fail)?;
        cd_len += entry.record.len() as u32;
    }
    let mut end_record = end_and_comment;
    put_u32(&mut end_record, 12, cd_len);
    put_u32(&mut end_record, 16, cd_start);
    out.write_all(&end_record).map_err(fail)?;
    let out = out.into_inner().map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    // The patched file keeps the original's permissions.
    let permissions = std::fs::metadata(file).map_err(|e| e.to_string())?.permissions();
    std::fs::set_permissions(&patched_path, permissions).map_err(|e| e.to_string())?;
    std::fs::rename(&patched_path, file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(changed)
}

/// The archive's directory: where the archive starts, and its entries with
/// where each sits in the file.
fn directory(source: &mut (impl Read + Seek), len: u64, file: &Path) -> Result<(u64, u64, Vec<CdEntry>, Vec<u8>), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    let tail_len = len.min(22 + 65535);
    source.seek(std::io::SeekFrom::Start(len - tail_len)).map_err(fail)?;
    let mut tail = vec![0; tail_len as usize];
    source.read_exact(&mut tail).map_err(fail)?;
    let end = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&i| tail[i..i + 4] == *b"PK\x05\x06" && i + 22 + usize::from(u16_at(&tail, i + 20)) == tail.len())
        .ok_or_else(|| format!("{}: not a zip archive", file.display()))?;
    let (count, cd_size, cd_offset) = (u16_at(&tail, end + 10), u32_at(&tail, end + 12), u32_at(&tail, end + 16));
    if count == u16::MAX || cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(format!("{}: zip64 archives aren't supported", file.display()));
    }
    let cd_at = len - tail_len + end as u64 - u64::from(cd_size);
    let base =
        cd_at.checked_sub(u64::from(cd_offset)).ok_or_else(|| format!("{}: its central directory is out of place", file.display()))?;
    source.seek(std::io::SeekFrom::Start(cd_at)).map_err(fail)?;
    let mut cd = vec![0; cd_size as usize];
    source.read_exact(&mut cd).map_err(fail)?;
    let mut entries = Vec::with_capacity(usize::from(count));
    let mut at = 0;
    while at + 46 <= cd.len() && cd[at..at + 4] == *b"PK\x01\x02" {
        let size = 46 + usize::from(u16_at(&cd, at + 28)) + usize::from(u16_at(&cd, at + 30)) + usize::from(u16_at(&cd, at + 32));
        let record = cd.get(at..at + size).ok_or("a truncated central directory")?.to_vec();
        let name = String::from_utf8_lossy(&record[46..46 + usize::from(u16_at(&record, 28))]).into_owned();
        entries.push(CdEntry { record, name, at: cd_at + at as u64 });
        at += size;
    }
    if entries.len() != usize::from(count) {
        return Err(format!("{}: {} entries listed, {} found", file.display(), count, entries.len()));
    }
    Ok((base, cd_at, entries, tail[end..].to_vec()))
}

/// `content` with `addition` first and as many of its whole blocks (each
/// starting with `block`) as still fit, padded with a line of `pad` to its
/// size; None when it already holds the addition.
fn same_size(content: &[u8], addition: &[u8], block: &[u8], pad: u8) -> Result<Option<Vec<u8>>, String> {
    if contains(content, addition) {
        return Ok(None);
    }
    let mut out = addition.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let starts: Vec<usize> = (0..content.len()).filter(|&i| content[i..].starts_with(block)).collect();
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(content.len());
        // Room is kept for the padding line (at least its newline and one pad).
        if out.len() + (end - start) + 2 > content.len() {
            break;
        }
        out.extend_from_slice(&content[start..end]);
    }
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let room = content.len().checked_sub(out.len()).filter(|r| *r >= 2).ok_or("the member is too small to make room for what's added")?;
    out.extend(std::iter::repeat_n(pad, room - 1));
    out.push(b'\n');
    Ok(Some(out))
}

/// Puts `addition` first in the members of the zip `file` whose names
/// match `members`, in place: each keeps its size (trailing blocks dropped
/// to make room), so a program the archive is a fixed part of still reads
/// it. Members must be stored (not compressed). Returns the members changed.
pub fn patch_archive_in_place(
    file: &Path,
    members: &[glob::Pattern],
    addition: &[u8],
    block: &[u8],
    pad: u8,
) -> Result<Vec<String>, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", file.display());
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(file).map_err(fail)?;
    let len = f.metadata().map_err(fail)?.len();
    let (base, _, entries, _) = directory(&mut f, len, file)?;
    let mut changed = Vec::new();
    for entry in entries.iter().filter(|e| members.iter().any(|m| m.matches(&e.name))) {
        if u16_at(&entry.record, CdEntry::METHOD) != 0 || u16_at(&entry.record, CdEntry::FLAGS) & 0x08 != 0 {
            return Err(format!("{}: {} isn't stored plainly, so it can't be patched", file.display(), entry.name));
        }
        let local_at = base + entry.offset();
        f.seek(std::io::SeekFrom::Start(local_at)).map_err(fail)?;
        let mut local = [0u8; 30];
        f.read_exact(&mut local).map_err(fail)?;
        if local[..4] != *b"PK\x03\x04" {
            return Err(format!("{}: {}: no local header where the directory says", file.display(), entry.name));
        }
        let data_at = local_at + 30 + u64::from(u16_at(&local, 26)) + u64::from(u16_at(&local, 28));
        let mut content = vec![0; u32_at(&entry.record, CdEntry::COMPRESSED) as usize];
        f.seek(std::io::SeekFrom::Start(data_at)).map_err(fail)?;
        f.read_exact(&mut content).map_err(fail)?;
        let Some(patched) = same_size(&content, addition, block, pad).map_err(|e| format!("{}: {}: {e}", file.display(), entry.name))?
        else {
            continue;
        };
        let crc = crc32fast::hash(&patched).to_le_bytes();
        f.seek(std::io::SeekFrom::Start(data_at)).map_err(fail)?;
        f.write_all(&patched).map_err(fail)?;
        f.seek(std::io::SeekFrom::Start(local_at + 14)).map_err(fail)?;
        f.write_all(&crc).map_err(fail)?;
        f.seek(std::io::SeekFrom::Start(entry.at + CdEntry::CRC as u64)).map_err(fail)?;
        f.write_all(&crc).map_err(fail)?;
        changed.push(entry.name.clone());
    }
    f.sync_all().map_err(fail)?;
    Ok(changed)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tar_member_outside_the_folder_is_refused_by_name() {
        // Written as is: the tar crate's own builder won't write `..` paths.
        let mut header = tar::Header::new_old();
        header.as_old_mut().name[..9].copy_from_slice(b"../escape");
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        builder.append(&header, &b"x\n"[..]).unwrap();
        let bytes = builder.into_inner().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let into = dir.path().join("pack");
        std::fs::create_dir_all(&into).unwrap();
        let err = untar(&bytes[..], &into, &[]).unwrap_err();
        assert!(err.contains("../escape") && err.contains("outside"), "{err}");
        assert!(!dir.path().join("escape").exists(), "nothing written outside");
    }

    #[test]
    fn a_missing_group_is_created_only_as_root_and_never_through_sudo() {
        let dir = tempfile::tempdir().unwrap();
        let groups = dir.path().join("group");
        std::fs::write(&groups, "root:x:0:\n").unwrap();
        // The platform's tool: appends the name it's given.
        let tool = dir.path().join("add");
        std::fs::write(&tool, format!("#!/bin/sh\necho \"$1:x:999:\" >> {}\n", groups.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let create = [tool.to_str().unwrap()];
        let limit = std::time::Duration::from_secs(10);
        // Not root: a clear error, nothing run.
        let err = ensure_group("nobody", &groups, &create, false, limit).unwrap_err();
        assert!(err.contains("needs root") && !err.contains("sudo"), "{err}");
        assert!(!std::fs::read_to_string(&groups).unwrap().contains("nobody"));
        // Root: created by the tool; there already: nothing to do.
        assert!(ensure_group("nobody", &groups, &create, true, limit).unwrap());
        assert!(std::fs::read_to_string(&groups).unwrap().contains("nobody:x:999:"));
        assert!(!ensure_group("nobody", &groups, &create, false, limit).unwrap(), "already there, even as not root");
    }
}
