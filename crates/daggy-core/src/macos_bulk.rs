//! Minimal, lossless wrapper around macOS `getattrlistbulk(2)`.
//!
//! The unsafe boundary is deliberately confined here. Records are counted from
//! the syscall return value, every offset is checked against its record, names
//! stay as raw `OsString` bytes, and symlinks are never followed.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use crate::scanner::RawEntry;
use crate::Kind;

const BUFFER_SIZE: usize = 256 * 1024;

const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_TOTALSIZE: u32 = 0x0000_0002;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;

const FSOPT_NOFOLLOW: u64 = 0x0000_0001;
const FSOPT_PACK_INVAL_ATTRS: u64 = 0x0000_0008;

#[repr(C)]
struct AttrList {
    bitmap_count: u16,
    reserved: u16,
    common: u32,
    volume: u32,
    directory: u32,
    file: u32,
    fork: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ReturnedAttrs {
    common: u32,
    volume: u32,
    directory: u32,
    file: u32,
    fork: u32,
}

unsafe extern "C" {
    fn getattrlistbulk(
        dir_fd: libc::c_int,
        attributes: *mut AttrList,
        buffer: *mut libc::c_void,
        buffer_size: libc::size_t,
        options: u64,
    ) -> libc::c_int;
}

pub(crate) fn read_dir(path: &Path) -> io::Result<Vec<RawEntry>> {
    let directory = File::open(path)?;
    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut entries = Vec::new();
    loop {
        let mut attributes = AttrList {
            bitmap_count: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            common: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_DEVID
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_MODTIME
                | ATTR_CMN_FILEID,
            volume: 0,
            directory: 0,
            file: ATTR_FILE_LINKCOUNT | ATTR_FILE_TOTALSIZE | ATTR_FILE_ALLOCSIZE,
            fork: 0,
        };
        let count = loop {
            // SAFETY: `directory` keeps a valid fd alive, the attrlist layout
            // matches sys/attr.h, and `buffer` is writable for its full length.
            let result = unsafe {
                getattrlistbulk(
                    directory.as_raw_fd(),
                    &mut attributes,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS,
                )
            };
            if result >= 0 {
                break result as usize;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(error);
            }
        };
        if count == 0 {
            break;
        }
        parse_records(&buffer, count, &mut entries)?;
    }
    Ok(entries)
}

fn parse_records(buffer: &[u8], count: usize, entries: &mut Vec<RawEntry>) -> io::Result<()> {
    let mut record_start = 0usize;
    for _ in 0..count {
        let record_len = read_u32(buffer, record_start)? as usize;
        let record_end = record_start
            .checked_add(record_len)
            .filter(|end| record_len >= 24 && *end <= buffer.len())
            .ok_or_else(invalid_record)?;
        entries.push(parse_record(buffer, record_start, record_end)?);
        record_start = record_end;
    }
    Ok(())
}

fn parse_record(buffer: &[u8], start: usize, end: usize) -> io::Result<RawEntry> {
    let mut cursor = start + 4;
    let returned = ReturnedAttrs {
        common: read_u32_at(buffer, &mut cursor, end)?,
        volume: read_u32_at(buffer, &mut cursor, end)?,
        directory: read_u32_at(buffer, &mut cursor, end)?,
        file: read_u32_at(buffer, &mut cursor, end)?,
        fork: read_u32_at(buffer, &mut cursor, end)?,
    };
    if returned.common & ATTR_CMN_NAME == 0 || returned.common & ATTR_CMN_OBJTYPE == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "getattrlistbulk omitted required attributes",
        ));
    }

    let name_ref = cursor;
    let name_offset = read_i32_at(buffer, &mut cursor, end)?;
    let name_len = read_u32_at(buffer, &mut cursor, end)? as usize;
    let name_start = (name_ref as i64)
        .checked_add(name_offset as i64)
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(invalid_record)?;
    let name_end = name_start
        .checked_add(name_len)
        .ok_or_else(invalid_record)?;
    if name_start < start || name_end > end {
        return Err(invalid_record());
    }
    let mut name = &buffer[name_start..name_end];
    if name.last() == Some(&0) {
        name = &name[..name.len() - 1];
    }

    let dev = if returned.common & ATTR_CMN_DEVID != 0 {
        read_u32_at(buffer, &mut cursor, end)? as u64
    } else {
        0
    };
    let object_type = read_u32_at(buffer, &mut cursor, end)?;
    let mtime = if returned.common & ATTR_CMN_MODTIME != 0 {
        let seconds = read_i64_at(buffer, &mut cursor, end)?;
        let _nanoseconds = read_i64_at(buffer, &mut cursor, end)?;
        seconds
    } else {
        0
    };
    let inode = if returned.common & ATTR_CMN_FILEID != 0 {
        read_u64_at(buffer, &mut cursor, end)?
    } else {
        0
    };
    let link_count = if returned.file & ATTR_FILE_LINKCOUNT != 0 {
        read_u32_at(buffer, &mut cursor, end)?
    } else {
        1
    };
    let size = if returned.file & ATTR_FILE_TOTALSIZE != 0 {
        read_u64_at(buffer, &mut cursor, end)?
    } else {
        0
    };
    let allocated = if returned.file & ATTR_FILE_ALLOCSIZE != 0 {
        read_u64_at(buffer, &mut cursor, end)?
    } else {
        size
    };

    Ok(RawEntry {
        name: OsString::from_vec(name.to_vec()),
        kind: match object_type {
            1 => Kind::File,
            2 => Kind::Dir,
            5 => Kind::Symlink,
            _ => Kind::Other,
        },
        size,
        alloc_size: allocated,
        mtime,
        inode,
        dev,
        link_count,
    })
}

fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "malformed getattrlistbulk record",
    )
}

fn read_u32(buffer: &[u8], offset: usize) -> io::Result<u32> {
    let bytes = buffer.get(offset..offset + 4).ok_or_else(invalid_record)?;
    Ok(u32::from_ne_bytes(
        bytes.try_into().map_err(|_| invalid_record())?,
    ))
}

fn read_u32_at(buffer: &[u8], cursor: &mut usize, end: usize) -> io::Result<u32> {
    let next = cursor
        .checked_add(4)
        .filter(|next| *next <= end)
        .ok_or_else(invalid_record)?;
    let value = read_u32(buffer, *cursor)?;
    *cursor = next;
    Ok(value)
}

fn read_i32_at(buffer: &[u8], cursor: &mut usize, end: usize) -> io::Result<i32> {
    Ok(read_u32_at(buffer, cursor, end)? as i32)
}

fn read_u64_at(buffer: &[u8], cursor: &mut usize, end: usize) -> io::Result<u64> {
    let next = cursor
        .checked_add(8)
        .filter(|next| *next <= end)
        .ok_or_else(invalid_record)?;
    let bytes = buffer.get(*cursor..next).ok_or_else(invalid_record)?;
    *cursor = next;
    Ok(u64::from_ne_bytes(
        bytes.try_into().map_err(|_| invalid_record())?,
    ))
}

fn read_i64_at(buffer: &[u8], cursor: &mut usize, end: usize) -> io::Result<i64> {
    Ok(read_u64_at(buffer, cursor, end)? as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_directory_without_following_symlinks() {
        let root = std::env::temp_dir().join(format!("daggy-bulk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("folder/file"), b"123").unwrap();
        std::os::unix::fs::symlink(root.join("folder"), root.join("link")).unwrap();
        let entries = read_dir(&root).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.name == "folder" && entry.kind == Kind::Dir));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "link" && entry.kind == Kind::Symlink));
        std::fs::remove_dir_all(root).unwrap();
    }
}
