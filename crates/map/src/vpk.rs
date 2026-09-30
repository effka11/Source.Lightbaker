//! Source VPK directory. Models a Hammer map leaves out of the pak live here.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

struct Entry {
    archive: u16,
    offset: u32,
    length: u32,
    preload: Vec<u8>,
}

pub struct Pack {
    dir: PathBuf,
    files: HashMap<String, Entry>,
}

pub(crate) fn search_with(map_path: &Path, report: &(dyn Fn(u64, u64) + Sync)) -> Vec<Pack> {
    let paths = pack_paths(map_path);
    let total = paths
        .iter()
        .map(|path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
        .fold(0u64, u64::saturating_add)
        .max(1);
    let mut done = 0u64;
    report(0, total);
    let mut packs = Vec::new();
    for path in paths {
        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if let Some(pack) = open_reporting(&path, done, total, report) {
            packs.push(pack);
        }
        done = done.saturating_add(size).min(total);
        report(done, total);
    }
    report(total, total);
    packs
}

fn pack_paths(map_path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut dir = map_path.parent();
    for _ in 0..6 {
        let Some(current) = dir else {
            break;
        };
        for child in ["", "garrysmod", "sourceengine", "hl2", "platform"] {
            let base = if child.is_empty() {
                current.to_path_buf()
            } else {
                current.join(child)
            };
            let Ok(read) = fs::read_dir(&base) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let lower = name.to_ascii_lowercase();
                if !lower.ends_with("_dir.vpk")
                    || lower.contains("sound")
                    || lower.contains("texture")
                {
                    continue;
                }
                paths.push(path);
            }
        }
        dir = current.parent();
    }
    paths
}

pub fn read(packs: &[Pack], name: &str) -> Option<Vec<u8>> {
    let key = normalize(name);
    for pack in packs {
        if let Some(bytes) = pack.read(&key) {
            return Some(bytes);
        }
    }
    None
}

fn read_reporting(
    path: &Path,
    done: u64,
    total: u64,
    report: &(dyn Fn(u64, u64) + Sync),
) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok().map(|meta| meta.len()).unwrap_or(0);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 256 * 1024];
    let mut read_at = 0u64;
    loop {
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..read]);
        read_at += read as u64;
        let at = done.saturating_add(read_at.min(size)).min(total);
        report(at, total);
    }
    Some(bytes)
}

fn open_reporting(
    path: &Path,
    done: u64,
    total: u64,
    report: &(dyn Fn(u64, u64) + Sync),
) -> Option<Pack> {
    let data = read_reporting(path, done, total, report)?;
    if data.len() < 28 || u32_at(&data, 0)? != 0x55aa_1234 {
        return None;
    }
    if u32_at(&data, 4)? != 2 {
        return None;
    }
    let tree = u32_at(&data, 8)? as usize;
    if 28 + tree > data.len() {
        return None;
    }
    let mut files = HashMap::new();
    let mut pos = 28usize;
    let end = 28 + tree;
    while pos < end {
        let (ext, next) = cstr(&data, pos)?;
        pos = next;
        if ext.is_empty() {
            break;
        }
        loop {
            let (path, next) = cstr(&data, pos)?;
            pos = next;
            if path.is_empty() {
                break;
            }
            loop {
                let (name, next) = cstr(&data, pos)?;
                pos = next;
                if name.is_empty() {
                    break;
                }
                if pos + 18 > end {
                    return None;
                }
                let preload_len = u16_at(&data, pos + 4)? as usize;
                let archive = u16_at(&data, pos + 6)?;
                let offset = u32_at(&data, pos + 8)?;
                let length = u32_at(&data, pos + 12)?;
                pos += 18;
                let preload = data.get(pos..pos + preload_len)?.to_vec();
                pos += preload_len;
                let key = if path.is_empty() {
                    format!("{name}.{ext}")
                } else {
                    format!("{path}/{name}.{ext}")
                };
                let lower = key.to_ascii_lowercase();
                if matches!(
                    lower.rsplit('.').next(),
                    Some("mdl" | "vvd" | "vtx" | "ppl" | "vhv")
                ) {
                    files.insert(
                        lower,
                        Entry {
                            archive,
                            offset,
                            length,
                            preload,
                        },
                    );
                }
            }
        }
    }
    Some(Pack {
        dir: path.to_path_buf(),
        files,
    })
}

impl Pack {
    fn read(&self, key: &str) -> Option<Vec<u8>> {
        let entry = self.files.get(key)?;
        let mut bytes = entry.preload.clone();
        if entry.length == 0 {
            return Some(bytes);
        }
        let file = if entry.archive == 0x7fff {
            self.dir.clone()
        } else {
            let name = self.dir.file_name()?.to_str()?;
            let replaced = name.replacen("_dir.vpk", &format!("_{:03}.vpk", entry.archive), 1);
            self.dir.with_file_name(replaced)
        };
        let data = fs::read(&file).ok()?;
        let start = if entry.archive == 0x7fff {
            let tree = u32_at(&data, 8)? as usize;
            28 + tree + entry.offset as usize
        } else {
            entry.offset as usize
        };
        let end = start + entry.length as usize;
        bytes.extend(data.get(start..end)?);
        Some(bytes)
    }
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn cstr(data: &[u8], offset: usize) -> Option<(String, usize)> {
    let rest = data.get(offset..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    let text = String::from_utf8_lossy(&rest[..end]).into_owned();
    Some((text, offset + end + 1))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}
