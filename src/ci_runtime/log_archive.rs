//! Inert native log inventories. Integrity and provider observations grant no authority.
use anyhow::{Context, Result, ensure};
use caseless::Caseless;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;

/// Hard ceilings match the existing completion collector. Callers can only lower them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub bytes: u64,
    pub expanded_bytes: u64,
    pub directory_bytes: u64,
    pub members: u64,
    pub requests: u64,
    pub seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 512 * 1024 * 1024,
            expanded_bytes: 512 * 1024 * 1024,
            directory_bytes: 16 * 1024 * 1024,
            members: 10000,
            requests: 256,
            seconds: 360,
        }
    }
}
pub struct Budget {
    limits: Limits,
    bytes: u64,
    expanded: u64,
    members: u64,
    requests: u64,
    directory_bytes: u64,
    deadline: Instant,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceFailure {
    DeadlineExhausted,
    ByteBudgetExhausted,
    RequestBudgetExhausted,
    DirectoryBudgetExhausted,
    MemberBudgetExhausted,
    ExpandedBudgetExhausted,
}
impl std::fmt::Display for ResourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "custody resource limit: {self:?}")
    }
}
impl std::error::Error for ResourceFailure {}

impl Budget {
    pub fn new(limits: Limits) -> Result<Self> {
        let max = Limits::default();
        ensure!(
            limits.bytes <= max.bytes
                && limits.expanded_bytes <= max.expanded_bytes
                && limits.directory_bytes <= max.directory_bytes
                && limits.members <= max.members
                && limits.requests <= max.requests
                && limits.seconds <= max.seconds,
            "custody ceilings cannot be raised"
        );
        Ok(Self {
            deadline: Instant::now() + Duration::from_secs(limits.seconds),
            limits,
            bytes: 0,
            expanded: 0,
            members: 0,
            requests: 0,
            directory_bytes: 0,
        })
    }
    pub fn check(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(ResourceFailure::DeadlineExhausted.into());
        }
        Ok(())
    }
    pub fn remaining(&self) -> Result<Duration> {
        self.check()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
    pub fn request(&mut self) -> Result<()> {
        self.check()?;
        self.requests += 1;
        if self.requests > self.limits.requests {
            return Err(ResourceFailure::RequestBudgetExhausted.into());
        }
        Ok(())
    }
    pub fn charge_bytes(&mut self, size: u64) -> Result<()> {
        self.check()?;
        self.bytes = self.bytes.checked_add(size).context("byte overflow")?;
        if self.bytes > self.limits.bytes {
            return Err(ResourceFailure::ByteBudgetExhausted.into());
        }
        Ok(())
    }
    pub fn usage(&self) -> serde_json::Value {
        serde_json::json!({"bytes":self.bytes,"expanded_bytes":self.expanded,
            "directory_bytes":self.directory_bytes,"members":self.members,
            "requests":self.requests,"deadline_exhausted":Instant::now() >= self.deadline,
            "limits":self.limits})
    }
}
impl Default for Budget {
    fn default() -> Self {
        Self::new(Limits::default()).expect("fixed custody ceilings")
    }
}

/// Open inert input without following a replaceable final symlink or blocking on a FIFO.
pub(super) fn regular_file(path: &Path) -> Result<File> {
    #[cfg(unix)]
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    #[cfg(not(unix))]
    let file = {
        ensure!(
            std::fs::symlink_metadata(path)?.file_type().is_file(),
            "capture must be a regular file"
        );
        File::open(path)?
    };
    ensure!(file.metadata()?.is_file(), "capture must be a regular file");
    Ok(file)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub raw_name_hex: String,
    pub size: u64,
    pub sha256: String,
    pub directory: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub sha256: String,
    pub bytes: u64,
    pub members: BTreeMap<String, Member>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveState {
    IdenticalRawArchive,
    EquivalentMemberContent,
    OmissionOnlyRegression,
    AddedOrChangedConflict,
    Invalid,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub state: ArchiveState,
    pub missing_members: Vec<String>,
    pub added_members: Vec<String>,
    pub changed_members: Vec<String>,
}

pub fn inspect(path: &Path) -> Result<Inventory> {
    inspect_with(path, &mut Budget::default())
}

/// Raw capture facts remain available even when ZIP admission fails.
pub fn raw_integrity(path: &Path, budget: &mut Budget) -> Result<(u64, String)> {
    let mut file = regular_file(path)?;
    let bytes = file.metadata()?.len();
    budget.charge_bytes(bytes)?;
    Ok((bytes, hash_stream(&mut file, bytes, budget)?))
}

fn read_at(file: &mut File, offset: u64, length: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = vec![0; length];
    file.read_exact(&mut raw)?;
    Ok(raw)
}
fn u16_at(raw: &[u8], index: usize) -> u16 {
    u16::from_le_bytes([raw[index], raw[index + 1]])
}
fn u32_at(raw: &[u8], index: usize) -> u32 {
    u32::from_le_bytes(raw[index..index + 4].try_into().expect("checked record"))
}
fn u64_at(raw: &[u8], index: usize) -> u64 {
    u64::from_le_bytes(raw[index..index + 8].try_into().expect("checked record"))
}

/// Count actual directory records before the ZIP library allocates its inventory.
fn directory(file: &mut File, bytes: u64, budget: &mut Budget) -> Result<Vec<(Vec<u8>, u64)>> {
    ensure!(bytes >= 22, "missing ZIP footer");
    let tail_len = bytes.min(65535 + 22) as usize;
    let tail_start = bytes - tail_len as u64;
    let tail = read_at(file, tail_start, tail_len)?;
    let candidates = (0..=tail.len() - 22)
        .filter(|&i| {
            tail[i..i + 4] == *b"PK\x05\x06"
                && i + 22 + u16_at(&tail, i + 20) as usize == tail.len()
        })
        .collect::<Vec<_>>();
    ensure!(candidates.len() == 1, "missing or ambiguous ZIP footer");
    let i = candidates[0];
    ensure!(
        u16_at(&tail, i + 4) == 0
            && u16_at(&tail, i + 6) == 0
            && u16_at(&tail, i + 8) == u16_at(&tail, i + 10),
        "split ZIP is unsupported"
    );
    let eocd = tail_start + i as u64;
    let (mut count, mut size, mut start) = (
        u16_at(&tail, i + 10) as u64,
        u32_at(&tail, i + 12) as u64,
        u32_at(&tail, i + 16) as u64,
    );
    let mut stop = eocd;
    let has_locator = eocd >= 20 && read_at(file, eocd - 20, 4)? == b"PK\x06\x07";
    if has_locator {
        let locator = read_at(file, eocd - 20, 20)?;
        ensure!(
            u32_at(&locator, 4) == 0 && u32_at(&locator, 16) == 1,
            "split ZIP64 is unsupported"
        );
        stop = u64_at(&locator, 8);
        ensure!(
            stop.checked_add(56).is_some_and(|end| end <= eocd - 20),
            "invalid ZIP64 offset"
        );
        let footer = read_at(file, stop, 56)?;
        let length = u64_at(&footer, 4);
        ensure!(
            footer[..4] == *b"PK\x06\x06"
                && length >= 44
                && length <= budget.limits.directory_bytes
                && stop.checked_add(12 + length) == Some(eocd - 20),
            "invalid ZIP64 footer framing"
        );
        ensure!(
            u32_at(&footer, 16) == 0
                && u32_at(&footer, 20) == 0
                && u64_at(&footer, 24) == u64_at(&footer, 32),
            "split ZIP64 is unsupported"
        );
        let wide = (
            u64_at(&footer, 32),
            u64_at(&footer, 40),
            u64_at(&footer, 48),
        );
        ensure!(
            (count == 0xffff || count == wide.0)
                && (size == 0xffffffff || size == wide.1)
                && (start == 0xffffffff || start == wide.2),
            "legacy/ZIP64 footer identity differs"
        );
        (count, size, start) = wide;
    } else {
        ensure!(
            count != 0xffff && size != 0xffffffff && start != 0xffffffff,
            "missing ZIP64 footer"
        );
    }
    if size > budget.limits.directory_bytes {
        return Err(ResourceFailure::DirectoryBudgetExhausted.into());
    }
    ensure!(
        start.checked_add(size) == Some(stop),
        "directory framing differs"
    );
    budget.directory_bytes = budget
        .directory_bytes
        .checked_add(size)
        .context("directory byte overflow")?;
    if budget.directory_bytes > budget.limits.directory_bytes {
        return Err(ResourceFailure::DirectoryBudgetExhausted.into());
    }
    let mut position = start;
    let mut records = Vec::new();
    let mut raw_names = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    let mut ranges = Vec::new();
    let mut expanded = 0_u64;
    while position < stop {
        budget.check()?;
        ensure!(position + 46 <= stop, "truncated directory record");
        let fixed = read_at(file, position, 46)?;
        ensure!(
            fixed[..4] == *b"PK\x01\x02"
                && u16_at(&fixed, 8) & !0x080e == 0
                && matches!(u16_at(&fixed, 10), 0 | 8)
                && u16_at(&fixed, 34) == 0,
            "unsupported ZIP directory variant"
        );
        if budget.members + records.len() as u64 >= budget.limits.members {
            return Err(ResourceFailure::MemberBudgetExhausted.into());
        }
        let (name_len, extra_len, comment_len) = (
            u16_at(&fixed, 28) as usize,
            u16_at(&fixed, 30) as usize,
            u16_at(&fixed, 32) as usize,
        );
        ensure!(
            name_len > 0
                && name_len <= 4096
                && position + 46 + (name_len + extra_len + comment_len) as u64 <= stop,
            "directory fields exceed bounds"
        );
        let fields = read_at(file, position + 46, name_len + extra_len)?;
        let name_raw = fields[..name_len].to_vec();
        let name =
            std::str::from_utf8(&name_raw).context("unsupported non-UTF8 raw member name")?;
        let canonical = name.strip_suffix('/').unwrap_or(name);
        ensure!(
            !canonical.is_empty()
                && !name.contains('\\')
                && !name.contains(':')
                && !name.chars().any(char::is_control)
                && canonical
                    .split('/')
                    .all(|p| !p.is_empty() && p != "." && p != ".."),
            "unsafe or aliased member name"
        );
        let alias = canonical
            .nfd()
            .default_case_fold()
            .nfd()
            .collect::<String>();
        ensure!(
            raw_names.insert(name_raw.clone()) && aliases.insert(alias),
            "duplicate or ambiguous member name"
        );
        let (mut uncompressed, mut compressed, mut offset) = (
            u32_at(&fixed, 24) as u64,
            u32_at(&fixed, 20) as u64,
            u32_at(&fixed, 42) as u64,
        );
        let mut cursor = name_len;
        let mut zip64 = false;
        while cursor < fields.len() {
            ensure!(cursor + 4 <= fields.len(), "truncated extra field");
            let (kind, length) = (
                u16_at(&fields, cursor),
                u16_at(&fields, cursor + 2) as usize,
            );
            cursor += 4;
            ensure!(cursor + length <= fields.len(), "truncated extra payload");
            if kind == 1 {
                ensure!(!zip64, "duplicate ZIP64 extra field");
                zip64 = true;
                let mut p = cursor;
                for value in [&mut uncompressed, &mut compressed, &mut offset] {
                    if *value == 0xffffffff {
                        ensure!(p + 8 <= cursor + length, "missing ZIP64 value");
                        *value = u64_at(&fields, p);
                        p += 8;
                    }
                }
            }
            cursor += length;
        }
        ensure!(
            uncompressed != 0xffffffff && compressed != 0xffffffff && offset != 0xffffffff,
            "missing ZIP64 value"
        );
        expanded = expanded
            .checked_add(uncompressed)
            .context("expanded overflow")?;
        if expanded > budget.limits.expanded_bytes.saturating_sub(budget.expanded) {
            return Err(ResourceFailure::ExpandedBudgetExhausted.into());
        }
        ensure!(
            offset.checked_add(30).is_some_and(|end| end <= start),
            "invalid local header offset"
        );
        let local = read_at(file, offset, 30)?;
        ensure!(
            local[..4] == *b"PK\x03\x04"
                && u16_at(&local, 6) == u16_at(&fixed, 8)
                && u16_at(&local, 8) == u16_at(&fixed, 10)
                && u16_at(&local, 26) as usize == name_len,
            "local and central headers differ"
        );
        ensure!(
            read_at(file, offset + 30, name_len)? == name_raw,
            "local raw name differs"
        );
        let data = offset + 30 + name_len as u64 + u16_at(&local, 28) as u64;
        let mut end = data
            .checked_add(compressed)
            .context("local range overflow")?;
        ensure!(end <= start, "member overlaps directory");
        let local_extra = read_at(
            file,
            offset + 30 + name_len as u64,
            u16_at(&local, 28) as usize,
        )?;
        let (mut local_uncompressed, mut local_compressed) =
            (u32_at(&local, 22) as u64, u32_at(&local, 18) as u64);
        let local_wide = local_uncompressed == 0xffffffff || local_compressed == 0xffffffff;
        let mut cursor = 0;
        let mut wide_seen = false;
        while cursor < local_extra.len() {
            ensure!(
                cursor + 4 <= local_extra.len(),
                "truncated local extra field"
            );
            let (kind, length) = (
                u16_at(&local_extra, cursor),
                u16_at(&local_extra, cursor + 2) as usize,
            );
            cursor += 4;
            ensure!(
                cursor + length <= local_extra.len(),
                "truncated local extra payload"
            );
            if kind == 1 {
                ensure!(!wide_seen, "duplicate local ZIP64 extra");
                wide_seen = true;
                let mut p = cursor;
                for value in [&mut local_uncompressed, &mut local_compressed] {
                    if *value == 0xffffffff {
                        ensure!(p + 8 <= cursor + length, "missing local ZIP64 value");
                        *value = u64_at(&local_extra, p);
                        p += 8;
                    }
                }
            }
            cursor += length;
        }
        if u16_at(&local, 6) & 8 == 0 {
            ensure!(
                u32_at(&local, 14) == u32_at(&fixed, 16)
                    && local_uncompressed == uncompressed
                    && local_compressed == compressed,
                "local size/CRC differs"
            );
        } else {
            ensure!(
                (u32_at(&local, 14) == 0 || u32_at(&local, 14) == u32_at(&fixed, 16))
                    && (local_uncompressed == 0 || local_uncompressed == uncompressed)
                    && (local_compressed == 0 || local_compressed == compressed),
                "local descriptor claims differ"
            );
            ensure!(end + 4 <= start, "missing data descriptor");
            if read_at(file, end, 4)? == b"PK\x07\x08" {
                end += 4;
            }
            let length = if local_wide { 20 } else { 12 };
            ensure!(end + length as u64 <= start, "truncated data descriptor");
            let descriptor = read_at(file, end, length)?;
            let sizes = if local_wide {
                (u64_at(&descriptor, 4), u64_at(&descriptor, 12))
            } else {
                (u32_at(&descriptor, 4) as u64, u32_at(&descriptor, 8) as u64)
            };
            ensure!(
                u32_at(&descriptor, 0) == u32_at(&fixed, 16) && sizes == (compressed, uncompressed),
                "data descriptor differs"
            );
            end += length as u64;
        }
        ranges.push((offset, end));
        records.push((name_raw, uncompressed));
        position += 46 + (name_len + extra_len + comment_len) as u64;
    }
    ensure!(
        position == stop && records.len() as u64 == count && !records.is_empty(),
        "actual directory count differs or is empty"
    );
    ranges.sort_unstable();
    ensure!(
        ranges[0].0 == 0
            && ranges.windows(2).all(|pair| pair[0].1 == pair[1].0)
            && ranges.last().is_some_and(|range| range.1 == start),
        "concatenated or overlapping ZIP variant"
    );
    budget.members += records.len() as u64;
    budget.expanded += expanded;
    Ok(records)
}

fn hash_stream(file: &mut File, bytes: u64, budget: &Budget) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0; 65536];
    loop {
        budget.check()?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= bytes, "archive grew during inspection");
        hash.update(&buffer[..n]);
    }
    ensure!(total == bytes, "archive length changed");
    Ok(hex::encode(hash.finalize()))
}

pub fn inspect_with(path: &Path, budget: &mut Budget) -> Result<Inventory> {
    let mut file = regular_file(path)?;
    let bytes = file.metadata()?.len();
    budget.charge_bytes(bytes)?;
    let sha256 = hash_stream(&mut file, bytes, budget)?;
    let records = directory(&mut file, bytes, budget)?;
    file.seek(SeekFrom::Start(0))?;
    let mut zip = zip::ZipArchive::new(file)?;
    ensure!(zip.len() == records.len(), "ZIP parser inventory differs");
    let mut members = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        budget.check()?;
        let mut entry = zip.by_index(index)?;
        ensure!(
            record.0 == entry.name_raw()
                && record.1 == entry.size()
                && std::str::from_utf8(&record.0)? == entry.name(),
            "ZIP parser raw member interpretation differs"
        );
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| matches!(mode & 0o170000, 0 | 0o100000 | 0o040000)),
            "linked or special member is unsupported"
        );
        let raw_name_hex = hex::encode(entry.name_raw());
        let name = entry.name().to_owned();
        let directory = entry.is_dir();
        let size = entry.size();
        let mut hash = Sha256::new();
        let mut read = 0;
        let mut buffer = [0; 65536];
        loop {
            budget.check()?;
            let allowed = size
                .saturating_sub(read)
                .saturating_add(1)
                .min(buffer.len() as u64) as usize;
            let n = entry.read(&mut buffer[..allowed])?;
            if n == 0 {
                break;
            }
            read += n as u64;
            ensure!(read <= size, "actual decompression exceeds admitted size");
            hash.update(&buffer[..n]);
        }
        ensure!(read == size, "member length differs");
        ensure!(
            members
                .insert(
                    name,
                    Member {
                        raw_name_hex,
                        size,
                        sha256: hex::encode(hash.finalize()),
                        directory
                    }
                )
                .is_none(),
            "duplicate member"
        );
    }
    let mut file = zip.into_inner();
    ensure!(
        hash_stream(&mut file, bytes, budget)? == sha256,
        "archive changed during inspection"
    );
    Ok(Inventory {
        sha256,
        bytes,
        members,
    })
}

pub fn compare(original: &Inventory, observed: &Inventory) -> Comparison {
    let missing_members = original
        .members
        .keys()
        .filter(|name| !observed.members.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let added_members = observed
        .members
        .keys()
        .filter(|name| !original.members.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    let changed_members = observed
        .members
        .iter()
        .filter(|(name, member)| {
            original
                .members
                .get(*name)
                .is_some_and(|old| old != *member)
        })
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let state = if original.sha256 == observed.sha256
        && original.bytes == observed.bytes
        && original.members == observed.members
    {
        ArchiveState::IdenticalRawArchive
    } else if missing_members.is_empty() && added_members.is_empty() && changed_members.is_empty() {
        ArchiveState::EquivalentMemberContent
    } else if !missing_members.is_empty() && added_members.is_empty() && changed_members.is_empty()
    {
        ArchiveState::OmissionOnlyRegression
    } else {
        ArchiveState::AddedOrChangedConflict
    };
    Comparison {
        state,
        missing_members,
        added_members,
        changed_members,
    }
}
